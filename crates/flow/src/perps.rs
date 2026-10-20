//! Perpetual futures across venues: one [`PerpSnapshot`] per venue contract (price, mark,
//! index, funding, open interest, volume), merged per coin into a [`Board`].
//!
//! Sources (all public, bulk endpoints where the venue has one):
//!
//! | Venue | Endpoints |
//! |---|---|
//! | Binance USDⓈ-M | `/fapi/v1/premiumIndex`, `/fapi/v1/ticker/24hr`, `/fapi/v1/fundingInfo`, `/fapi/v1/openInterest` (per symbol), `/futures/data/globalLongShortAccountRatio` |
//! | Bybit | `/v5/market/tickers?category=linear` |
//! | OKX | `/api/v5/market/tickers`, `/api/v5/public/open-interest`, `/api/v5/public/mark-price`, `/api/v5/public/funding-rate`, `/api/v5/market/index-tickers` |
//! | Hyperliquid | `POST /info {"type":"metaAndAssetCtxs"}` |
//! | Deribit | `/api/v2/public/get_book_summary_by_currency` (BTC and ETH inverse perps) |

use std::collections::{BTreeMap, HashMap};

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::venue::{Contract, Exchange};
use crate::{Http, int, num};

pub const BINANCE_FAPI: &str = "https://fapi.binance.com";
pub const BYBIT: &str = "https://api.bybit.com";
pub const OKX: &str = "https://www.okx.com";
pub const HYPERLIQUID_INFO: &str = "https://api.hyperliquid.xyz/info";
pub const DERIBIT: &str = "https://www.deribit.com/api/v2";

/// One venue's perpetual for one coin. Prices are per coin (venue price / multiplier).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PerpSnapshot {
    pub exchange: Exchange,
    pub coin: String,
    /// The venue's own symbol (`1000PEPEUSDT`, `kPEPE`, `BTC-USDT-SWAP`).
    pub symbol: String,
    pub price: f64,
    pub mark: Option<f64>,
    pub index: Option<f64>,
    /// Funding rate for one interval, as a fraction (0.0001 = 0.01%).
    pub funding: Option<f64>,
    /// Hours between fundings (8 on most CEX contracts, 1 on Hyperliquid).
    pub funding_hours: f64,
    /// Next funding time (Unix ms).
    pub next_funding: Option<i64>,
    pub oi_usd: Option<f64>,
    pub volume_usd: Option<f64>,
    /// Change over 24 hours, as a fraction.
    pub change_24h: Option<f64>,
}

impl PerpSnapshot {
    fn new(exchange: Exchange, c: Contract, symbol: &str, venue_price: f64) -> PerpSnapshot {
        PerpSnapshot {
            exchange,
            coin: c.coin,
            symbol: symbol.into(),
            price: venue_price / c.mult,
            mark: None,
            index: None,
            funding: None,
            funding_hours: 8.0,
            next_funding: None,
            oi_usd: None,
            volume_usd: None,
            change_24h: None,
        }
    }

    /// Funding normalised to an 8-hour interval, so venues compare like for like.
    pub fn funding_8h(&self) -> Option<f64> {
        Some(self.funding? * 8.0 / self.funding_hours.max(1e-9))
    }

    /// Funding annualised (simple, not compounded).
    pub fn funding_apr(&self) -> Option<f64> {
        Some(self.funding? * 24.0 * 365.0 / self.funding_hours.max(1e-9))
    }

    /// Perp premium over the index (mark / index − 1).
    pub fn premium(&self) -> Option<f64> {
        let (m, i) = (self.mark?, self.index?);
        (i > 0.0).then(|| m / i - 1.0)
    }
}

fn parse_err(venue: &str) -> impl FnOnce(serde_json::Error) -> anyhow::Error + '_ {
    move |e| anyhow!("parsing {venue}: {e}")
}

// ---- Binance -------------------------------------------------------------------------

