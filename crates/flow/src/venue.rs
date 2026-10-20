//! Venues and symbol normalisation: every venue names the same perpetual differently
//! (`1000PEPEUSDT`, `SHIB1000USDT`, `kPEPE`, `PEPE-USDT-SWAP`), and some quote prices and
//! sizes per 1,000 coins. [`Contract`] maps a venue symbol back to the coin and multiplier.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default, Serialize, Deserialize)]
pub enum Exchange {
    #[default]
    Binance,
    Bybit,
    Okx,
    Hyperliquid,
    Deribit,
}

impl Exchange {
    /// The venues on the derivatives board, in column order.
    pub const BOARD: [Exchange; 4] = [Exchange::Binance, Exchange::Bybit, Exchange::Okx, Exchange::Hyperliquid];

    pub fn label(self) -> &'static str {
        match self {
            Exchange::Binance => "Binance",
            Exchange::Bybit => "Bybit",
            Exchange::Okx => "OKX",
            Exchange::Hyperliquid => "Hyperliquid",
            Exchange::Deribit => "Deribit",
        }
    }

    /// Three-letter column code.
    pub fn short(self) -> &'static str {
        match self {
            Exchange::Binance => "BIN",
            Exchange::Bybit => "BYB",
            Exchange::Okx => "OKX",
            Exchange::Hyperliquid => "HL",
            Exchange::Deribit => "DRB",
        }
    }

    pub fn parse(s: &str) -> Option<Exchange> {
        Some(match s.to_ascii_uppercase().as_str() {
            "BIN" | "BINANCE" | "BN" => Exchange::Binance,
            "BYB" | "BYBIT" | "BB" => Exchange::Bybit,
            "OKX" | "OK" => Exchange::Okx,
            "HL" | "HYPE" | "HYPERLIQUID" => Exchange::Hyperliquid,
            "DRB" | "DERIBIT" | "DB" => Exchange::Deribit,
            _ => return None,
        })
    }
}

/// A venue contract resolved to its coin: `price_per_coin = venue_price / mult`.
#[derive(Debug, Clone, PartialEq)]
pub struct Contract {
    pub coin: String,
    /// Coins per quoted unit (1000 for `1000PEPEUSDT` and `kPEPE`).
    pub mult: f64,
}

impl Contract {
    fn new(coin: &str, mult: f64) -> Option<Contract> {
        let ok = !coin.is_empty() && coin.chars().all(|c| c.is_ascii_alphanumeric());
        ok.then(|| Contract { coin: coin.to_ascii_uppercase(), mult })
    }

    /// Binance or Bybit linear USDT perpetual (`BTCUSDT`, `1000PEPEUSDT`, `SHIB1000USDT`).
    /// Dated contracts (`BTCUSDT_251226`) and non-USDT margins are not perpetuals here.
    pub fn from_usdt_perp(symbol: &str) -> Option<Contract> {
        let base = symbol.strip_suffix("USDT")?;
        for (prefix, mult) in [("1000000", 1e6), ("1000", 1e3)] {
            if let Some(coin) = base.strip_prefix(prefix).filter(|c| c.chars().next().is_some_and(char::is_alphabetic))
            {
                return Contract::new(coin, mult);
            }
        }
        if let Some(coin) = base.strip_suffix("1000").filter(|c| !c.is_empty()) {
            return Contract::new(coin, 1e3);
        }
        Contract::new(base, 1.0)
    }

    /// OKX linear swap (`BTC-USDT-SWAP`); sizes come in contracts (see `ctVal`).
    pub fn from_okx_swap(inst_id: &str) -> Option<Contract> {
        let mut parts = inst_id.split('-');
        let (coin, quote, kind) = (parts.next()?, parts.next()?, parts.next()?);
        (quote == "USDT" && kind == "SWAP" && parts.next().is_none()).then_some(())?;
        Contract::new(coin, 1.0)
    }

    /// Hyperliquid perp name (`BTC`, `kPEPE`). HIP-3 builder markets (`xyz:TSLA`) and spot
    /// pairs (`@107`, `PURR/USDC`) are skipped.
    pub fn from_hyperliquid(name: &str) -> Option<Contract> {
        if name.contains([':', '@', '/']) {
            return None;
        }
        match name.strip_prefix('k') {
            Some(rest) if rest.chars().next().is_some_and(|c| c.is_ascii_uppercase()) => Contract::new(rest, 1e3),
            _ => Contract::new(name, 1.0),
        }
    }
}

