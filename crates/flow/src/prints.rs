//! Whale prints: large trades on Hyperliquid with the wallets on both sides (Hyperliquid's
//! public `trades` stream carries `users: [buyer, seller]`), so every big print links to a
//! wallet you can open.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;

use crate::orderflow::HYPERLIQUID_WS;
use crate::{int, num};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Print {
    pub coin: String,
    pub price: f64,
    pub size: f64,
    pub usd: f64,
    /// The aggressor bought.
    pub buy: bool,
    /// Unix ms.
    pub ts: i64,
    /// Aggressor (taker) and resting (maker) wallets.
    pub taker: String,
    pub maker: String,
}

/// Hyperliquid `trades` channel message -> prints of at least `min_usd`. Spot pairs (`@107`)
/// are skipped; `k` markets are quoted per 1,000 coins, which leaves notional unchanged.
pub fn parse(text: &str, min_usd: f64) -> Vec<Print> {
    let Ok(v) = serde_json::from_str::<Value>(text) else { return Vec::new() };
    if v["channel"] != "trades" {
        return Vec::new();
    }
    v["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|t| {
            let coin = t["coin"].as_str()?;
            if coin.starts_with('@') || coin.contains('/') {
                return None;
            }
            let (price, size) = (num(&t["px"])?, num(&t["sz"])?);
            let usd = price * size;
            (usd >= min_usd).then_some(())?;
            let buy = t["side"].as_str()? == "B";
            let users = t["users"].as_array()?;
            let (buyer, seller) = (users.first()?.as_str()?, users.get(1)?.as_str()?);
            let (taker, maker) = if buy { (buyer, seller) } else { (seller, buyer) };
            Some(Print {
                coin: crate::venue::Contract::from_hyperliquid(coin).map_or_else(|| coin.to_string(), |c| c.coin),
                price,
                size,
                usd,
                buy,
                ts: int(&t["time"])?,
                taker: taker.to_ascii_lowercase(),
                maker: maker.to_ascii_lowercase(),
            })
        })
        .collect()
}

#[derive(Debug, Default)]
pub struct Tape {
    pub prints: VecDeque<Print>,
    pub status: String,
    pub simulated: bool,
}

impl Tape {
    pub const CAP: usize = 5_000;

    pub fn push(&mut self, p: Print) {
        self.prints.push_front(p);
        self.prints.truncate(Self::CAP);
    }
}

pub type SharedTape = Arc<Mutex<Tape>>;

/// Smallest print recorded; panes filter higher.
pub const MIN_USD: f64 = 25_000.0;

/// Stream `coins` (Hyperliquid names) into `tape` until nobody else holds it.
pub async fn run(tape: SharedTape, coins: Vec<String>) {
    let set = |s: &str| {
        if let Ok(mut t) = tape.lock() {
            t.status = s.to_string();
        }
    };
    let mut backoff = 1u64;
    while Arc::strong_count(&tape) > 1 {
        set("connecting");
        let ws = tokio::time::timeout(Duration::from_secs(15), tokio_tungstenite::connect_async(HYPERLIQUID_WS)).await;
        let mut ws = match ws {
            Ok(Ok((ws, _))) => ws,
            _ => {
                set("offline");
                tokio::time::sleep(Duration::from_secs(backoff)).await;
                backoff = (backoff * 2).min(60);
                continue;
            }
        };
        let mut ok = true;
        for c in &coins {
            let sub = json!({"method": "subscribe", "subscription": {"type": "trades", "coin": c}});
            ok &= ws.send(Message::Text(sub.to_string().into())).await.is_ok();
        }
        if !ok {
            continue;
        }
        set("live");
        backoff = 1;
        let mut ping = tokio::time::interval(Duration::from_secs(30));
        loop {
            tokio::select! {
                msg = ws.next() => match msg {
                    Some(Ok(Message::Text(t))) => {
                        let ps = parse(t.as_str(), MIN_USD);
                        if !ps.is_empty() && let Ok(mut tp) = tape.lock() {
                            for p in ps {
                                tp.push(p);
                            }
                        }
                    }
                    Some(Ok(Message::Ping(p))) => { let _ = ws.send(Message::Pong(p)).await; }
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                    _ => {}
                },
                _ = ping.tick() => {
                    if Arc::strong_count(&tape) <= 1 {
                        let _ = ws.close(None).await;
                        return;
                    }
                    if ws.send(Message::Text(r#"{"method":"ping"}"#.into())).await.is_err() {
                        break;
                    }
                }
            }
        }
        set("reconnecting");
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn big_prints_with_wallets() {
        let msg = r#"{"channel":"trades","data":[
            {"coin":"BTC","side":"A","px":"67000.0","sz":"2.0","time":1760936400000,"hash":"0x1","tid":1,"users":["0xAAAA000000000000000000000000000000000001","0xbbbb000000000000000000000000000000000002"]},
            {"coin":"BTC","side":"B","px":"67000.0","sz":"0.01","time":1760936400001,"hash":"0x2","tid":2,"users":["0xa","0xb"]},
            {"coin":"kPEPE","side":"B","px":"0.0123","sz":"5000000","time":1760936400002,"hash":"0x3","tid":3,"users":["0xc","0xd"]},
            {"coin":"@107","side":"B","px":"30.0","sz":"10000","time":1760936400003,"hash":"0x4","tid":4,"users":["0xe","0xf"]}
        ]}"#;
        let p = parse(msg, 25_000.0);
        assert_eq!(p.len(), 2, "small and spot prints skipped");
        assert_eq!(p[0].usd, 134_000.0);
        assert!(!p[0].buy);
        assert_eq!(p[0].taker, "0xbbbb000000000000000000000000000000000002", "seller aggressed");
        assert_eq!(p[0].maker, "0xaaaa000000000000000000000000000000000001");
        assert_eq!((p[1].coin.as_str(), p[1].taker.as_str()), ("PEPE", "0xc"));
        assert!(parse(r#"{"channel":"pong"}"#, 0.0).is_empty());
    }
}
