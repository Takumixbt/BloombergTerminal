//! Crypto alerts (Velo-style): price, 24h change, funding, open interest change and large
//! liquidations, written as one-line rules:
//!
//! ```text
//! BTC > 70000          BTC price above 70,000
//! ETH < 2400           ETH price below 2,400
//! SOL CHG > 8%         SOL up more than 8% in 24h
//! HYPE FUNDING > 0.05% HYPE OI-weighted funding above 0.05% per 8h
//! BTC OI < -5%         BTC open interest down more than 5% in 24h
//! LIQ > 1M             any single liquidation over $1M (BTC LIQ > 250K for one coin)
//! ```
//!
//! Rules fire when their condition becomes true and re-arm when it is false again.

use serde::{Deserialize, Serialize};

use crate::liq::Liquidation;
use crate::perps::Board;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Metric {
    Price,
    Change,
    Funding,
    OiChange,
    Liquidation,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Rule {
    pub coin: Option<String>,
    pub metric: Metric,
    pub above: bool,
    /// Price or USD, or a fraction for percentages.
    pub value: f64,
    /// Currently true (fired and not yet re-armed).
    #[serde(default)]
    pub active: bool,
    /// Unix seconds of the last firing.
    #[serde(default)]
    pub fired_at: Option<i64>,
}

fn amount(s: &str) -> Option<(f64, bool)> {
    let s = s.trim().trim_start_matches('$').replace(',', "");
    let (s, pct) = match s.strip_suffix('%') {
        Some(x) => (x.to_string(), true),
        None => (s, false),
    };
    let (n, mult) = match s.chars().last()?.to_ascii_uppercase() {
        'K' => (&s[..s.len() - 1], 1e3),
        'M' => (&s[..s.len() - 1], 1e6),
        'B' => (&s[..s.len() - 1], 1e9),
        _ => (&s[..], 1.0),
    };
    let v: f64 = n.parse().ok()?;
    Some(if pct { (v / 100.0, true) } else { (v * mult, false) })
}

impl Rule {
    /// Parse `[COIN] [METRIC] >|< VALUE` (see the module docs).
    pub fn parse(s: &str) -> Option<Rule> {
        let s = s.trim().to_ascii_uppercase();
        let (lhs, rhs, above) = if let Some((l, r)) = s.split_once('>') {
            (l, r, true)
        } else {
            let (l, r) = s.split_once('<')?;
            (l, r, false)
        };
        let words: Vec<&str> = lhs.split_whitespace().collect();
        let metric_of = |w: &str| match w {
            "PRICE" | "PX" => Some(Metric::Price),
            "CHG" | "CHANGE" | "24H" => Some(Metric::Change),
            "FUNDING" | "FUND" | "FR" => Some(Metric::Funding),
            "OI" => Some(Metric::OiChange),
            "LIQ" | "LIQS" | "LIQUIDATION" => Some(Metric::Liquidation),
            _ => None,
        };
        let (coin, metric) = match words.as_slice() {
            [w] => match metric_of(w) {
                Some(m @ Metric::Liquidation) => (None, m),
                Some(_) => return None,
                None => (Some(crate::venue::coin_of_key(w)), Metric::Price),
            },
            [c, w] => (Some(crate::venue::coin_of_key(c)), metric_of(w)?),
            _ => return None,
        };
        let (value, pct) = amount(rhs)?;
        let wants_pct = matches!(metric, Metric::Change | Metric::Funding | Metric::OiChange);
        if pct != wants_pct {
            return None;
        }
        Some(Rule { coin, metric, above, value, active: false, fired_at: None })
    }

    pub fn describe(&self) -> String {
        let coin = self.coin.as_deref().unwrap_or("Any");
        let op = if self.above { ">" } else { "<" };
        match self.metric {
            Metric::Price => format!("{coin} price {op} {}", self.value),
            Metric::Change => format!("{coin} 24h change {op} {:+.2}%", self.value * 100.0),
            Metric::Funding => format!("{coin} funding/8h {op} {:+.4}%", self.value * 100.0),
            Metric::OiChange => format!("{coin} OI 24h {op} {:+.1}%", self.value * 100.0),
            Metric::Liquidation => format!("{coin} liquidation {op} {}", crate::fmt_usd(self.value)),
        }
    }

    fn test(&self, x: f64) -> bool {
        if self.above { x > self.value } else { x < self.value }
    }

    /// The current value for this rule from the board (not for liquidations).
    pub fn current(&self, board: &Board) -> Option<f64> {
        let row = board.row(self.coin.as_deref()?)?;
        match self.metric {
            Metric::Price => Some(row.price),
            Metric::Change => row.change_24h,
            Metric::Funding => row.funding_8h,
            Metric::OiChange => row.oi_change_24h,
            Metric::Liquidation => None,
        }
    }
}

/// Evaluate `rules` against the board and liquidations newer than `liq_since_ms`; returns the
/// messages of rules that fired now.
pub fn evaluate(rules: &mut [Rule], board: Option<&Board>, liqs: &[Liquidation], now: i64) -> Vec<String> {
    let mut fired = Vec::new();
    for r in rules.iter_mut() {
        let hit = match r.metric {
            Metric::Liquidation => liqs
                .iter()
                .filter(|l| r.coin.as_deref().is_none_or(|c| l.coin.eq_ignore_ascii_case(c)))
                .find(|l| r.test(l.usd))
                .map(|l| {
                    format!(
                        "{} {} liquidation {} on {}",
                        l.coin,
                        if l.side == crate::liq::Side::Long { "long" } else { "short" },
                        crate::fmt_usd(l.usd),
                        l.exchange.label()
                    )
                }),
            _ => board.and_then(|b| r.current(b)).filter(|x| r.test(*x)).map(|_| format!("Alert: {}", r.describe())),
        };
        match hit {
            // Liquidations are events: each matching batch fires.
            Some(msg) if r.metric == Metric::Liquidation => {
                r.fired_at = Some(now);
                fired.push(msg);
            }
            Some(msg) if !r.active => {
                r.active = true;
                r.fired_at = Some(now);
                fired.push(msg);
            }
            Some(_) => {}
            None => r.active = false,
        }
    }
    fired
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::perps::CoinRow;

    #[test]
    fn parse_rules() {
        let r = Rule::parse("btc > 70k").unwrap();
        assert_eq!((r.coin.as_deref(), r.metric, r.above, r.value), (Some("BTC"), Metric::Price, true, 70_000.0));
        let f = Rule::parse("HYPE FUNDING > 0.05%").unwrap();
        assert_eq!((f.metric, f.value), (Metric::Funding, 0.0005));
        assert_eq!(Rule::parse("ETH OI < -5%").unwrap().value, -0.05);
        let l = Rule::parse("LIQ > 1M").unwrap();
        assert_eq!((l.coin, l.metric, l.value), (None, Metric::Liquidation, 1e6));
        assert!(Rule::parse("BTC FUNDING > 5").is_none(), "funding needs a percentage");
        assert!(Rule::parse("BTC > 5%").is_none(), "price is not a percentage");
        assert!(Rule::parse("hello").is_none());
        assert_eq!(Rule::parse("SOL CHG > 8%").unwrap().describe(), "SOL 24h change > +8.00%");
    }

    #[test]
    fn edge_triggered() {
        let mut rules = vec![Rule::parse("BTC > 100").unwrap(), Rule::parse("LIQ > 1M").unwrap()];
        let board = |p: f64| Board {
            rows: vec![CoinRow { coin: "BTC".into(), price: p, ..Default::default() }],
            ..Default::default()
        };
        assert!(evaluate(&mut rules, Some(&board(99.0)), &[], 1).is_empty());
        assert_eq!(evaluate(&mut rules, Some(&board(101.0)), &[], 2).len(), 1);
        assert!(evaluate(&mut rules, Some(&board(102.0)), &[], 3).is_empty(), "stays quiet while true");
        assert!(evaluate(&mut rules, Some(&board(98.0)), &[], 4).is_empty());
        assert_eq!(evaluate(&mut rules, Some(&board(105.0)), &[], 5).len(), 1, "re-armed");
        let liq = Liquidation {
            exchange: crate::Exchange::Bybit,
            coin: "ETH".into(),
            side: crate::liq::Side::Long,
            price: 1.0,
            qty: 1.0,
            usd: 2e6,
            ts: 0,
        };
        let m = evaluate(&mut rules, None, &[liq], 6);
        assert_eq!(m, ["ETH long liquidation $2.0M on Bybit"]);
    }
}
