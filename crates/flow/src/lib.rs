//! Crypto derivatives, order flow and prediction markets from free public venue APIs.
//!
//! - [`perps`]: funding, open interest, volume, premium and long/short ratio for every USDT
//!   perpetual on Binance, Bybit, OKX and Hyperliquid (plus Deribit's inverse perps),
//!   normalised per coin and aggregated into one board ([`perps::Board`]).
//! - [`history`]: open interest, funding and long/short history for one coin (Binance).
//! - [`deribit`]: the dated futures term structure (annualised basis) and the DVOL index.
//! - [`liq`]: live liquidations from the Binance, Bybit and OKX websockets ([`liq::Tape`]).
//! - [`orderflow`]: a live trade and depth stream per market (Binance USDⓈ-M futures or
//!   Hyperliquid) feeding an order book heatmap, footprint candles and CVD ([`orderflow::Flow`]).
//! - [`polymarket`]: prediction markets from Polymarket's Gamma and CLOB APIs.
//! - [`hyperliquid`]: on-chain trader intelligence: any wallet's positions and fills, the
//!   leaderboard and vaults, and positioning of the top traders (cohorts, liquidation levels).
//! - [`prints`]: large Hyperliquid trades with the wallets on both sides.
//! - [`options_flow`]: Deribit BTC/ETH option trades (premium, IV, blocks) and summaries.
//! - [`sample`]: deterministic offline data for all of the above (`TR_PROVIDER=sample`).
//!
//! No API keys are needed. Every parser is a pure function over the venue's JSON, tested
//! against fixtures in `tests/fixtures`; the network code is a thin layer on top.

pub mod deribit;
pub mod history;
pub mod hyperliquid;
pub mod liq;
pub mod options_flow;
pub mod orderflow;
pub mod perps;
pub mod polymarket;
pub mod prints;
pub mod sample;
pub mod venue;

pub use venue::Exchange;

use std::future::Future;
use std::time::Duration;

use anyhow::{Result, bail};
use serde_json::Value;

/// A number that venues send either as a JSON string or a JSON number.
pub(crate) fn num(v: &Value) -> Option<f64> {
    match v {
        Value::String(s) => s.trim().parse().ok().filter(|x: &f64| x.is_finite()),
        Value::Number(n) => n.as_f64().filter(|x| x.is_finite()),
        _ => None,
    }
}

/// An integer timestamp sent as a string or a number.
pub(crate) fn int(v: &Value) -> Option<i64> {
    match v {
        Value::String(s) => s.trim().parse().ok(),
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
        _ => None,
    }
}

/// Install the process-wide TLS crypto provider (idempotent).
pub fn ensure_tls() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
}

/// True when running fully offline on simulated data (`TR_PROVIDER=sample`).
pub fn offline() -> bool {
    std::env::var("TR_PROVIDER").is_ok_and(|v| v.eq_ignore_ascii_case("sample"))
}

/// A shared HTTP client for the public venue APIs.
#[derive(Clone)]
pub struct Http {
    client: reqwest::Client,
}

impl Http {
    pub fn new() -> Result<Self> {
        ensure_tls();
        let client = reqwest::Client::builder()
            .user_agent(concat!("terminal-rs/", env!("CARGO_PKG_VERSION")))
            .timeout(Duration::from_secs(15))
            .build()?;
        Ok(Self { client })
    }

    /// The request is built immediately, so the returned future borrows nothing (handy in
    /// `tokio::join!` with temporary URLs).
    pub fn get(&self, url: &str, query: &[(&str, String)]) -> impl Future<Output = Result<String>> + Send + 'static {
        Self::send(self.client.get(url).query(query), format!("GET {url}"))
    }

    pub fn post_json(&self, url: &str, body: &Value) -> impl Future<Output = Result<String>> + Send + 'static {
        Self::send(self.client.post(url).json(body), format!("POST {url}"))
    }

    async fn send(req: reqwest::RequestBuilder, what: String) -> Result<String> {
        let resp = req.send().await?;
        let status = resp.status();
        if !status.is_success() {
            bail!("{what}: HTTP {status}");
        }
        Ok(resp.text().await?)
    }
}

/// Round `x` up to a "nice" step (1, 2, 2.5 or 5 × 10ⁿ), for price buckets.
pub fn nice_step(x: f64) -> f64 {
    if !(x.is_finite() && x > 0.0) {
        return 1.0;
    }
    let mag = 10f64.powf(x.log10().floor());
    let f = x / mag;
    let n = [1.0, 2.0, 2.5, 5.0, 10.0].into_iter().find(|&s| f <= s + 1e-9).unwrap_or(10.0);
    n * mag
}

/// `1.23B`, `45.6M`, `789K` for dollar amounts.
pub fn fmt_usd(x: f64) -> String {
    let a = x.abs();
    let s = if a >= 1e9 {
        format!("{:.2}B", x / 1e9)
    } else if a >= 1e6 {
        format!("{:.1}M", x / 1e6)
    } else if a >= 1e3 {
        format!("{:.0}K", x / 1e3)
    } else {
        format!("{x:.0}")
    };
    if x < 0.0 { format!("-${}", s.trim_start_matches('-')) } else { format!("${s}") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nice_steps() {
        assert_eq!(nice_step(13.4), 20.0);
        assert_eq!(nice_step(2.2), 2.5);
        assert_eq!(nice_step(10.0), 10.0);
        assert_eq!(nice_step(0.0042), 0.005);
        assert_eq!(nice_step(1.9), 2.0);
        assert_eq!(nice_step(-1.0), 1.0);
    }

    #[test]
    fn numbers_from_strings_or_numbers() {
        assert_eq!(num(&Value::from("0.0001")), Some(0.0001));
        assert_eq!(num(&Value::from(2.5)), Some(2.5));
        assert_eq!(num(&Value::from("")), None);
        assert_eq!(num(&Value::from("NaN")), None);
        assert_eq!(int(&Value::from("1700000000000")), Some(1_700_000_000_000));
        assert_eq!(fmt_usd(1.5e9), "$1.50B");
        assert_eq!(fmt_usd(-2.3e6), "-$2.3M");
        assert_eq!(fmt_usd(12_345.0), "$12K");
    }
}
