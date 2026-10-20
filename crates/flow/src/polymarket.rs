//! Prediction markets from Polymarket's public APIs: market listings from the Gamma API
//! (`gamma-api.polymarket.com/markets`), price history and order books from the CLOB API
//! (`clob.polymarket.com/prices-history`, `/book`). Prices are probabilities in [0, 1].

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{Http, int, num};

pub const GAMMA: &str = "https://gamma-api.polymarket.com";
pub const CLOB: &str = "https://clob.polymarket.com";

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Market {
    pub id: String,
    pub question: String,
    pub slug: String,
    /// The parent event's title, when the market is one of several (e.g. per-candidate).
    pub event: Option<String>,
    pub category: Option<String>,
    pub outcomes: Vec<String>,
    /// Price per outcome, aligned with `outcomes`.
    pub prices: Vec<f64>,
    /// CLOB token id per outcome.
    pub tokens: Vec<String>,
    pub volume: f64,
    pub volume_24h: f64,
    pub liquidity: f64,
    /// ISO date.
    pub end_date: Option<String>,
    pub best_bid: Option<f64>,
    pub best_ask: Option<f64>,
    /// Change of the first outcome's price over a day, in probability points.
    pub change_1d: Option<f64>,
}

impl Market {
    /// The first outcome ("Yes" on binary markets) and its price.
    pub fn lead(&self) -> Option<(&str, f64)> {
        Some((self.outcomes.first()?.as_str(), *self.prices.first()?))
    }

    pub fn url(&self) -> String {
        format!("https://polymarket.com/market/{}", self.slug)
    }

    pub fn matches(&self, q: &str) -> bool {
        let q = q.to_lowercase();
        q.split_whitespace().all(|w| {
            self.question.to_lowercase().contains(w)
                || self.event.as_deref().is_some_and(|e| e.to_lowercase().contains(w))
                || self.category.as_deref().is_some_and(|c| c.to_lowercase().contains(w))
        })
    }
}

/// Gamma encodes some arrays as JSON strings (`"[\"Yes\", \"No\"]"`); accept both forms.
fn str_list(v: &Value) -> Vec<String> {
    let parsed;
    let arr = match v {
        Value::String(s) => {
            parsed = serde_json::from_str::<Value>(s).unwrap_or(Value::Null);
            &parsed
        }
        other => other,
    };
    arr.as_array()
        .into_iter()
        .flatten()
        .filter_map(|x| match x {
            Value::String(s) => Some(s.clone()),
            Value::Number(n) => Some(n.to_string()),
            _ => None,
        })
        .collect()
}

pub fn parse_market(m: &Value) -> Option<Market> {
    let outcomes = str_list(&m["outcomes"]);
    let prices: Vec<f64> = str_list(&m["outcomePrices"]).iter().filter_map(|p| p.parse().ok()).collect();
    if outcomes.is_empty() || outcomes.len() != prices.len() {
        return None;
    }
    let event = m["events"].as_array().and_then(|e| e.first()).and_then(|e| e["title"].as_str()).map(str::to_string);
    Some(Market {
        id: m["id"].as_str().map(str::to_string).or_else(|| int(&m["id"]).map(|i| i.to_string()))?,
        question: m["question"].as_str()?.to_string(),
        slug: m["slug"].as_str().unwrap_or_default().to_string(),
        event: event.filter(|e| Some(e.as_str()) != m["question"].as_str()),
        category: m["category"].as_str().filter(|c| !c.is_empty()).map(str::to_string),
        tokens: str_list(&m["clobTokenIds"]),
        outcomes,
        prices,
        volume: num(&m["volumeNum"]).or_else(|| num(&m["volume"])).unwrap_or(0.0),
        volume_24h: num(&m["volume24hr"]).unwrap_or(0.0),
        liquidity: num(&m["liquidityNum"]).or_else(|| num(&m["liquidity"])).unwrap_or(0.0),
        end_date: m["endDate"].as_str().map(|d| d.chars().take(10).collect()),
        best_bid: num(&m["bestBid"]),
        best_ask: num(&m["bestAsk"]),
        change_1d: num(&m["oneDayPriceChange"]),
    })
}

/// `/markets?…`: an array of markets.
pub fn parse_markets(json: &str) -> Result<Vec<Market>> {
    let v: Value = serde_json::from_str(json).context("parsing Polymarket markets")?;
    Ok(v.as_array().context("markets: not an array")?.iter().filter_map(parse_market).collect())
}