/// `/fapi/v1/ticker/24hr` (all symbols) joined with `/fapi/v1/premiumIndex` (all symbols) and
/// `/fapi/v1/fundingInfo` (only symbols whose interval differs from 8 hours).
pub fn parse_binance(ticker_24h: &str, premium: &str, funding_info: Option<&str>) -> Result<Vec<PerpSnapshot>> {
    let tickers: Vec<Value> = serde_json::from_str(ticker_24h).map_err(parse_err("Binance 24hr"))?;
    let prem: Vec<Value> = serde_json::from_str(premium).map_err(parse_err("Binance premiumIndex"))?;
    let prem: HashMap<&str, &Value> = prem.iter().filter_map(|p| Some((p["symbol"].as_str()?, p))).collect();
    let mut hours: HashMap<String, f64> = HashMap::new();
    if let Some(info) = funding_info {
        let rows: Vec<Value> = serde_json::from_str(info).map_err(parse_err("Binance fundingInfo"))?;
        for r in rows {
            if let (Some(s), Some(h)) = (r["symbol"].as_str(), num(&r["fundingIntervalHours"])) {
                hours.insert(s.to_string(), h);
            }
        }
    }
    let mut out = Vec::new();
    for t in &tickers {
        let Some(sym) = t["symbol"].as_str() else { continue };
        let Some(c) = Contract::from_usdt_perp(sym) else { continue };
        // Delisted contracts linger in the 24h ticker with no premium index entry.
        let Some(p) = prem.get(sym) else { continue };
        let Some(last) = num(&t["lastPrice"]).filter(|x| *x > 0.0) else { continue };
        let mult = c.mult;
        let mut s = PerpSnapshot::new(Exchange::Binance, c, sym, last);
        s.mark = num(&p["markPrice"]).map(|x| x / mult);
        s.index = num(&p["indexPrice"]).map(|x| x / mult);
        s.funding = num(&p["lastFundingRate"]);
        s.next_funding = int(&p["nextFundingTime"]).filter(|t| *t > 0);
        s.funding_hours = hours.get(sym).copied().unwrap_or(8.0);
        s.volume_usd = num(&t["quoteVolume"]);
        s.change_24h = num(&t["priceChangePercent"]).map(|x| x / 100.0);
        out.push(s);
    }
    Ok(out)
}

/// `/fapi/v1/openInterest?symbol=…`: open interest in contract units.
pub fn parse_binance_open_interest(json: &str) -> Result<f64> {
    let v: Value = serde_json::from_str(json).map_err(parse_err("Binance openInterest"))?;
    num(&v["openInterest"]).context("no openInterest")
}

/// `/futures/data/globalLongShortAccountRatio`: the latest long/short account ratio.
pub fn parse_binance_long_short(json: &str) -> Result<Option<f64>> {
    let rows: Vec<Value> = serde_json::from_str(json).map_err(parse_err("Binance long/short"))?;
    Ok(rows.iter().max_by_key(|r| int(&r["timestamp"]).unwrap_or(0)).and_then(|r| num(&r["longShortRatio"])))
}

// ---- Bybit ---------------------------------------------------------------------------

/// `/v5/market/tickers?category=linear`: everything in one call, including open interest
/// value and the funding interval.
pub fn parse_bybit(json: &str) -> Result<Vec<PerpSnapshot>> {
    let v: Value = serde_json::from_str(json).map_err(parse_err("Bybit tickers"))?;
    if v["retCode"].as_i64().unwrap_or(0) != 0 {
        return Err(anyhow!("Bybit: {}", v["retMsg"].as_str().unwrap_or("error")));
    }
    let mut out = Vec::new();
    for t in v["result"]["list"].as_array().into_iter().flatten() {
        let Some(sym) = t["symbol"].as_str() else { continue };
        let Some(c) = Contract::from_usdt_perp(sym) else { continue };
        let Some(last) = num(&t["lastPrice"]).filter(|x| *x > 0.0) else { continue };
        // Pre-market and settling contracts have no funding; skip them.
        let Some(funding) = num(&t["fundingRate"]) else { continue };
        let mult = c.mult;
        let mut s = PerpSnapshot::new(Exchange::Bybit, c, sym, last);
        s.mark = num(&t["markPrice"]).map(|x| x / mult);
        s.index = num(&t["indexPrice"]).map(|x| x / mult);
        s.funding = Some(funding);
        s.funding_hours = num(&t["fundingIntervalHour"]).filter(|h| *h > 0.0).unwrap_or(8.0);
        s.next_funding = int(&t["nextFundingTime"]).filter(|t| *t > 0);
        s.oi_usd = num(&t["openInterestValue"]);
        s.volume_usd = num(&t["turnover24h"]);
        s.change_24h = num(&t["price24hPcnt"]);
        out.push(s);
    }
    Ok(out)
}