/// Binance USDⓈ-M perpetual for a coin, with its multiplier (`PEPE` -> `1000PEPEUSDT`).
pub fn binance_perp(coin: &str) -> (String, f64) {
    let coin = coin.to_ascii_uppercase();
    match KILO.iter().find(|k| **k == coin) {
        Some(_) => (format!("1000{coin}USDT"), 1e3),
        None => (format!("{coin}USDT"), 1.0),
    }
}

/// Hyperliquid perp name for a coin (`PEPE` -> `kPEPE`).
pub fn hyperliquid_perp(coin: &str) -> (String, f64) {
    let coin = coin.to_ascii_uppercase();
    match KILO.iter().find(|k| **k == coin) {
        Some(_) => (format!("k{coin}"), 1e3),
        None => (coin, 1.0),
    }
}

/// Coins that Binance and Hyperliquid list per 1,000 units.
const KILO: [&str; 8] = ["PEPE", "SHIB", "BONK", "FLOKI", "LUNC", "SATS", "RATS", "CAT"];

/// The coin of a terminal market key (`BTCUSD Curncy`, `ETHUSDT`, `SOL`) for flow panes.
pub fn coin_of_key(key: &str) -> String {
    let t = key.split_whitespace().next().unwrap_or(key).to_ascii_uppercase();
    for suffix in ["USDT", "USDC", "USD", "PERP", "-PERP"] {
        if let Some(base) = t.strip_suffix(suffix).filter(|b| !b.is_empty()) {
            return base.trim_end_matches(['-', '/']).to_string();
        }
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(coin: &str, mult: f64) -> Option<Contract> {
        Some(Contract { coin: coin.into(), mult })
    }

    #[test]
    fn usdt_perps() {
        assert_eq!(Contract::from_usdt_perp("BTCUSDT"), c("BTC", 1.0));
        assert_eq!(Contract::from_usdt_perp("1000PEPEUSDT"), c("PEPE", 1e3));
        assert_eq!(Contract::from_usdt_perp("1000000MOGUSDT"), c("MOG", 1e6));
        assert_eq!(Contract::from_usdt_perp("SHIB1000USDT"), c("SHIB", 1e3));
        // A coin whose name starts with digits is not a multiplier.
        assert_eq!(Contract::from_usdt_perp("1INCHUSDT"), c("1INCH", 1.0));
        assert_eq!(Contract::from_usdt_perp("BTCUSDT_251226"), None);
        assert_eq!(Contract::from_usdt_perp("BTCUSD"), None);
        assert_eq!(Contract::from_usdt_perp("ETHBTC"), None);
    }

    #[test]
    fn okx_and_hyperliquid() {
        assert_eq!(Contract::from_okx_swap("BTC-USDT-SWAP"), c("BTC", 1.0));
        assert_eq!(Contract::from_okx_swap("BTC-USD-SWAP"), None);
        assert_eq!(Contract::from_okx_swap("BTC-USDT-251226"), None);
        assert_eq!(Contract::from_hyperliquid("kPEPE"), c("PEPE", 1e3));
        assert_eq!(Contract::from_hyperliquid("HYPE"), c("HYPE", 1.0));
        assert_eq!(Contract::from_hyperliquid("xyz:TSLA"), None);
        assert_eq!(Contract::from_hyperliquid("@107"), None);
    }

    #[test]
    fn symbols_for_coins() {
        assert_eq!(binance_perp("pepe"), ("1000PEPEUSDT".into(), 1e3));
        assert_eq!(binance_perp("BTC"), ("BTCUSDT".into(), 1.0));
        assert_eq!(hyperliquid_perp("PEPE"), ("kPEPE".into(), 1e3));
        assert_eq!(coin_of_key("BTCUSD Curncy"), "BTC");
        assert_eq!(coin_of_key("ETHUSDT"), "ETH");
        assert_eq!(coin_of_key("sol"), "SOL");
        assert_eq!(Exchange::parse("hl"), Some(Exchange::Hyperliquid));
    }
}
