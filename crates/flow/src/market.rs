//! The market dashboard (Velo-style): returns over 1h / 4h / 24h / 7d / 30d, taker-flow CVD in
//! dollars, open interest and funding per coin, market breadth, and seasonality (average
//! return by weekday × hour of day, and by trading session).
//!
//! Source: Binance USDⓈ-M futures hourly klines (`/fapi/v1/klines`), which also carry taker
//! buy volume, joined with the derivatives board for open interest and funding.

use std::collections::HashMap;

use anyhow::{Context, Result};
use chrono::{Datelike, Timelike};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::perps::{BINANCE_FAPI, Board};
use crate::{Http, int, num};

/// One hourly futures bar with taker flow.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Bar {
    /// Unix seconds (open).
    pub ts: i64,
    pub open: f64,
    pub close: f64,
    /// Quote (USD) volume.
    pub volume: f64,
    /// Quote volume bought by takers.
    pub taker_buy: f64,
}

impl Bar {
    /// Net taker flow in dollars (bought by takers minus sold).
    pub fn delta(&self) -> f64 {
        2.0 * self.taker_buy - self.volume
    }
}

/// `/fapi/v1/klines`: `[open_ms, o, h, l, c, v, close_ms, quote_vol, trades, taker_buy_base,
/// taker_buy_quote, ignore]`.
pub fn parse_klines(json: &str) -> Result<Vec<Bar>> {
    let rows: Vec<Vec<Value>> = serde_json::from_str(json).context("parsing klines")?;
    let mut bars: Vec<Bar> = rows
        .iter()
        .filter_map(|r| {
            Some(Bar {
                ts: int(r.first()?)? / 1000,
                open: num(r.get(1)?)?,
                close: num(r.get(4)?)?,
                volume: num(r.get(7)?)?,
                taker_buy: num(r.get(10)?)?,
            })
        })
        .collect();
    bars.sort_by_key(|b| b.ts);
    bars.dedup_by_key(|b| b.ts);
    Ok(bars)
}

/// The windows the dashboard shows, in hours.
pub const WINDOWS: [(&str, usize); 5] = [("1h", 1), ("4h", 4), ("24h", 24), ("7d", 168), ("30d", 720)];

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Row {
    pub coin: String,
    pub price: f64,
    /// Returns per [`WINDOWS`] entry (fractions).
    pub returns: [Option<f64>; 5],
    pub volume_24h: f64,
    /// Net taker flow over 24 hours, USD.
    pub cvd_24h: f64,
    /// Net taker flow per hour over the last 24 hours, for a sparkline.
    pub cvd_spark: Vec<f64>,
    pub oi_usd: f64,
    pub funding_8h: Option<f64>,
}

/// Returns and flow from hourly bars (newest last).
pub fn row_from_bars(coin: &str, bars: &[Bar]) -> Option<Row> {
    let last = bars.last()?;
    let ret = |h: usize| -> Option<f64> {
        let i = bars.len().checked_sub(h)?;
        let base = bars[i].open;
        (base > 0.0).then(|| last.close / base - 1.0)
    };
    let day = &bars[bars.len().saturating_sub(24)..];
    Some(Row {
        coin: coin.to_string(),
        price: last.close,
        returns: WINDOWS.map(|(_, h)| ret(h)),
        volume_24h: day.iter().map(|b| b.volume).sum(),
        cvd_24h: day.iter().map(Bar::delta).sum(),
        cvd_spark: day
            .iter()
            .scan(0.0, |acc, b| {
                *acc += b.delta();
                Some(*acc)
            })
            .collect(),
        ..Default::default()
    })
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Dashboard {
    pub rows: Vec<Row>,
    pub updated: i64,
    pub simulated: bool,
    pub errors: usize,
}

impl Dashboard {
    /// Coins up and down over window `w`, and the median return.
    pub fn breadth(&self, w: usize) -> (usize, usize, Option<f64>) {
        let mut r: Vec<f64> = self.rows.iter().filter_map(|x| x.returns[w]).collect();
        let up = r.iter().filter(|x| **x > 0.0).count();
        let down = r.iter().filter(|x| **x < 0.0).count();
        r.sort_by(f64::total_cmp);
        (up, down, r.get(r.len() / 2).copied())
    }
}

/// Join open interest and funding from the board.
pub fn join_board(rows: &mut [Row], board: &Board) {
    for r in rows {
        if let Some(b) = board.row(&r.coin) {
            r.oi_usd = b.oi_usd;
            r.funding_8h = b.funding_8h;
        }
    }
}

/// Average return and hit rate by weekday (0 = Monday) × hour (UTC), and by session.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Seasonality {
    pub coin: String,
    /// `[weekday][hour]` mean hourly return.
    pub grid: Vec<Vec<f64>>,
    pub hour_mean: Vec<f64>,
    pub weekday_mean: Vec<f64>,
    /// `(session name, mean hourly return, share of positive hours)`.
    pub sessions: Vec<(String, f64, f64)>,
    pub hours: usize,
    pub simulated: bool,
}

