//! Crypto options flow from Deribit's public trade history: every BTC/ETH option print with
//! its premium, implied volatility and taker side, flagged when it was a block trade, plus a
//! summary (call vs put premium bought and sold, busiest strikes and expiries).
//!
//! Source: `GET /api/v2/public/get_last_trades_by_currency?kind=option` (polled; trades are
//! merged by id). Deribit option prices are quoted in the underlying, so premium in USD is
//! `price × amount × index`.

use std::collections::{BTreeMap, HashSet, VecDeque};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::perps::DERIBIT;
use crate::{Http, int, num};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OptionTrade {
    pub id: String,
    /// Unix ms.
    pub ts: i64,
    pub instrument: String,
    pub currency: String,
    /// Unix seconds.
    pub expiry: i64,
    pub strike: f64,
    pub call: bool,
    /// The taker bought.
    pub buy: bool,
    /// Contracts (1 contract = 1 coin).
    pub amount: f64,
    /// Price in the underlying.
    pub price: f64,
    pub index: f64,
    pub iv: Option<f64>,
    pub block: bool,
}

impl OptionTrade {
    pub fn premium_usd(&self) -> f64 {
        self.price * self.amount * self.index
    }

    pub fn notional_usd(&self) -> f64 {
        self.amount * self.index
    }

    /// `25DEC26 80000 C`.
    pub fn label(&self) -> String {
        let mut parts = self.instrument.splitn(2, '-');
        parts.next();
        parts.next().unwrap_or(&self.instrument).replace('-', " ")
    }
}

/// `BTC-25DEC26-80000-C` -> (currency, expiry, strike, call). Linear USDC options are skipped.
pub fn parse_instrument(name: &str) -> Option<(String, i64, f64, bool)> {
    let p: Vec<&str> = name.split('-').collect();
    let [ccy, date, strike, kind] = p.as_slice() else { return None };
    if ccy.contains('_') {
        return None;
    }
    let call = match *kind {
        "C" => true,
        "P" => false,
        _ => return None,
    };
    let expiry = crate::deribit::parse_expiry(&format!("{ccy}-{date}"))?;
    Some((ccy.to_string(), expiry, strike.replace('d', ".").parse().ok()?, call))
}

/// `get_last_trades_by_currency`: `{"result":{"trades":[…],"has_more":…}}`.
pub fn parse_trades(json: &str) -> Result<Vec<OptionTrade>> {
    let v: Value = serde_json::from_str(json).context("parsing Deribit option trades")?;
    let trades = v["result"]["trades"].as_array().context("Deribit: no trades")?;
    Ok(trades
        .iter()
        .filter_map(|t| {
            let instrument = t["instrument_name"].as_str()?;
            let (currency, expiry, strike, call) = parse_instrument(instrument)?;
            Some(OptionTrade {
                id: t["trade_id"]
                    .as_str()
                    .map(str::to_string)
                    .or_else(|| int(&t["trade_id"]).map(|i| i.to_string()))?,
                ts: int(&t["timestamp"])?,
                instrument: instrument.into(),
                currency,
                expiry,
                strike,
                call,
                buy: t["direction"].as_str()? == "buy",
                amount: num(&t["amount"])?,
                price: num(&t["price"])?,
                index: num(&t["index_price"])?,
                iv: num(&t["iv"]).map(|x| x / 100.0),
                block: t.get("block_trade_id").is_some_and(|b| !b.is_null()),
            })
        })
        .collect())
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Premiums {
    pub calls_bought: f64,
    pub calls_sold: f64,
    pub puts_bought: f64,
    pub puts_sold: f64,
    pub count: usize,
}

impl Premiums {
    pub fn add(&mut self, t: &OptionTrade) {
        let p = t.premium_usd();
        match (t.call, t.buy) {
            (true, true) => self.calls_bought += p,
            (true, false) => self.calls_sold += p,
            (false, true) => self.puts_bought += p,
            (false, false) => self.puts_sold += p,
        }
        self.count += 1;
    }

    pub fn calls(&self) -> f64 {
        self.calls_bought + self.calls_sold
    }

    pub fn puts(&self) -> f64 {
        self.puts_bought + self.puts_sold
    }

    /// Put/call premium ratio.
    pub fn put_call(&self) -> Option<f64> {
        (self.calls() > 0.0).then(|| self.puts() / self.calls())
    }
}

/// Recent option trades (newest first) for BTC and ETH.
#[derive(Debug, Default)]
pub struct Tape {
    pub trades: VecDeque<OptionTrade>,
    seen: HashSet<String>,
    pub error: Option<String>,
    pub simulated: bool,
}

impl Tape {
    pub const CAP: usize = 20_000;

    /// Merge newly fetched trades (any order), skipping ones already seen.
    pub fn merge(&mut self, mut trades: Vec<OptionTrade>) {
        trades.sort_by_key(|t| t.ts);
        for t in trades {
            if self.seen.insert(t.id.clone()) {
                let at = self.trades.iter().position(|x| x.ts <= t.ts).unwrap_or(self.trades.len());
                self.trades.insert(at, t);
            }
        }
        while self.trades.len() > Self::CAP {
            if let Some(t) = self.trades.pop_back() {
                self.seen.remove(&t.id);
            }
        }
    }

    pub fn filtered<'a>(
        &'a self,
        currency: Option<&'a str>,
        since_ms: i64,
    ) -> impl Iterator<Item = &'a OptionTrade> + 'a {
        self.trades
            .iter()
            .take_while(move |t| t.ts >= since_ms)
            .filter(move |t| currency.is_none_or(|c| t.currency.eq_ignore_ascii_case(c)))
    }

    pub fn premiums(&self, currency: Option<&str>, since_ms: i64) -> Premiums {
        let mut p = Premiums::default();
        for t in self.filtered(currency, since_ms) {
            p.add(t);
        }
        p
    }

    /// Premium by strike (calls, puts), largest total first.
    pub fn by_strike(&self, currency: Option<&str>, since_ms: i64) -> Vec<(f64, f64, f64)> {
        let mut m: BTreeMap<i64, (f64, f64)> = BTreeMap::new();
        for t in self.filtered(currency, since_ms) {
            let e = m.entry(t.strike as i64).or_default();
            if t.call {
                e.0 += t.premium_usd();
            } else {
                e.1 += t.premium_usd();
            }
        }
        let mut v: Vec<(f64, f64, f64)> = m.into_iter().map(|(k, (c, p))| (k as f64, c, p)).collect();
        v.sort_by(|a, b| (b.1 + b.2).total_cmp(&(a.1 + a.2)));
        v
    }

    /// Premium by expiry (Unix seconds), soonest first.
    pub fn by_expiry(&self, currency: Option<&str>, since_ms: i64) -> Vec<(i64, Premiums)> {
        let mut m: BTreeMap<i64, Premiums> = BTreeMap::new();
        for t in self.filtered(currency, since_ms) {
            m.entry(t.expiry).or_default().add(t);
        }
        m.into_iter().collect()
    }
}

