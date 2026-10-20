//! Live liquidations: Binance `!forceOrder@arr`, Bybit `allLiquidation.<symbol>` and OKX
//! `liquidation-orders` (SWAP), normalised to [`Liquidation`] and collected in a [`Tape`].
//!
//! Binance pushes at most one liquidation per symbol per second (the largest), so its feed
//! undercounts in cascades; Bybit and OKX push every order.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;

use crate::venue::{Contract, Exchange};
use crate::{int, num};

pub const BINANCE_WS: &str = "wss://fstream.binance.com/ws/!forceOrder@arr";
pub const BYBIT_WS: &str = "wss://stream.bybit.com/v5/public/linear";
pub const OKX_WS: &str = "wss://ws.okx.com:8443/ws/v5/public";

/// Which side was liquidated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Side {
    /// A long was closed by a forced sell.
    Long,
    /// A short was closed by a forced buy.
    Short,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Liquidation {
    pub exchange: Exchange,
    pub coin: String,
    pub side: Side,
    /// Price per coin.
    pub price: f64,
    /// Size in coins.
    pub qty: f64,
    pub usd: f64,
    /// Unix ms.
    pub ts: i64,
}

/// Binance `forceOrder` event: `{"e":"forceOrder","o":{"s","S","q","p","ap","z","T",…}}`.
/// `S` is the liquidation order's side (`SELL` closes a long).
pub fn parse_binance(text: &str) -> Option<Liquidation> {
    let v: Value = serde_json::from_str(text).ok()?;
    let o = v.get("o")?;
    let c = Contract::from_usdt_perp(o["s"].as_str()?)?;
    let side = match o["S"].as_str()? {
        "SELL" => Side::Long,
        "BUY" => Side::Short,
        _ => return None,
    };
    let price = num(&o["ap"]).filter(|p| *p > 0.0).or_else(|| num(&o["p"]))?;
    let qty = num(&o["z"]).filter(|q| *q > 0.0).or_else(|| num(&o["q"]))?;
    Some(Liquidation {
        exchange: Exchange::Binance,
        coin: c.coin,
        side,
        price: price / c.mult,
        qty: qty * c.mult,
        usd: price * qty,
        ts: int(&o["T"]).or_else(|| int(&v["E"]))?,
    })
}

/// Bybit `allLiquidation.<symbol>`: `{"topic","data":[{"T","s","S","v","p"}]}`, where
/// `S = Buy` means a long position was liquidated.
pub fn parse_bybit(text: &str) -> Vec<Liquidation> {
    let Ok(v) = serde_json::from_str::<Value>(text) else { return Vec::new() };
    if !v["topic"].as_str().is_some_and(|t| t.starts_with("allLiquidation.")) {
        return Vec::new();
    }
    v["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|d| {
            let c = Contract::from_usdt_perp(d["s"].as_str()?)?;
            let side = match d["S"].as_str()? {
                "Buy" => Side::Long,
                "Sell" => Side::Short,
                _ => return None,
            };
            let (price, qty) = (num(&d["p"])?, num(&d["v"])?);
            Some(Liquidation {
                exchange: Exchange::Bybit,
                coin: c.coin,
                side,
                price: price / c.mult,
                qty: qty * c.mult,
                usd: price * qty,
                ts: int(&d["T"])?,
            })
        })
        .collect()
}