// ---- OKX -----------------------------------------------------------------------------

fn okx_data<'a>(v: &'a Value, what: &str) -> Result<&'a Vec<Value>> {
    if v["code"].as_str().is_some_and(|c| c != "0") {
        return Err(anyhow!("OKX {what}: {}", v["msg"].as_str().unwrap_or("error")));
    }
    v["data"].as_array().with_context(|| format!("OKX {what}: no data"))
}

/// OKX swaps from `/market/tickers?instType=SWAP`, `/public/open-interest?instType=SWAP`,
/// `/public/mark-price?instType=SWAP`, `/public/funding-rate` (one or all instruments) and
/// `/market/index-tickers?quoteCcy=USDT`.
pub fn parse_okx(
    tickers: &str,
    open_interest: &str,
    mark: &str,
    funding: &str,
    index: &str,
) -> Result<Vec<PerpSnapshot>> {
    let parse = |s: &str, what: &str| -> Result<Value> {
        serde_json::from_str(s).map_err(|e| anyhow!("parsing OKX {what}: {e}"))
    };
    let (t, oi, mk, fr, ix) = (
        parse(tickers, "tickers")?,
        parse(open_interest, "open-interest")?,
        parse(mark, "mark-price")?,
        parse(funding, "funding-rate")?,
        parse(index, "index-tickers")?,
    );
    let by_inst = |rows: &Vec<Value>| -> HashMap<String, Value> {
        rows.iter().filter_map(|r| Some((r["instId"].as_str()?.to_string(), r.clone()))).collect()
    };
    let oi = by_inst(okx_data(&oi, "open-interest")?);
    let mk = by_inst(okx_data(&mk, "mark-price")?);
    let fr = by_inst(okx_data(&fr, "funding-rate").unwrap_or(&Vec::new()));
    let ix = by_inst(okx_data(&ix, "index-tickers").unwrap_or(&Vec::new()));
    let mut out = Vec::new();
    for r in okx_data(&t, "tickers")? {
        let Some(inst) = r["instId"].as_str() else { continue };
        let Some(c) = Contract::from_okx_swap(inst) else { continue };
        let Some(last) = num(&r["last"]).filter(|x| *x > 0.0) else { continue };
        let coin = c.coin.clone();
        let mut s = PerpSnapshot::new(Exchange::Okx, c, inst, last);
        s.mark = mk.get(inst).and_then(|m| num(&m["markPx"]));
        s.index = ix.get(&format!("{coin}-USDT")).and_then(|m| num(&m["idxPx"]));
        if let Some(f) = fr.get(inst) {
            s.funding = num(&f["fundingRate"]);
            let (ft, nt) = (int(&f["fundingTime"]), int(&f["nextFundingTime"]));
            s.next_funding = ft;
            if let (Some(a), Some(b)) = (ft, nt)
                && b > a
            {
                s.funding_hours = ((b - a) as f64 / 3_600_000.0).round().max(1.0);
            }
        }
        // `volCcy24h` is in coins for USDT swaps.
        s.volume_usd = num(&r["volCcy24h"]).map(|v| v * last);
        s.change_24h = num(&r["open24h"]).filter(|o| *o > 0.0).map(|o| last / o - 1.0);
        s.oi_usd = oi.get(inst).and_then(|o| num(&o["oiUsd"]).or_else(|| Some(num(&o["oiCcy"])? * last)));
        out.push(s);
    }
    Ok(out)
}