/// UTC sessions: Asia 00–08, Europe 08–13, US 13–21, late US 21–24.
pub const SESSIONS: [(&str, u32, u32); 4] = [("Asia", 0, 8), ("Europe", 8, 13), ("US", 13, 21), ("Late US", 21, 24)];

pub fn seasonality(coin: &str, bars: &[Bar]) -> Seasonality {
    let mut sum = vec![vec![0.0; 24]; 7];
    let mut cnt = vec![vec![0usize; 24]; 7];
    let mut sess = vec![(0.0, 0usize, 0usize); SESSIONS.len()];
    for b in bars.iter().filter(|b| b.open > 0.0) {
        let Some(t) = chrono::DateTime::from_timestamp(b.ts, 0) else { continue };
        let r = b.close / b.open - 1.0;
        let (d, h) = (t.weekday().num_days_from_monday() as usize, t.hour());
        sum[d][h as usize] += r;
        cnt[d][h as usize] += 1;
        if let Some(k) = SESSIONS.iter().position(|(_, a, z)| (*a..*z).contains(&h)) {
            sess[k].0 += r;
            sess[k].1 += 1;
            sess[k].2 += usize::from(r > 0.0);
        }
    }
    let mean = |s: f64, n: usize| if n > 0 { s / n as f64 } else { 0.0 };
    let grid: Vec<Vec<f64>> = (0..7).map(|d| (0..24).map(|h| mean(sum[d][h], cnt[d][h])).collect()).collect();
    let hour_mean = (0..24).map(|h| mean((0..7).map(|d| sum[d][h]).sum(), (0..7).map(|d| cnt[d][h]).sum())).collect();
    let weekday_mean = (0..7).map(|d| mean(sum[d].iter().sum(), cnt[d].iter().sum())).collect();
    let sessions = SESSIONS
        .iter()
        .zip(&sess)
        .map(|((n, _, _), (s, c, up))| (n.to_string(), mean(*s, *c), if *c > 0 { *up as f64 / *c as f64 } else { 0.0 }))
        .collect();
    Seasonality { coin: coin.to_string(), grid, hour_mean, weekday_mean, sessions, hours: bars.len(), simulated: false }
}

pub async fn klines(http: &Http, symbol: &str, limit: usize, end_ms: Option<i64>) -> Result<Vec<Bar>> {
    let mut q = vec![("symbol", symbol.to_string()), ("interval", "1h".into()), ("limit", limit.min(1500).to_string())];
    if let Some(e) = end_ms {
        q.push(("endTime", e.to_string()));
    }
    parse_klines(&http.get(&format!("{BINANCE_FAPI}/fapi/v1/klines"), &q).await?)
}