/// OKX `liquidation-orders`: `{"arg","data":[{"instId","details":[{"side","posSide","bkPx",
/// "sz","ts"}]}]}`. `sz` is in contracts, converted with each swap's `ctVal` (coins per
/// contract, from `/api/v5/public/instruments`). `side = sell` closes a long.
pub fn parse_okx(text: &str, ct_val: &HashMap<String, f64>) -> Vec<Liquidation> {
    let Ok(v) = serde_json::from_str::<Value>(text) else { return Vec::new() };
    if v["arg"]["channel"] != "liquidation-orders" {
        return Vec::new();
    }
    let mut out = Vec::new();
    for d in v["data"].as_array().into_iter().flatten() {
        let Some(inst) = d["instId"].as_str() else { continue };
        let Some(c) = Contract::from_okx_swap(inst) else { continue };
        let Some(&cv) = ct_val.get(inst) else { continue };
        for x in d["details"].as_array().into_iter().flatten() {
            let side = match (x["side"].as_str(), x["posSide"].as_str()) {
                (_, Some("long")) | (Some("sell"), _) => Side::Long,
                (_, Some("short")) | (Some("buy"), _) => Side::Short,
                _ => continue,
            };
            let (Some(price), Some(sz), Some(ts)) = (num(&x["bkPx"]), num(&x["sz"]), int(&x["ts"])) else { continue };
            let qty = sz * cv;
            out.push(Liquidation {
                exchange: Exchange::Okx,
                coin: c.coin.clone(),
                side,
                price,
                qty,
                usd: price * qty,
                ts,
            });
        }
    }
    out
}

/// `/api/v5/public/instruments?instType=SWAP`: contract value per instrument.
pub fn parse_okx_ct_val(json: &str) -> Result<HashMap<String, f64>> {
    let v: Value = serde_json::from_str(json)?;
    Ok(v["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| Some((r["instId"].as_str()?.to_string(), num(&r["ctVal"])?)))
        .collect())
}

/// Totals for one side over a window.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Totals {
    pub long_usd: f64,
    pub short_usd: f64,
    pub count: usize,
}

/// The most recent liquidations (newest first) and connection status per venue.
#[derive(Debug, Default)]
pub struct Tape {
    pub items: VecDeque<Liquidation>,
    pub status: HashMap<Exchange, String>,
    pub simulated: bool,
}

impl Tape {
    pub const CAP: usize = 5_000;

    pub fn push(&mut self, l: Liquidation) {
        self.items.push_front(l);
        self.items.truncate(Self::CAP);
    }

    /// Totals since `since_ms`, optionally for one coin.
    pub fn totals(&self, since_ms: i64, coin: Option<&str>) -> Totals {
        let mut t = Totals::default();
        for l in self.items.iter().take_while(|l| l.ts >= since_ms) {
            if coin.is_some_and(|c| !l.coin.eq_ignore_ascii_case(c)) {
                continue;
            }
            match l.side {
                Side::Long => t.long_usd += l.usd,
                Side::Short => t.short_usd += l.usd,
            }
            t.count += 1;
        }
        t
    }

    /// Totals per coin since `since_ms`, largest first.
    pub fn by_coin(&self, since_ms: i64) -> Vec<(String, Totals)> {
        let mut m: HashMap<&str, Totals> = HashMap::new();
        for l in self.items.iter().take_while(|l| l.ts >= since_ms) {
            let t = m.entry(&l.coin).or_default();
            match l.side {
                Side::Long => t.long_usd += l.usd,
                Side::Short => t.short_usd += l.usd,
            }
            t.count += 1;
        }
        let mut v: Vec<(String, Totals)> = m.into_iter().map(|(k, t)| (k.to_string(), t)).collect();
        v.sort_by(|a, b| (b.1.long_usd + b.1.short_usd).total_cmp(&(a.1.long_usd + a.1.short_usd)));
        v
    }
}

pub type SharedTape = Arc<Mutex<Tape>>;

fn set_status(tape: &SharedTape, e: Exchange, s: impl Into<String>) {
    if let Ok(mut t) = tape.lock() {
        t.status.insert(e, s.into());
    }
}

fn push_all(tape: &SharedTape, ls: impl IntoIterator<Item = Liquidation>) {
    if let Ok(mut t) = tape.lock() {
        for l in ls {
            t.push(l);
        }
    }
}

/// True while someone besides the stream tasks still holds the tape (`holders` = the number
/// of task-owned clones).
fn wanted(tape: &SharedTape, holders: usize) -> bool {
    Arc::strong_count(tape) > holders
}