// ---- Hyperliquid ---------------------------------------------------------------------

/// `{"type":"metaAndAssetCtxs"}`: `[{universe:[{name,…}]}, [{funding, openInterest, markPx,
/// oraclePx, prevDayPx, dayNtlVlm, …}]]`, aligned by index. Funding is hourly; open interest
/// is in coins (per quoted unit for `k` markets).
pub fn parse_hyperliquid(json: &str) -> Result<Vec<PerpSnapshot>> {
    let v: Vec<Value> = serde_json::from_str(json).map_err(parse_err("Hyperliquid metaAndAssetCtxs"))?;
    let universe = v.first().and_then(|m| m["universe"].as_array()).context("Hyperliquid: no universe")?;
    let ctxs = v.get(1).and_then(Value::as_array).context("Hyperliquid: no asset contexts")?;
    let mut out = Vec::new();
    for (meta, ctx) in universe.iter().zip(ctxs) {
        if meta["isDelisted"].as_bool() == Some(true) {
            continue;
        }
        let Some(name) = meta["name"].as_str() else { continue };
        let Some(c) = Contract::from_hyperliquid(name) else { continue };
        let Some(mark) = num(&ctx["markPx"]).filter(|x| *x > 0.0) else { continue };
        let price = num(&ctx["midPx"]).unwrap_or(mark);
        let mult = c.mult;
        let mut s = PerpSnapshot::new(Exchange::Hyperliquid, c, name, price);
        s.mark = Some(mark / mult);
        s.index = num(&ctx["oraclePx"]).map(|x| x / mult);
        s.funding = num(&ctx["funding"]);
        s.funding_hours = 1.0;
        s.oi_usd = num(&ctx["openInterest"]).map(|oi| oi * mark);
        s.volume_usd = num(&ctx["dayNtlVlm"]);
        s.change_24h = num(&ctx["prevDayPx"]).filter(|p| *p > 0.0).map(|p| price / p - 1.0);
        out.push(s);
    }
    Ok(out)
}

// ---- Deribit -------------------------------------------------------------------------

/// The inverse perpetual from `get_book_summary_by_currency?kind=future` (`BTC-PERPETUAL`).
/// Open interest on inverse contracts is already in USD.
pub fn parse_deribit_perp(json: &str) -> Result<Option<PerpSnapshot>> {
    let v: Value = serde_json::from_str(json).map_err(parse_err("Deribit book summary"))?;
    let rows = v["result"].as_array().context("Deribit: no result")?;
    let Some(r) = rows
        .iter()
        .find(|r| r["instrument_name"].as_str().is_some_and(|n| n.ends_with("-PERPETUAL") && !n.contains('_')))
    else {
        return Ok(None);
    };
    let name = r["instrument_name"].as_str().unwrap_or_default();
    let coin = name.trim_end_matches("-PERPETUAL");
    let Some(last) = num(&r["last"]).or_else(|| num(&r["mark_price"])) else { return Ok(None) };
    let mut s = PerpSnapshot::new(Exchange::Deribit, Contract { coin: coin.into(), mult: 1.0 }, name, last);
    s.mark = num(&r["mark_price"]);
    s.index = num(&r["estimated_delivery_price"]);
    s.funding = num(&r["funding_8h"]);
    s.oi_usd = num(&r["open_interest"]);
    s.volume_usd = num(&r["volume_usd"]);
    s.change_24h = num(&r["price_change"]).map(|x| x / 100.0);
    Ok(Some(s))
}

// ---- Aggregation ---------------------------------------------------------------------

/// One coin across venues.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct CoinRow {
    pub coin: String,
    /// Price from the most liquid venue.
    pub price: f64,
    pub change_24h: Option<f64>,
    /// Open interest summed across venues.
    pub oi_usd: f64,
    /// Open interest change over 24 hours (Binance history), as a fraction.
    pub oi_change_24h: Option<f64>,
    pub volume_usd: f64,
    /// Open-interest-weighted funding, normalised to 8 hours.
    pub funding_8h: Option<f64>,
    /// Open-interest-weighted perp premium.
    pub premium: Option<f64>,
    /// Binance global long/short account ratio.
    pub long_short: Option<f64>,
    pub venues: BTreeMap<Exchange, PerpSnapshot>,
}

