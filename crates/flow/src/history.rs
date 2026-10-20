//! History for one coin's derivatives: open interest, funding and the long/short ratio, from
//! Binance USDⓈ-M public data (the futures-data endpoints keep the last 30 days).

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::perps::BINANCE_FAPI;
use crate::{Http, int, num};

/// `(unix_secs, value)` points, oldest first.
pub type Series = Vec<(i64, f64)>;

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct CoinHistory {
    pub coin: String,
    /// Open interest value in USD.
    pub oi_usd: Series,
    /// Funding per interval as a fraction, at each funding time.
    pub funding: Series,
    /// Global long/short account ratio.
    pub long_short: Series,
    /// Taker buy volume / sell volume.
    pub taker_ratio: Series,
    pub errors: Vec<String>,
    pub simulated: bool,
}

impl CoinHistory {
    /// Open interest change over the series' last `secs` seconds, as a fraction.
    pub fn oi_change(&self, secs: i64) -> Option<f64> {
        let &(t1, v1) = self.oi_usd.last()?;
        let &(_, v0) = self.oi_usd.iter().find(|(t, _)| *t >= t1 - secs)?;
        (v0 > 0.0).then(|| v1 / v0 - 1.0)
    }
}

fn series(json: &str, ts_key: &str, val_key: &str) -> Result<Series> {
    let rows: Vec<Value> = serde_json::from_str(json)?;
    let mut s: Series = rows.iter().filter_map(|r| Some((int(&r[ts_key])? / 1000, num(&r[val_key])?))).collect();
    s.sort_by_key(|p| p.0);
    s.dedup_by_key(|p| p.0);
    Ok(s)
}

/// `/futures/data/openInterestHist`: `sumOpenInterestValue` (USD) by `timestamp` (ms).
pub fn parse_oi_hist(json: &str) -> Result<Series> {
    series(json, "timestamp", "sumOpenInterestValue")
}

/// `/fapi/v1/fundingRate`: `fundingRate` by `fundingTime` (ms).
pub fn parse_funding_hist(json: &str) -> Result<Series> {
    series(json, "fundingTime", "fundingRate")
}

/// `/futures/data/globalLongShortAccountRatio`: `longShortRatio` by `timestamp` (ms).
pub fn parse_long_short_hist(json: &str) -> Result<Series> {
    series(json, "timestamp", "longShortRatio")
}

/// `/futures/data/takerlongshortRatio`: `buySellRatio` by `timestamp` (ms).
pub fn parse_taker_hist(json: &str) -> Result<Series> {
    series(json, "timestamp", "buySellRatio")
}

/// Fetch a coin's history: hourly open interest, long/short and taker ratios for the last
/// week, and the last 90 fundings.
pub async fn fetch(http: &Http, coin: &str) -> CoinHistory {
    let (sym, _) = crate::venue::binance_perp(coin);
    let hourly = |s: &str| vec![("symbol", s.to_string()), ("period", "1h".to_string()), ("limit", "168".to_string())];
    let (oi, fr, ls, tk) = tokio::join!(
        http.get(&format!("{BINANCE_FAPI}/futures/data/openInterestHist"), &hourly(&sym)),
        http.get(&format!("{BINANCE_FAPI}/fapi/v1/fundingRate"), &[("symbol", sym.clone()), ("limit", "90".into())]),
        http.get(&format!("{BINANCE_FAPI}/futures/data/globalLongShortAccountRatio"), &hourly(&sym)),
        http.get(&format!("{BINANCE_FAPI}/futures/data/takerlongshortRatio"), &hourly(&sym)),
    );
    let mut h = CoinHistory { coin: coin.to_ascii_uppercase(), ..Default::default() };
    let mut take = |r: Result<String>, parse: fn(&str) -> Result<Series>, what: &str| match r.and_then(|j| parse(&j)) {
        Ok(s) => s,
        Err(e) => {
            h.errors.push(format!("{what}: {e:#}"));
            Vec::new()
        }
    };
    let oi_usd = take(oi, parse_oi_hist, "open interest");
    let funding = take(fr, parse_funding_hist, "funding");
    let long_short = take(ls, parse_long_short_hist, "long/short");
    let taker_ratio = take(tk, parse_taker_hist, "taker ratio");
    CoinHistory { oi_usd, funding, long_short, taker_ratio, ..h }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_sorts() {
        let oi = parse_oi_hist(
            r#"[{"symbol":"BTCUSDT","sumOpenInterest":"81000.1","sumOpenInterestValue":"5400000000.5","timestamp":1760936400000},
                {"symbol":"BTCUSDT","sumOpenInterest":"80000.1","sumOpenInterestValue":"5000000000","timestamp":1760850000000}]"#,
        )
        .unwrap();
        assert_eq!(oi, vec![(1_760_850_000, 5e9), (1_760_936_400, 5_400_000_000.5)]);
        let h = CoinHistory { oi_usd: oi, ..Default::default() };
        assert!((h.oi_change(86_400).unwrap() - 0.0800000001).abs() < 1e-9);
        let f = parse_funding_hist(
            r#"[{"symbol":"BTCUSDT","fundingTime":1760918400000,"fundingRate":"0.00010000","markPrice":"66800.1"},
                {"symbol":"BTCUSDT","fundingTime":1760947200001,"fundingRate":"-0.00002100","markPrice":"67000.0"}]"#,
        )
        .unwrap();
        assert_eq!(f[1], (1_760_947_200, -0.000021));
        let t = parse_taker_hist(
            r#"[{"buySellRatio":"1.0312","sellVol":"100","buyVol":"103.12","timestamp":1760936400000}]"#,
        )
        .unwrap();
        assert_eq!(t, vec![(1_760_936_400, 1.0312)]);
    }
}