/// Run all three liquidation feeds into `tape` until every other holder drops it.
/// `bybit_symbols` are the Bybit contracts to subscribe to (it has no all-market topic).
pub async fn run(tape: SharedTape, http: crate::Http, bybit_symbols: Vec<String>) {
    // This task and the three feeds below each hold one clone.
    const HOLDERS: usize = 4;
    let ct_val = match http
        .get(&format!("{}/api/v5/public/instruments", crate::perps::OKX), &[("instType", "SWAP".into())])
        .await
    {
        Ok(j) => parse_okx_ct_val(&j).unwrap_or_default(),
        Err(e) => {
            set_status(&tape, Exchange::Okx, format!("instruments: {e:#}"));
            HashMap::new()
        }
    };
    let a = feed(tape.clone(), Exchange::Binance, HOLDERS, Vec::new(), |t| parse_binance(t).into_iter().collect());
    let subs: Vec<String> = bybit_symbols
        .chunks(10)
        .map(|c| {
            json!({"op": "subscribe", "args": c.iter().map(|s| format!("allLiquidation.{s}")).collect::<Vec<_>>()})
                .to_string()
        })
        .collect();
    let b = feed(tape.clone(), Exchange::Bybit, HOLDERS, subs, parse_bybit);
    let okx_sub =
        json!({"op": "subscribe", "args": [{"channel": "liquidation-orders", "instType": "SWAP"}]}).to_string();
    let ct = ct_val;
    let c = feed(tape.clone(), Exchange::Okx, HOLDERS, vec![okx_sub], move |t| parse_okx(t, &ct));
    tokio::join!(a, b, c);
    drop(tape);
}