/// `/prices-history`: `{"history":[{"t": unix_secs, "p": price}]}`.
pub fn parse_history(json: &str) -> Result<Vec<(i64, f64)>> {
    let v: Value = serde_json::from_str(json).context("parsing Polymarket price history")?;
    let mut h: Vec<(i64, f64)> =
        v["history"].as_array().into_iter().flatten().filter_map(|p| Some((int(&p["t"])?, num(&p["p"])?))).collect();
    h.sort_by_key(|p| p.0);
    Ok(h)
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Book {
    /// Best first.
    pub bids: Vec<(f64, f64)>,
    pub asks: Vec<(f64, f64)>,
}

/// `/book?token_id=`: `{"bids":[{"price","size"}],"asks":[…]}` (sorted worst to best by the
/// API; returned best first).
pub fn parse_book(json: &str) -> Result<Book> {
    let v: Value = serde_json::from_str(json).context("parsing Polymarket book")?;
    let side = |k: &str| -> Vec<(f64, f64)> {
        v[k].as_array().into_iter().flatten().filter_map(|l| Some((num(&l["price"])?, num(&l["size"])?))).collect()
    };
    let mut bids = side("bids");
    let mut asks = side("asks");
    bids.sort_by(|a, b| b.0.total_cmp(&a.0));
    asks.sort_by(|a, b| a.0.total_cmp(&b.0));
    Ok(Book { bids, asks })
}

/// One market's detail: price history per outcome and the first outcome's book.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Detail {
    pub market_id: String,
    pub history: Vec<Vec<(i64, f64)>>,
    pub book: Option<Book>,
    pub error: Option<String>,
}

#[derive(Clone)]
pub struct Client {
    http: Http,
}

impl Client {
    pub fn new(http: Http) -> Self {
        Self { http }
    }

    /// Open markets by 24-hour volume.
    pub async fn top(&self, limit: usize) -> Result<Vec<Market>> {
        let q = [
            ("active", "true".to_string()),
            ("closed", "false".into()),
            ("archived", "false".into()),
            ("order", "volume24hr".into()),
            ("ascending", "false".into()),
            ("limit", limit.min(500).to_string()),
        ];
        parse_markets(&self.http.get(&format!("{GAMMA}/markets"), &q).await?)
    }

    pub async fn detail(&self, m: &Market) -> Detail {
        let mut d = Detail { market_id: m.id.clone(), ..Default::default() };
        let hist = m.tokens.iter().take(4).map(|t| {
            let q = [("market", t.clone()), ("interval", "1w".to_string()), ("fidelity", "60".into())];
            async move { self.http.get(&format!("{CLOB}/prices-history"), &q).await.and_then(|j| parse_history(&j)) }
        });
        let book = async {
            let t = m.tokens.first()?.clone();
            Some(self.http.get(&format!("{CLOB}/book"), &[("token_id", t)]).await.and_then(|j| parse_book(&j)))
        };
        let (hist, book) = tokio::join!(futures::future::join_all(hist), book);
        for h in hist {
            match h {
                Ok(h) => d.history.push(h),
                Err(e) => {
                    d.history.push(Vec::new());
                    d.error.get_or_insert(format!("{e:#}"));
                }
            }
        }
        match book {
            Some(Ok(b)) => d.book = Some(b),
            Some(Err(e)) => {
                d.error.get_or_insert(format!("{e:#}"));
            }
            None => {}
        }
        d
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markets_with_string_encoded_arrays() {
        let j = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/polymarket_markets.json"))
            .unwrap();
        let ms = parse_markets(&j).unwrap();
        assert_eq!(ms.len(), 2, "market with mismatched prices skipped");
        let m = &ms[0];
        assert_eq!(m.outcomes, ["Yes", "No"]);
        assert_eq!(m.prices, [0.62, 0.38]);
        assert_eq!(m.tokens.len(), 2);
        assert_eq!(m.lead(), Some(("Yes", 0.62)));
        assert_eq!(m.volume_24h, 1_234_567.89);
        assert_eq!(m.end_date.as_deref(), Some("2026-12-31"));
        assert_eq!(m.event.as_deref(), Some("Fed decisions 2026"));
        assert!(m.matches("fed CUT"));
        assert!(!m.matches("bitcoin"));
        assert_eq!(ms[1].outcomes.len(), 3, "multi-outcome market");
        assert_eq!(ms[1].event, None, "event title equal to the question is dropped");
    }

    #[test]
    fn history_and_book() {
        let h = parse_history(r#"{"history":[{"t":1760940000,"p":0.61},{"t":1760936400,"p":0.6}]}"#).unwrap();
        assert_eq!(h, vec![(1_760_936_400, 0.6), (1_760_940_000, 0.61)]);
        let b = parse_book(r#"{"market":"0x1","asset_id":"1","bids":[{"price":"0.58","size":"100"},{"price":"0.61","size":"250.5"}],"asks":[{"price":"0.66","size":"10"},{"price":"0.63","size":"40"}],"hash":"x","timestamp":"1"}"#).unwrap();
        assert_eq!(b.bids[0], (0.61, 250.5));
        assert_eq!(b.asks[0], (0.63, 40.0));
    }
}
