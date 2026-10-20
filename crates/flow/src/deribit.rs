//! Deribit: the dated futures term structure (premium over the index, annualised) and the
//! DVOL implied volatility index, both public.

use anyhow::{Context, Result};
use chrono::{NaiveDate, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::perps::DERIBIT;
use crate::{Http, int, num};

/// One point of the futures curve.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TermPoint {
    pub instrument: String,
    /// Expiry (Unix seconds); `None` for the perpetual.
    pub expiry: Option<i64>,
    pub mark: f64,
    pub index: f64,
    pub oi_usd: f64,
    pub volume_usd: f64,
}

impl TermPoint {
    /// Premium over the index (mark / index − 1).
    pub fn basis(&self) -> f64 {
        if self.index > 0.0 { self.mark / self.index - 1.0 } else { 0.0 }
    }

    /// Basis annualised to the expiry (simple), as seen at `now` (Unix seconds).
    pub fn annualised(&self, now: i64) -> Option<f64> {
        let days = (self.expiry? - now) as f64 / 86_400.0;
        (days > 0.5).then(|| self.basis() * 365.0 / days)
    }

    pub fn days_to_expiry(&self, now: i64) -> Option<f64> {
        Some((self.expiry? - now) as f64 / 86_400.0)
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct TermStructure {
    pub currency: String,
    /// Perpetual first, then dated futures by expiry.
    pub points: Vec<TermPoint>,
    /// DVOL candles `(unix_secs, close)`, hourly.
    pub dvol: Vec<(i64, f64)>,
    pub error: Option<String>,
    pub simulated: bool,
}

/// `BTC-25DEC26` -> 25 Dec 2026 08:00 UTC (Deribit's expiry time).
pub fn parse_expiry(instrument: &str) -> Option<i64> {
    let date = instrument.rsplit('-').next()?;
    let split = date.find(|c: char| c.is_ascii_alphabetic())?;
    let (day, rest) = date.split_at(split);
    let (mon, yy) = rest.split_at(rest.len().checked_sub(2)?);
    let month = ["JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC"]
        .iter()
        .position(|m| *m == mon)? as u32
        + 1;
    let d = NaiveDate::from_ymd_opt(2000 + yy.parse::<i32>().ok()?, month, day.parse().ok()?)?;
    Some(Utc.from_utc_datetime(&d.and_hms_opt(8, 0, 0)?).timestamp())
}

/// `get_book_summary_by_currency?kind=future`: the inverse perpetual and every dated future
/// (USDC-margined linear contracts, with `_` in the name, are skipped).
pub fn parse_term_structure(json: &str, currency: &str) -> Result<TermStructure> {
    let v: Value = serde_json::from_str(json)?;
    let rows = v["result"].as_array().context("Deribit: no result")?;
    let mut points: Vec<TermPoint> = rows
        .iter()
        .filter_map(|r| {
            let name = r["instrument_name"].as_str()?;
            if name.contains('_') {
                return None;
            }
            let expiry = if name.ends_with("-PERPETUAL") { None } else { Some(parse_expiry(name)?) };
            Some(TermPoint {
                instrument: name.into(),
                expiry,
                mark: num(&r["mark_price"])?,
                index: num(&r["estimated_delivery_price"])?,
                oi_usd: num(&r["open_interest"]).unwrap_or(0.0),
                volume_usd: num(&r["volume_usd"]).unwrap_or(0.0),
            })
        })
        .collect();
    points.sort_by_key(|p| p.expiry.unwrap_or(i64::MIN));
    Ok(TermStructure { currency: currency.to_ascii_uppercase(), points, ..Default::default() })
}

/// `get_volatility_index_data`: `{"result":{"data":[[ts_ms, open, high, low, close], …]}}`.
pub fn parse_dvol(json: &str) -> Result<Vec<(i64, f64)>> {
    let v: Value = serde_json::from_str(json)?;
    let rows = v["result"]["data"].as_array().context("Deribit DVOL: no data")?;
    let mut out: Vec<(i64, f64)> =
        rows.iter().filter_map(|r| Some((int(r.get(0)?)? / 1000, num(r.get(4)?)?))).collect();
    out.sort_by_key(|p| p.0);
    Ok(out)
}

/// Currencies with dated futures and a DVOL index on Deribit.
pub fn supported(coin: &str) -> bool {
    matches!(coin.to_ascii_uppercase().as_str(), "BTC" | "ETH")
}

pub async fn fetch(http: &Http, currency: &str) -> TermStructure {
    let ccy = currency.to_ascii_uppercase();
    let now = Utc::now().timestamp();
    let (book, dvol) = tokio::join!(
        http.get(
            &format!("{DERIBIT}/public/get_book_summary_by_currency"),
            &[("currency", ccy.clone()), ("kind", "future".into())]
        ),
        http.get(
            &format!("{DERIBIT}/public/get_volatility_index_data"),
            &[
                ("currency", ccy.clone()),
                ("start_timestamp", ((now - 7 * 86_400) * 1000).to_string()),
                ("end_timestamp", (now * 1000).to_string()),
                ("resolution", "3600".into()),
            ],
        ),
    );
    let mut ts = match book.and_then(|j| parse_term_structure(&j, &ccy)) {
        Ok(t) => t,
        Err(e) => TermStructure { currency: ccy, error: Some(format!("{e:#}")), ..Default::default() },
    };
    match dvol.and_then(|j| parse_dvol(&j)) {
        Ok(d) => ts.dvol = d,
        Err(e) => tracing::debug!("DVOL: {e:#}"),
    }
    ts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expiries() {
        let t = parse_expiry("BTC-25DEC26").unwrap();
        assert_eq!(chrono::DateTime::from_timestamp(t, 0).unwrap().to_rfc3339(), "2026-12-25T08:00:00+00:00");
        assert!(parse_expiry("BTC-5JAN27").is_some(), "single-digit day");
        assert_eq!(parse_expiry("BTC-PERPETUAL"), None);
        assert_eq!(parse_expiry("BTC-31FOO26"), None);
    }

    #[test]
    fn term_structure_from_fixture() {
        let j = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/deribit_book_summary_btc.json"
        ))
        .unwrap();
        let ts = parse_term_structure(&j, "btc").unwrap();
        let names: Vec<&str> = ts.points.iter().map(|p| p.instrument.as_str()).collect();
        assert_eq!(names, ["BTC-PERPETUAL", "BTC-31OCT26", "BTC-25DEC26", "BTC-26JUN27"]);
        let now = Utc.with_ymd_and_hms(2026, 10, 20, 8, 0, 0).unwrap().timestamp();
        let dec = &ts.points[2];
        let ann = dec.annualised(now).unwrap();
        assert!((dec.basis() - (68_921.4 / 66_991.88 - 1.0)).abs() < 1e-12);
        assert!((ann - dec.basis() * 365.0 / 66.0).abs() < 1e-9, "{ann}");
        assert_eq!(ts.points[0].annualised(now), None, "perpetual has no expiry");
    }

    #[test]
    fn dvol_candles() {
        let d = parse_dvol(r#"{"jsonrpc":"2.0","result":{"data":[[1760940000000,48.1,48.9,47.8,48.5],[1760936400000,47.0,48.2,46.9,48.1]],"continuation":null}}"#).unwrap();
        assert_eq!(d, vec![(1_760_936_400, 48.1), (1_760_940_000, 48.5)]);
    }
}