pub async fn fetch(http: &Http, currency: &str) -> Result<Vec<OptionTrade>> {
    let q = [
        ("currency", currency.to_string()),
        ("kind", "option".into()),
        ("count", "1000".into()),
        ("sorting", "desc".into()),
    ];
    parse_trades(&http.get(&format!("{DERIBIT}/public/get_last_trades_by_currency"), &q).await?)
}

/// Poll BTC and ETH option trades every 15 s into `tape` until nobody else holds it.
pub async fn run(tape: std::sync::Arc<std::sync::Mutex<Tape>>, http: Http) {
    while std::sync::Arc::strong_count(&tape) > 1 {
        let (b, e) = tokio::join!(fetch(&http, "BTC"), fetch(&http, "ETH"));
        if let Ok(mut t) = tape.lock() {
            t.error = None;
            for r in [b, e] {
                match r {
                    Ok(trades) => t.merge(trades),
                    Err(err) => t.error = Some(format!("{err:#}")),
                }
            }
        }
        tokio::time::sleep(std::time::Duration::from_secs(15)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instruments() {
        let (c, exp, k, call) = parse_instrument("BTC-25DEC26-80000-C").unwrap();
        assert_eq!((c.as_str(), k, call), ("BTC", 80_000.0, true));
        assert_eq!(exp, crate::deribit::parse_expiry("BTC-25DEC26").unwrap());
        assert!(parse_instrument("ETH-31OCT26-2500-P").is_some_and(|x| !x.3));
        assert_eq!(parse_instrument("SOL_USDC-31OCT26-150-C"), None);
        assert_eq!(parse_instrument("BTC-PERPETUAL"), None);
    }

    #[test]
    fn trades_and_tape() {
        let j =
            std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/deribit_option_trades.json"))
                .unwrap();
        let ts = parse_trades(&j).unwrap();
        assert_eq!(ts.len(), 3, "linear option skipped");
        let block = ts.iter().find(|t| t.block).unwrap();
        assert!((block.premium_usd() - 0.0215 * 250.0 * 67_000.0).abs() < 1e-6);
        assert_eq!(block.iv, Some(0.512));
        assert_eq!(block.label(), "25DEC26 80000 C");
        let mut tape = Tape::default();
        tape.merge(ts.clone());
        tape.merge(ts);
        assert_eq!(tape.trades.len(), 3, "deduplicated");
        assert!(tape.trades[0].ts >= tape.trades[1].ts, "newest first");
        let p = tape.premiums(Some("BTC"), 0);
        assert_eq!(p.count, 2);
        assert!(p.calls_bought > 0.0 && p.puts_sold > 0.0);
        assert!(p.put_call().is_some());
        assert_eq!(tape.by_strike(None, 0)[0].0, 80_000.0);
        assert_eq!(tape.by_expiry(Some("ETH"), 0).len(), 1);
    }
}