/// The dashboard for `coins` (31 days of hourly bars each, six at a time).
pub async fn dashboard(http: &Http, coins: &[String]) -> Dashboard {
    use futures::StreamExt;
    let results: Vec<(String, Result<Vec<Bar>>)> = futures::stream::iter(coins.iter().cloned().map(|c| {
        let http = http.clone();
        async move {
            let (sym, _) = crate::venue::binance_perp(&c);
            let r = klines(&http, &sym, 744, None).await;
            (c, r)
        }
    }))
    .buffer_unordered(6)
    .collect()
    .await;
    let errors = results.iter().filter(|r| r.1.is_err()).count();
    let order: HashMap<&String, usize> = coins.iter().enumerate().map(|(i, c)| (c, i)).collect();
    let mut rows: Vec<Row> = results.iter().filter_map(|(c, r)| row_from_bars(c, r.as_ref().ok()?)).collect();
    rows.sort_by_key(|r| order.get(&r.coin).copied().unwrap_or(usize::MAX));
    Dashboard { rows, updated: chrono::Utc::now().timestamp(), simulated: false, errors }
}

/// About 125 days of hourly bars for one coin's seasonality.
pub async fn seasonality_for(http: &Http, coin: &str) -> Result<Seasonality> {
    let (sym, _) = crate::venue::binance_perp(coin);
    let recent = klines(http, &sym, 1500, None).await?;
    let first = recent.first().map(|b| b.ts * 1000 - 1);
    let mut older = match first {
        Some(e) => klines(http, &sym, 1500, Some(e)).await.unwrap_or_default(),
        None => Vec::new(),
    };
    older.extend(recent);
    Ok(seasonality(coin, &older))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bars(n: usize, step: f64) -> Vec<Bar> {
        (0..n)
            .map(|i| {
                let o = 100.0 + i as f64 * step;
                Bar { ts: 1_760_918_400 + i as i64 * 3600, open: o, close: o + step, volume: 1000.0, taker_buy: 600.0 }
            })
            .collect()
    }

    #[test]
    fn klines_and_returns() {
        let k = parse_klines(r#"[[1760918400000,"100.0","101","99","100.5","10",1760921999999,"1005.0",50,"6","603.0","0"],
                                 [1760922000000,"100.5","102","100","101.5","12",1760925599999,"1218.0",60,"5","500.0","0"]]"#).unwrap();
        assert_eq!(k.len(), 2);
        assert_eq!((k[1].ts, k[1].close, k[1].volume, k[1].taker_buy), (1_760_922_000, 101.5, 1218.0, 500.0));
        assert!((k[0].delta() - 201.0).abs() < 1e-9);
        let r = row_from_bars("BTC", &bars(800, 0.1)).unwrap();
        assert!((r.returns[0].unwrap() - 0.1 / (100.0 + 799.0 * 0.1)).abs() < 1e-12, "1h");
        assert!(r.returns[4].unwrap() > r.returns[3].unwrap(), "30d > 7d on a steady climb");
        assert_eq!(r.cvd_24h, 24.0 * 200.0);
        assert_eq!(r.cvd_spark.len(), 24);
        assert_eq!(row_from_bars("X", &bars(3, 1.0)).unwrap().returns[2], None, "not enough history");
        let d =
            Dashboard { rows: vec![r, Row { returns: [Some(-0.1); 5], ..Default::default() }], ..Default::default() };
        assert_eq!(d.breadth(2).0 + d.breadth(2).1, 2);
    }

    #[test]
    fn seasonality_grid() {
        // Rising bars on Mondays 14:00 UTC only.
        let mut b = Vec::new();
        for w in 0..10 {
            for h in 0..168 {
                let ts = 1_760_918_400 + w * 7 * 86_400 + h * 3600; // 2025-10-20, a Monday, 00:00 UTC
                let up = h == 14;
                b.push(Bar { ts, open: 100.0, close: if up { 101.0 } else { 100.0 }, volume: 1.0, taker_buy: 0.5 });
            }
        }
        let s = seasonality("BTC", &b);
        assert!((s.grid[0][14] - 0.01).abs() < 1e-12, "{:?}", s.grid[0]);
        assert_eq!(s.grid[1][14], 0.0);
        assert!(s.hour_mean[14] > 0.0 && s.weekday_mean[0] > 0.0);
        assert_eq!(s.sessions[2].0, "US");
        assert!(s.sessions[2].1 > 0.0 && s.sessions[0].1 == 0.0);
    }
}