/// One reconnecting websocket feed: `subs` are the subscribe messages, `parse` turns each
/// text frame into liquidations. Venue keep-alives (Bybit and OKX expect a ping every ~20 s)
/// are sent from here.
async fn feed(
    tape: SharedTape,
    ex: Exchange,
    holders: usize,
    subs: Vec<String>,
    parse: impl Fn(&str) -> Vec<Liquidation>,
) {
    let url = match ex {
        Exchange::Binance => BINANCE_WS,
        Exchange::Bybit => BYBIT_WS,
        _ => OKX_WS,
    };
    let mut backoff = 1u64;
    while wanted(&tape, holders) {
        set_status(&tape, ex, "connecting");
        let conn = tokio::time::timeout(Duration::from_secs(15), tokio_tungstenite::connect_async(url)).await;
        let mut ws = match conn {
            Ok(Ok((ws, _))) => ws,
            Ok(Err(e)) => {
                set_status(&tape, ex, format!("offline: {e}"));
                tokio::time::sleep(Duration::from_secs(backoff)).await;
                backoff = (backoff * 2).min(60);
                continue;
            }
            Err(_) => {
                set_status(&tape, ex, "offline: connect timed out");
                tokio::time::sleep(Duration::from_secs(backoff)).await;
                backoff = (backoff * 2).min(60);
                continue;
            }
        };
        let mut ok = true;
        for s in &subs {
            ok &= ws.send(Message::Text(s.clone().into())).await.is_ok();
        }
        if !ok {
            continue;
        }
        set_status(&tape, ex, "live");
        backoff = 1;
        let mut ping = tokio::time::interval(Duration::from_secs(18));
        loop {
            tokio::select! {
                msg = ws.next() => match msg {
                    Some(Ok(Message::Text(t))) => push_all(&tape, parse(t.as_str())),
                    Some(Ok(Message::Ping(p))) => { let _ = ws.send(Message::Pong(p)).await; }
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                    _ => {}
                },
                _ = ping.tick() => {
                    if !wanted(&tape, holders) {
                        let _ = ws.close(None).await;
                        return;
                    }
                    let keepalive = match ex {
                        Exchange::Bybit => Some(Message::Text(r#"{"op":"ping"}"#.into())),
                        Exchange::Okx => Some(Message::Text("ping".into())),
                        _ => None,
                    };
                    if let Some(k) = keepalive && ws.send(k).await.is_err() {
                        break;
                    }
                }
            }
        }
        set_status(&tape, ex, "reconnecting");
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binance_force_order() {
        let l = parse_binance(
            r#"{"e":"forceOrder","E":1760936400100,"o":{"s":"1000PEPEUSDT","S":"SELL","o":"LIMIT","f":"IOC","q":"100000","p":"0.0122","ap":"0.0123","X":"FILLED","l":"100000","z":"100000","T":1760936400050}}"#,
        )
        .unwrap();
        assert_eq!((l.exchange, l.coin.as_str(), l.side), (Exchange::Binance, "PEPE", Side::Long));
        assert!((l.usd - 1230.0).abs() < 1e-9);
        assert!((l.qty - 1e8).abs() < 1e-3, "coins, not contracts");
        assert!((l.price - 0.0000123).abs() < 1e-15);
        assert_eq!(l.ts, 1_760_936_400_050);
        assert!(parse_binance(r#"{"e":"other"}"#).is_none());
    }

    #[test]
    fn bybit_all_liquidation() {
        let ls = parse_bybit(
            r#"{"topic":"allLiquidation.BTCUSDT","type":"snapshot","ts":1760936400200,"data":[{"T":1760936400150,"s":"BTCUSDT","S":"Sell","v":"0.5","p":"67000"},{"T":1760936400160,"s":"BTCUSDT","S":"Buy","v":"0.1","p":"66900"}]}"#,
        );
        assert_eq!(ls.len(), 2);
        assert_eq!(ls[0].side, Side::Short);
        assert_eq!(ls[1].side, Side::Long);
        assert!((ls[0].usd - 33_500.0).abs() < 1e-9);
        assert!(parse_bybit(r#"{"success":true,"op":"subscribe"}"#).is_empty());
    }

    #[test]
    fn okx_liquidation_orders() {
        let ct = parse_okx_ct_val(r#"{"code":"0","data":[{"instId":"BTC-USDT-SWAP","ctVal":"0.01"},{"instId":"ETH-USDT-SWAP","ctVal":"0.1"}]}"#).unwrap();
        let ls = parse_okx(
            r#"{"arg":{"channel":"liquidation-orders","instType":"SWAP"},"data":[{"details":[{"bkLoss":"0","bkPx":"66950.5","ccy":"","posSide":"long","side":"sell","sz":"20","ts":"1760936400300"}],"instFamily":"BTC-USDT","instId":"BTC-USDT-SWAP","instType":"SWAP","uly":"BTC-USDT"},{"details":[{"bkPx":"2570","posSide":"net","side":"buy","sz":"5","ts":"1760936400310"}],"instId":"ETH-USDT-SWAP"},{"details":[{"bkPx":"1","posSide":"short","side":"buy","sz":"5","ts":"1"}],"instId":"BTC-USD-SWAP"}]}"#,
            &ct,
        );
        assert_eq!(ls.len(), 2, "inverse swap skipped");
        assert_eq!((ls[0].coin.as_str(), ls[0].side), ("BTC", Side::Long));
        assert!((ls[0].qty - 0.2).abs() < 1e-12);
        assert!((ls[0].usd - 13_390.1).abs() < 1e-6);
        assert_eq!(ls[1].side, Side::Short, "net mode falls back to the order side");
    }

    #[test]
    fn tape_totals() {
        let mut t = Tape::default();
        let l = |coin: &str, side, usd, ts| Liquidation {
            exchange: Exchange::Bybit,
            coin: coin.into(),
            side,
            price: 1.0,
            qty: usd,
            usd,
            ts,
        };
        t.push(l("BTC", Side::Long, 100.0, 1_000));
        t.push(l("ETH", Side::Short, 50.0, 2_000));
        t.push(l("BTC", Side::Short, 25.0, 3_000));
        let all = t.totals(0, None);
        assert_eq!((all.long_usd, all.short_usd, all.count), (100.0, 75.0, 3));
        let recent = t.totals(1_500, Some("btc"));
        assert_eq!((recent.long_usd, recent.short_usd, recent.count), (0.0, 25.0, 1));
        let by = t.by_coin(0);
        assert_eq!(by[0].0, "BTC");
    }
}