impl CoinRow {
    pub fn venue(&self, e: Exchange) -> Option<&PerpSnapshot> {
        self.venues.get(&e)
    }

    /// Spread between the highest and lowest 8h funding across venues.
    pub fn funding_spread(&self) -> Option<f64> {
        let f: Vec<f64> = self.venues.values().filter_map(PerpSnapshot::funding_8h).collect();
        let max = f.iter().copied().reduce(f64::max)?;
        let min = f.iter().copied().reduce(f64::min)?;
        (f.len() > 1).then_some(max - min)
    }
}

/// The derivatives board: every coin listed on at least one venue, largest open interest
/// first, plus per-venue errors (a venue being down never blanks the board).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Board {
    pub rows: Vec<CoinRow>,
    pub errors: Vec<(Exchange, String)>,
    /// Unix seconds.
    pub updated: i64,
    pub simulated: bool,
}

impl Board {
    pub fn total_oi(&self) -> f64 {
        self.rows.iter().map(|r| r.oi_usd).sum()
    }

    pub fn total_volume(&self) -> f64 {
        self.rows.iter().map(|r| r.volume_usd).sum()
    }

    pub fn row(&self, coin: &str) -> Option<&CoinRow> {
        self.rows.iter().find(|r| r.coin.eq_ignore_ascii_case(coin))
    }

    /// Open interest per venue across all coins.
    pub fn oi_by_venue(&self) -> BTreeMap<Exchange, f64> {
        let mut m = BTreeMap::new();
        for r in &self.rows {
            for (e, s) in &r.venues {
                *m.entry(*e).or_insert(0.0) += s.oi_usd.unwrap_or(0.0);
            }
        }
        m
    }
}

/// Merge venue snapshots into per-coin rows sorted by aggregate open interest.
pub fn aggregate(snaps: Vec<PerpSnapshot>) -> Vec<CoinRow> {
    let mut by_coin: BTreeMap<String, CoinRow> = BTreeMap::new();
    for s in snaps {
        let row =
            by_coin.entry(s.coin.clone()).or_insert_with(|| CoinRow { coin: s.coin.clone(), ..Default::default() });
        // A venue listing the same coin twice (e.g. a relisting) keeps the more liquid one.
        let keep_old = row.venues.get(&s.exchange).is_some_and(|o| o.volume_usd >= s.volume_usd);
        if !keep_old {
            row.venues.insert(s.exchange, s);
        }
    }
    let mut rows: Vec<CoinRow> = by_coin
        .into_values()
        .map(|mut r| {
            let vs: Vec<&PerpSnapshot> = r.venues.values().collect();
            r.oi_usd = vs.iter().filter_map(|s| s.oi_usd).sum();
            r.volume_usd = vs.iter().filter_map(|s| s.volume_usd).sum();
            let lead = vs.iter().max_by(|a, b| a.volume_usd.unwrap_or(0.0).total_cmp(&b.volume_usd.unwrap_or(0.0)));
            if let Some(lead) = lead {
                r.price = lead.price;
                r.change_24h = lead.change_24h;
            }
            r.funding_8h = weighted(&vs, PerpSnapshot::funding_8h);
            r.premium = weighted(&vs, PerpSnapshot::premium);
            r
        })
        .collect();
    rows.sort_by(|a, b| b.oi_usd.total_cmp(&a.oi_usd).then(b.volume_usd.total_cmp(&a.volume_usd)));
    rows
}

/// Average of `f` over venues weighted by open interest (equal weights when no venue reports
/// open interest).
fn weighted(vs: &[&PerpSnapshot], f: impl Fn(&PerpSnapshot) -> Option<f64>) -> Option<f64> {
    let pts: Vec<(f64, f64)> = vs.iter().filter_map(|s| Some((f(s)?, s.oi_usd.unwrap_or(0.0)))).collect();
    if pts.is_empty() {
        return None;
    }
    let w: f64 = pts.iter().map(|p| p.1).sum();
    Some(if w > 0.0 {
        pts.iter().map(|(x, wi)| x * wi).sum::<f64>() / w
    } else {
        pts.iter().map(|p| p.0).sum::<f64>() / pts.len() as f64
    })
}

// ---- Network -------------------------------------------------------------------------

/// Fetches the board from every venue concurrently.
#[derive(Clone)]
pub struct PerpsClient {
    http: Http,
}

impl PerpsClient {
    pub fn new() -> Result<Self> {
        Ok(Self { http: Http::new()? })
    }

    pub fn http(&self) -> &Http {
        &self.http
    }

    async fn binance(&self, detail: usize) -> Result<Vec<PerpSnapshot>> {
        let (t, p, f) = tokio::join!(
            self.http.get(&format!("{BINANCE_FAPI}/fapi/v1/ticker/24hr"), &[]),
            self.http.get(&format!("{BINANCE_FAPI}/fapi/v1/premiumIndex"), &[]),
            self.http.get(&format!("{BINANCE_FAPI}/fapi/v1/fundingInfo"), &[]),
        );
        let mut snaps = parse_binance(&t?, &p?, f.ok().as_deref())?;
        // Open interest has no bulk endpoint: fetch it for the most traded contracts.
        snaps.sort_by(|a, b| b.volume_usd.unwrap_or(0.0).total_cmp(&a.volume_usd.unwrap_or(0.0)));
        let futs = snaps.iter().take(detail).map(|s| {
            let sym = s.symbol.clone();
            async move {
                let j = self.http.get(&format!("{BINANCE_FAPI}/fapi/v1/openInterest"), &[("symbol", sym)]).await?;
                parse_binance_open_interest(&j)
            }
        });
        let ois = futures::future::join_all(futs).await;
        for (s, oi) in snaps.iter_mut().zip(ois) {
            if let Ok(oi) = oi {
                // Contract units × venue mark = USD (the multiplier cancels).
                let mult = Contract::from_usdt_perp(&s.symbol).map_or(1.0, |c| c.mult);
                s.oi_usd = s.mark.map(|m| oi * m * mult);
            }
        }
        Ok(snaps)
    }

    async fn bybit(&self) -> Result<Vec<PerpSnapshot>> {
        parse_bybit(&self.http.get(&format!("{BYBIT}/v5/market/tickers"), &[("category", "linear".into())]).await?)
    }

    async fn okx(&self) -> Result<Vec<PerpSnapshot>> {
        let swap = || vec![("instType", "SWAP".to_string())];
        let (t, oi, mk, fr, ix) = tokio::join!(
            self.http.get(&format!("{OKX}/api/v5/market/tickers"), &swap()),
            self.http.get(&format!("{OKX}/api/v5/public/open-interest"), &swap()),
            self.http.get(&format!("{OKX}/api/v5/public/mark-price"), &swap()),
            self.http.get(&format!("{OKX}/api/v5/public/funding-rate"), &[("instId", "ANY".into())]),
            self.http.get(&format!("{OKX}/api/v5/market/index-tickers"), &[("quoteCcy", "USDT".into())]),
        );
        let empty = r#"{"code":"0","data":[]}"#.to_string();
        parse_okx(&t?, &oi?, &mk?, &fr.unwrap_or_else(|_| empty.clone()), &ix.unwrap_or(empty))
    }

    async fn hyperliquid(&self) -> Result<Vec<PerpSnapshot>> {
        parse_hyperliquid(
            &self.http.post_json(HYPERLIQUID_INFO, &serde_json::json!({"type": "metaAndAssetCtxs"})).await?,
        )
    }

    async fn deribit(&self) -> Result<Vec<PerpSnapshot>> {
        let mut out = Vec::new();
        for ccy in ["BTC", "ETH"] {
            let q = [("currency", ccy.to_string()), ("kind", "future".into())];
            let j = self.http.get(&format!("{DERIBIT}/public/get_book_summary_by_currency"), &q).await?;
            out.extend(parse_deribit_perp(&j)?);
        }
        Ok(out)
    }

    /// Long/short ratios for the given coins (Binance global accounts, 5-minute period).
    pub async fn long_short(&self, coins: &[String]) -> HashMap<String, f64> {
        let futs = coins.iter().map(|coin| async move {
            let (sym, _) = crate::venue::binance_perp(coin);
            let q = [("symbol", sym), ("period", "5m".into()), ("limit", "1".into())];
            let j =
                self.http.get(&format!("{BINANCE_FAPI}/futures/data/globalLongShortAccountRatio"), &q).await.ok()?;
            Some((coin.clone(), parse_binance_long_short(&j).ok()??))
        });
        futures::future::join_all(futs).await.into_iter().flatten().collect()
    }

    /// The full board. `detail` bounds the per-symbol requests (Binance open interest and
    /// long/short ratios) to the most traded contracts.
    pub async fn board(&self, detail: usize) -> Board {
        let (b, y, o, h, d) =
            tokio::join!(self.binance(detail), self.bybit(), self.okx(), self.hyperliquid(), self.deribit());
        let mut snaps = Vec::new();
        let mut errors = Vec::new();
        for (e, r) in [
            (Exchange::Binance, b),
            (Exchange::Bybit, y),
            (Exchange::Okx, o),
            (Exchange::Hyperliquid, h),
            (Exchange::Deribit, d),
        ] {
            match r {
                Ok(s) => snaps.extend(s),
                Err(err) => {
                    tracing::warn!("{} derivatives: {err:#}", e.label());
                    errors.push((e, format!("{err:#}")));
                }
            }
        }
        let mut rows = aggregate(snaps);
        let top: Vec<String> = rows.iter().take(detail).map(|r| r.coin.clone()).collect();
        let ls = self.long_short(&top).await;
        for r in &mut rows {
            r.long_short = ls.get(&r.coin).copied();
        }
        Board { rows, errors, updated: chrono::Utc::now().timestamp(), simulated: false }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fx(name: &str) -> String {
        std::fs::read_to_string(format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
    }

    #[test]
    fn binance_join() {
        let s = parse_binance(
            &fx("binance_ticker_24hr.json"),
            &fx("binance_premium_index.json"),
            Some(&fx("binance_funding_info.json")),
        )
        .unwrap();
        assert_eq!(s.len(), 3, "dated and delisted contracts are skipped");
        let btc = s.iter().find(|x| x.coin == "BTC").unwrap();
        assert_eq!(btc.symbol, "BTCUSDT");
        assert!((btc.funding.unwrap() - 0.0001).abs() < 1e-12);
        assert!((btc.premium().unwrap() - (67_012.5 / 66_990.1 - 1.0)).abs() < 1e-12);
        assert!((btc.change_24h.unwrap() - 0.0213).abs() < 1e-12);
        let pepe = s.iter().find(|x| x.coin == "PEPE").unwrap();
        assert!((pepe.price - 0.0000123).abs() < 1e-15, "per-coin price: {}", pepe.price);
        assert_eq!(pepe.funding_hours, 4.0);
        assert!((pepe.funding_8h().unwrap() - 0.0001).abs() < 1e-12);
        assert!((parse_binance_open_interest(&fx("binance_open_interest.json")).unwrap() - 81_234.567).abs() < 1e-9);
        assert_eq!(parse_binance_long_short(&fx("binance_long_short.json")).unwrap(), Some(1.8));
    }

    #[test]
    fn bybit_tickers() {
        let s = parse_bybit(&fx("bybit_tickers_linear.json")).unwrap();
        assert_eq!(s.len(), 2, "pre-market contract without funding skipped");
        let shib = s.iter().find(|x| x.coin == "SHIB").unwrap();
        assert_eq!(shib.symbol, "SHIB1000USDT");
        assert!((shib.price - 0.00001702).abs() < 1e-15);
        let btc = s.iter().find(|x| x.coin == "BTC").unwrap();
        assert_eq!(btc.oi_usd, Some(4_512_345_678.9));
        assert_eq!(btc.funding_hours, 8.0);
        assert!((btc.change_24h.unwrap() - 0.0215).abs() < 1e-12);
        assert!(parse_bybit(r#"{"retCode":10001,"retMsg":"bad"}"#).is_err());
    }

    #[test]
    fn okx_join() {
        let s = parse_okx(
            &fx("okx_tickers_swap.json"),
            &fx("okx_open_interest.json"),
            &fx("okx_mark_price.json"),
            &fx("okx_funding_rate.json"),
            &fx("okx_index_tickers.json"),
        )
        .unwrap();
        assert_eq!(s.len(), 2, "inverse swap skipped");
        let btc = s.iter().find(|x| x.coin == "BTC").unwrap();
        assert_eq!(btc.oi_usd, Some(2_789_012_345.6));
        assert_eq!(btc.mark, Some(67_010.2));
        assert_eq!(btc.index, Some(66_995.0));
        assert_eq!(btc.funding_hours, 8.0);
        assert!((btc.volume_usd.unwrap() - 123_456.0 * 67_000.0).abs() < 1e-3);
        let eth = s.iter().find(|x| x.coin == "ETH").unwrap();
        assert_eq!(eth.funding, None, "no funding row for ETH in this fixture");
        assert!(eth.oi_usd.unwrap() > 0.0, "falls back to oiCcy × last");
    }

    #[test]
    fn hyperliquid_contexts() {
        let s = parse_hyperliquid(&fx("hyperliquid_meta_and_asset_ctxs.json")).unwrap();
        assert_eq!(s.len(), 3, "delisted market skipped");
        let btc = s.iter().find(|x| x.coin == "BTC").unwrap();
        assert_eq!(btc.funding_hours, 1.0);
        assert!((btc.funding_8h().unwrap() - 0.0000125 * 8.0).abs() < 1e-15);
        assert!((btc.oi_usd.unwrap() - 25_000.5 * 67_005.0).abs() < 1e-6);
        let pepe = s.iter().find(|x| x.coin == "PEPE").unwrap();
        assert_eq!(pepe.symbol, "kPEPE");
        assert!((pepe.price - 0.0000123).abs() < 1e-15);
    }

    #[test]
    fn deribit_perp() {
        let s = parse_deribit_perp(&fx("deribit_book_summary_btc.json")).unwrap().unwrap();
        assert_eq!(s.coin, "BTC");
        assert_eq!(s.oi_usd, Some(812_345_670.0));
        assert_eq!(s.funding, Some(0.0000412));
    }

    #[test]
    fn aggregate_weights_by_open_interest() {
        let mk = |e, oi: f64, f: f64, vol: f64, price: f64| PerpSnapshot {
            exchange: e,
            coin: "BTC".into(),
            symbol: "X".into(),
            price,
            mark: Some(price),
            index: Some(price * 0.999),
            funding: Some(f),
            funding_hours: 8.0,
            next_funding: None,
            oi_usd: Some(oi),
            volume_usd: Some(vol),
            change_24h: None,
        };
        let rows = aggregate(vec![
            mk(Exchange::Binance, 300.0, 0.0003, 50.0, 100.0),
            mk(Exchange::Bybit, 100.0, -0.0001, 90.0, 101.0),
            PerpSnapshot { coin: "ETH".into(), ..mk(Exchange::Okx, 50.0, 0.0, 1.0, 5.0) },
        ]);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].coin, "BTC", "largest OI first");
        assert_eq!(rows[0].oi_usd, 400.0);
        assert!((rows[0].funding_8h.unwrap() - 0.0002).abs() < 1e-12);
        assert_eq!(rows[0].price, 101.0, "price from the most traded venue");
        assert!((rows[0].funding_spread().unwrap() - 0.0004).abs() < 1e-12);
        let board = Board { rows, ..Default::default() };
        assert_eq!(board.total_oi(), 450.0);
        assert_eq!(board.oi_by_venue()[&Exchange::Binance], 300.0);
    }
}
