//! Order flow for one market: a live trade stream and a locally maintained order book, turned
//! into an order book **heatmap** (resting liquidity per price over time, with traded volume
//! on top), **footprint** candles (buy and sell volume at each price inside each candle) and
//! **CVD** (cumulative volume delta: aggressive buys minus sells).
//!
//! Venues: Binance USDⓈ-M futures (`<sym>@aggTrade` + `<sym>@depth@100ms` on one combined
//! stream, synced to a `/fapi/v1/depth` snapshot by update id as Binance documents) and
//! Hyperliquid (`trades` + `l2Book`, which sends whole-book snapshots).

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;

use crate::venue::Exchange;
use crate::{Http, int, nice_step, num};

pub const BINANCE_STREAM: &str = "wss://fstream.binance.com/stream";
pub const HYPERLIQUID_WS: &str = "wss://api.hyperliquid.xyz/ws";

/// One aggressive trade.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Trade {
    /// Unix ms.
    pub ts: i64,
    pub price: f64,
    pub qty: f64,
    /// The aggressor bought (lifted the offer).
    pub buy: bool,
}

pub type Levels = Vec<(f64, f64)>;

/// Binance `aggTrade` payload. `m` = the buyer was the maker, so the aggressor sold.
pub fn parse_binance_agg_trade(d: &Value) -> Option<Trade> {
    (d["e"] == "aggTrade").then_some(())?;
    Some(Trade { ts: int(&d["T"])?, price: num(&d["p"])?, qty: num(&d["q"])?, buy: !d["m"].as_bool()? })
}

/// Binance futures `depthUpdate`: first id `U`, final id `u`, previous final id `pu`.
#[derive(Debug, Clone, PartialEq)]
pub struct DepthUpdate {
    pub first: i64,
    pub last: i64,
    pub prev: i64,
    pub bids: Levels,
    pub asks: Levels,
}

fn levels(v: &Value) -> Levels {
    v.as_array().into_iter().flatten().filter_map(|l| Some((num(l.get(0)?)?, num(l.get(1)?)?))).collect()
}

pub fn parse_binance_depth_update(d: &Value) -> Option<DepthUpdate> {
    (d["e"] == "depthUpdate").then_some(())?;
    Some(DepthUpdate {
        first: int(&d["U"])?,
        last: int(&d["u"])?,
        prev: int(&d["pu"]).unwrap_or(-1),
        bids: levels(&d["b"]),
        asks: levels(&d["a"]),
    })
}

/// `/fapi/v1/depth`: `(lastUpdateId, bids, asks)`.
pub fn parse_binance_depth_snapshot(json: &str) -> Result<(i64, Levels, Levels)> {
    let v: Value = serde_json::from_str(json)?;
    Ok((int(&v["lastUpdateId"]).context("no lastUpdateId")?, levels(&v["bids"]), levels(&v["asks"])))
}

/// Hyperliquid `trades` channel; `side` is the aggressor (`B` buy, `A` sell).
pub fn parse_hl_trades(v: &Value) -> Vec<Trade> {
    if v["channel"] != "trades" {
        return Vec::new();
    }
    v["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|t| {
            Some(Trade {
                ts: int(&t["time"])?,
                price: num(&t["px"])?,
                qty: num(&t["sz"])?,
                buy: t["side"].as_str()? == "B",
            })
        })
        .collect()
}

/// Hyperliquid `l2Book` channel: `{"levels":[[bids…],[asks…]]}` with `{px, sz, n}` levels.
pub fn parse_hl_book(v: &Value) -> Option<(Levels, Levels)> {
    (v["channel"] == "l2Book").then_some(())?;
    let lv = v["data"]["levels"].as_array()?;
    let side = |s: &Value| -> Levels {
        s.as_array().into_iter().flatten().filter_map(|l| Some((num(&l["px"])?, num(&l["sz"])?))).collect()
    };
    Some((side(lv.first()?), side(lv.get(1)?)))
}

/// Price key that orders like the price (valid for positive finite prices).
fn key(p: f64) -> u64 {
    p.to_bits()
}

fn price(k: u64) -> f64 {
    f64::from_bits(k)
}

/// A local order book.
#[derive(Debug, Clone, Default)]
pub struct Book {
    bids: BTreeMap<u64, f64>,
    asks: BTreeMap<u64, f64>,
}

impl Book {
    pub fn replace(&mut self, bids: &[(f64, f64)], asks: &[(f64, f64)]) {
        self.bids.clear();
        self.asks.clear();
        self.apply(bids, asks);
    }

    /// Apply absolute level sizes; zero removes the level.
    pub fn apply(&mut self, bids: &[(f64, f64)], asks: &[(f64, f64)]) {
        for (side, levels) in [(&mut self.bids, bids), (&mut self.asks, asks)] {
            for &(p, q) in levels {
                if !(p.is_finite() && p > 0.0) {
                    continue;
                }
                if q > 0.0 {
                    side.insert(key(p), q);
                } else {
                    side.remove(&key(p));
                }
            }
        }
    }

    pub fn best_bid(&self) -> Option<f64> {
        self.bids.keys().next_back().map(|k| price(*k))
    }

    pub fn best_ask(&self) -> Option<f64> {
        self.asks.keys().next().map(|k| price(*k))
    }

    pub fn mid(&self) -> Option<f64> {
        Some((self.best_bid()? + self.best_ask()?) / 2.0)
    }

    pub fn is_empty(&self) -> bool {
        self.bids.is_empty() || self.asks.is_empty()
    }

    /// Bids best first.
    pub fn bids(&self) -> impl Iterator<Item = (f64, f64)> + '_ {
        self.bids.iter().rev().map(|(k, q)| (price(*k), *q))
    }

    /// Asks best first.
    pub fn asks(&self) -> impl Iterator<Item = (f64, f64)> + '_ {
        self.asks.iter().map(|(k, q)| (price(*k), *q))
    }

    /// Levels between `lo` and `hi` (both sides).
    pub fn range(&self, lo: f64, hi: f64) -> impl Iterator<Item = (f64, f64, bool)> + '_ {
        let (lo, hi) = (key(lo.max(f64::MIN_POSITIVE)), key(hi.max(f64::MIN_POSITIVE)));
        let b = self.bids.range(lo..=hi).map(|(k, q)| (price(*k), *q, true));
        let a = self.asks.range(lo..=hi).map(|(k, q)| (price(*k), *q, false));
        b.chain(a)
    }
}

/// Keeps a Binance futures book in step with its diff stream.
#[derive(Debug, Default)]
pub struct BinanceSync {
    pub book: Book,
    last: Option<i64>,
    snapshot: Option<i64>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum SyncResult {
    Applied,
    /// Older than the snapshot, ignored.
    Stale,
    /// A gap: the book must be rebuilt from a new snapshot.
    Gap,
}

impl BinanceSync {
    pub fn snapshot(&mut self, last_update_id: i64, bids: &[(f64, f64)], asks: &[(f64, f64)]) {
        self.book.replace(bids, asks);
        self.snapshot = Some(last_update_id);
        self.last = None;
    }

    pub fn update(&mut self, u: &DepthUpdate) -> SyncResult {
        let Some(snap) = self.snapshot else { return SyncResult::Gap };
        if u.last < snap {
            return SyncResult::Stale;
        }
        match self.last {
            // First event after the snapshot must straddle it.
            None if !(u.first <= snap && u.last >= snap) => return SyncResult::Gap,
            Some(prev) if u.prev != prev => return SyncResult::Gap,
            _ => {}
        }
        self.book.apply(&u.bids, &u.asks);
        self.last = Some(u.last);
        SyncResult::Applied
    }
}

/// One heatmap column: resting liquidity per price bucket at one moment, and the volume
/// traded since the previous column.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct HeatCol {
    /// Unix ms.
    pub ts: i64,
    /// Bucket index (`floor(price / step)`) of `cells[0]`.
    pub base: i64,
    /// Resting size (coins) per bucket, bids and asks together.
    pub cells: Vec<f32>,
    pub bid: f64,
    pub ask: f64,
    pub buy: f64,
    pub sell: f64,
    /// Volume-weighted price of the trades in this column.
    pub vwap: Option<f64>,
}

impl HeatCol {
    pub fn size_at(&self, bucket: i64) -> f32 {
        usize::try_from(bucket - self.base).ok().and_then(|i| self.cells.get(i)).copied().unwrap_or(0.0)
    }

    pub fn max_size(&self) -> f32 {
        self.cells.iter().copied().fold(0.0, f32::max)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Heatmap {
    /// Price per bucket.
    pub step: f64,
    /// Fraction of the mid price sampled each side.
    pub range: f64,
    pub cols: VecDeque<HeatCol>,
    pub max_cols: usize,
    pending: (f64, f64, f64, f64),
}

impl Heatmap {
    pub fn new(step: f64) -> Heatmap {
        Heatmap { step, range: 0.02, cols: VecDeque::new(), max_cols: 1_800, pending: (0.0, 0.0, 0.0, 0.0) }
    }

    pub fn bucket(&self, p: f64) -> i64 {
        (p / self.step).floor() as i64
    }

    fn trade(&mut self, t: &Trade) {
        if t.buy {
            self.pending.0 += t.qty;
        } else {
            self.pending.1 += t.qty;
        }
        self.pending.2 += t.price * t.qty;
        self.pending.3 += t.qty;
    }

    /// Close a column from the current book.
    pub fn sample(&mut self, book: &Book, ts: i64) {
        let Some(mid) = book.mid() else { return };
        let (lo, hi) = (mid * (1.0 - self.range), mid * (1.0 + self.range));
        let base = self.bucket(lo);
        let n = (self.bucket(hi) - base + 1).clamp(1, 4_000) as usize;
        let mut cells = vec![0f32; n];
        for (p, q, _) in book.range(lo, hi) {
            if let Some(c) = usize::try_from(self.bucket(p) - base).ok().and_then(|i| cells.get_mut(i)) {
                *c += q as f32;
            }
        }
        let (buy, sell, pv, v) = std::mem::take(&mut self.pending);
        self.cols.push_back(HeatCol {
            ts,
            base,
            cells,
            bid: book.best_bid().unwrap_or(mid),
            ask: book.best_ask().unwrap_or(mid),
            buy,
            sell,
            vwap: (v > 0.0).then(|| pv / v),
        });
        while self.cols.len() > self.max_cols {
            self.cols.pop_front();
        }
    }
}

/// A candle with volume at price.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FootCandle {
    /// Unix seconds (candle open).
    pub ts: i64,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub buy: f64,
    pub sell: f64,
    /// CVD at the candle close.
    pub cvd: f64,
    /// Bucket index -> (buy, sell) volume.
    pub levels: BTreeMap<i64, (f64, f64)>,
}

impl FootCandle {
    pub fn delta(&self) -> f64 {
        self.buy - self.sell
    }

    pub fn volume(&self) -> f64 {
        self.buy + self.sell
    }

    /// Point of control: the bucket with the most volume.
    pub fn poc(&self) -> Option<i64> {
        self.levels.iter().max_by(|a, b| (a.1.0 + a.1.1).total_cmp(&(b.1.0 + b.1.1))).map(|(k, _)| *k)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Footprint {
    pub tf_secs: i64,
    pub step: f64,
    pub candles: VecDeque<FootCandle>,
    pub max_candles: usize,
    pub cvd: f64,
}

impl Footprint {
    pub fn new(tf_secs: i64, step: f64) -> Footprint {
        Footprint { tf_secs: tf_secs.max(1), step, candles: VecDeque::new(), max_candles: 600, cvd: 0.0 }
    }

    pub fn bucket(&self, p: f64) -> i64 {
        (p / self.step).floor() as i64
    }

    pub fn add(&mut self, t: &Trade) {
        let ts = (t.ts / 1000).div_euclid(self.tf_secs) * self.tf_secs;
        if self.candles.back().is_none_or(|c| c.ts < ts) {
            self.candles.push_back(FootCandle {
                ts,
                open: t.price,
                high: t.price,
                low: t.price,
                close: t.price,
                cvd: self.cvd,
                ..Default::default()
            });
            while self.candles.len() > self.max_candles {
                self.candles.pop_front();
            }
        }
        let b = self.bucket(t.price);
        let Some(c) = self.candles.back_mut().filter(|c| c.ts == ts) else { return };
        c.high = c.high.max(t.price);
        c.low = c.low.min(t.price);
        c.close = t.price;
        let lvl = c.levels.entry(b).or_insert((0.0, 0.0));
        if t.buy {
            c.buy += t.qty;
            lvl.0 += t.qty;
            self.cvd += t.qty;
        } else {
            c.sell += t.qty;
            lvl.1 += t.qty;
            self.cvd -= t.qty;
        }
        c.cvd = self.cvd;
    }
}

/// Price bucket for footprints at `tf_secs`, from the price (≈1 bp per minute-ish candle).
pub fn footprint_step(price: f64, tf_secs: i64) -> f64 {
    let scale = ((tf_secs as f64) / 60.0).sqrt().clamp(1.0, 8.0);
    nice_step(price * 1e-4 * scale)
}

/// Price bucket for the heatmap.
pub fn heatmap_step(price: f64) -> f64 {
    nice_step(price * 1e-4)
}

/// Live state for one market, shared between the stream task and the UI.
#[derive(Debug)]
pub struct Flow {
    pub venue: Exchange,
    pub coin: String,
    pub symbol: String,
    pub status: String,
    pub live: bool,
    pub simulated: bool,
    pub book: Book,
    pub heat: Heatmap,
    pub foot: Footprint,
    /// Recent trades, newest last (bounded; used to rebuild footprints at a new interval).
    pub trades: VecDeque<Trade>,
    pub last: Option<f64>,
    /// When the stream started delivering (Unix ms).
    pub since: Option<i64>,
}

impl Flow {
    pub const MAX_TRADES: usize = 200_000;

    pub fn new(venue: Exchange, coin: &str, tf_secs: i64) -> Flow {
        let coin = coin.to_ascii_uppercase();
        let symbol = match venue {
            Exchange::Hyperliquid => crate::venue::hyperliquid_perp(&coin).0,
            _ => crate::venue::binance_perp(&coin).0,
        };
        Flow {
            venue,
            coin,
            symbol,
            status: "connecting".into(),
            live: false,
            simulated: false,
            book: Book::default(),
            heat: Heatmap::new(0.0),
            foot: Footprint::new(tf_secs, 0.0),
            trades: VecDeque::new(),
            last: None,
            since: None,
        }
    }

    /// Size buckets from the first price seen.
    fn ensure_steps(&mut self, p: f64) {
        if self.heat.step <= 0.0 {
            self.heat.step = heatmap_step(p);
        }
        if self.foot.step <= 0.0 {
            self.foot.step = footprint_step(p, self.foot.tf_secs);
        }
    }

    pub fn on_trade(&mut self, t: Trade) {
        self.ensure_steps(t.price);
        self.since.get_or_insert(t.ts);
        self.foot.add(&t);
        self.heat.trade(&t);
        self.last = Some(t.price);
        self.trades.push_back(t);
        while self.trades.len() > Self::MAX_TRADES {
            self.trades.pop_front();
        }
    }

    /// Close a heatmap column (called about once a second).
    pub fn tick(&mut self, now_ms: i64) {
        if let Some(m) = self.book.mid() {
            self.ensure_steps(m);
            self.heat.sample(&self.book, now_ms);
        }
    }

    /// Rebuild the footprint at another interval from the stored trades.
    pub fn set_timeframe(&mut self, tf_secs: i64) {
        if tf_secs == self.foot.tf_secs {
            return;
        }
        let p = self.last.unwrap_or(0.0);
        let mut f = Footprint::new(tf_secs, if p > 0.0 { footprint_step(p, tf_secs) } else { 0.0 });
        for t in &self.trades {
            f.add(t);
        }
        self.foot = f;
    }

    /// Change the footprint price bucket (rebuilds from stored trades).
    pub fn set_footprint_step(&mut self, step: f64) {
        let mut f = Footprint::new(self.foot.tf_secs, step);
        for t in &self.trades {
            f.add(t);
        }
        self.foot = f;
    }
}

pub type SharedFlow = Arc<Mutex<Flow>>;

fn with(flow: &SharedFlow, f: impl FnOnce(&mut Flow)) {
    if let Ok(mut g) = flow.lock() {
        f(&mut g);
    }
}

/// Stream into `flow` until every other holder drops it, reconnecting on errors.
pub async fn run(flow: SharedFlow, http: Http) {
    let venue = flow.lock().map(|f| f.venue).unwrap_or(Exchange::Binance);
    let mut backoff = 1u64;
    while Arc::strong_count(&flow) > 1 {
        let r = match venue {
            Exchange::Hyperliquid => run_hyperliquid(&flow).await,
            _ => run_binance(&flow, &http).await,
        };
        let msg = match r {
            Ok(()) => return,
            Err(e) => format!("{e:#}"),
        };
        with(&flow, |f| {
            f.live = false;
            f.status = format!("reconnecting: {msg}");
        });
        tokio::time::sleep(Duration::from_secs(backoff)).await;
        backoff = (backoff * 2).min(30);
    }
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

async fn run_binance(flow: &SharedFlow, http: &Http) -> Result<()> {
    let sym = flow.lock().map(|f| f.symbol.clone()).unwrap_or_default();
    let lower = sym.to_ascii_lowercase();
    let url = format!("{BINANCE_STREAM}?streams={lower}@aggTrade/{lower}@depth@100ms");
    let (mut ws, _) = tokio::time::timeout(Duration::from_secs(15), tokio_tungstenite::connect_async(url.as_str()))
        .await
        .context("connect timed out")??;
    let mut sync = BinanceSync::default();
    let snapshot = |http: Http, sym: String| async move {
        let j = http
            .get(&format!("{}/fapi/v1/depth", crate::perps::BINANCE_FAPI), &[("symbol", sym), ("limit", "1000".into())])
            .await?;
        parse_binance_depth_snapshot(&j)
    };
    // Stream events queue in the socket while the snapshot loads.
    let (id, b, a) = snapshot(http.clone(), sym.clone()).await?;
    sync.snapshot(id, &b, &a);
    with(flow, |f| {
        f.status = "live".into();
        f.live = true;
    });
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    loop {
        tokio::select! {
            msg = ws.next() => {
                let text = match msg {
                    Some(Ok(Message::Text(t))) => t,
                    Some(Ok(Message::Ping(p))) => { ws.send(Message::Pong(p)).await?; continue }
                    Some(Ok(Message::Close(_))) | None => bail!("stream closed"),
                    Some(Err(e)) => return Err(e.into()),
                    _ => continue,
                };
                let Ok(v) = serde_json::from_str::<Value>(text.as_str()) else { continue };
                let d = &v["data"];
                if let Some(t) = parse_binance_agg_trade(d) {
                    with(flow, |f| f.on_trade(t));
                } else if let Some(u) = parse_binance_depth_update(d)
                    && sync.update(&u) == SyncResult::Gap
                {
                    tracing::debug!("{sym} depth gap at {}; resyncing", u.first);
                    let (id, b, a) = snapshot(http.clone(), sym.clone()).await?;
                    sync.snapshot(id, &b, &a);
                }
            }
            _ = tick.tick() => {
                if Arc::strong_count(flow) <= 1 {
                    let _ = ws.close(None).await;
                    return Ok(());
                }
                let book = sync.book.clone();
                with(flow, |f| {
                    f.book = book;
                    f.tick(now_ms());
                });
            }
        }
    }
}

async fn run_hyperliquid(flow: &SharedFlow) -> Result<()> {
    let coin = flow.lock().map(|f| f.symbol.clone()).unwrap_or_default();
    let (mut ws, _) = tokio::time::timeout(Duration::from_secs(15), tokio_tungstenite::connect_async(HYPERLIQUID_WS))
        .await
        .context("connect timed out")??;
    for kind in ["trades", "l2Book"] {
        let sub = json!({"method": "subscribe", "subscription": {"type": kind, "coin": coin}});
        ws.send(Message::Text(sub.to_string().into())).await?;
    }
    with(flow, |f| {
        f.status = "live".into();
        f.live = true;
    });
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    let mut pings = 0u32;
    loop {
        tokio::select! {
            msg = ws.next() => {
                let text = match msg {
                    Some(Ok(Message::Text(t))) => t,
                    Some(Ok(Message::Ping(p))) => { ws.send(Message::Pong(p)).await?; continue }
                    Some(Ok(Message::Close(_))) | None => bail!("stream closed"),
                    Some(Err(e)) => return Err(e.into()),
                    _ => continue,
                };
                let Ok(v) = serde_json::from_str::<Value>(text.as_str()) else { continue };
                let trades = parse_hl_trades(&v);
                let book = parse_hl_book(&v);
                with(flow, |f| {
                    for t in trades {
                        f.on_trade(t);
                    }
                    if let Some((b, a)) = book {
                        f.book.replace(&b, &a);
                    }
                });
            }
            _ = tick.tick() => {
                if Arc::strong_count(flow) <= 1 {
                    let _ = ws.close(None).await;
                    return Ok(());
                }
                with(flow, |f| f.tick(now_ms()));
                pings += 1;
                // Hyperliquid closes idle connections after 60 s.
                if pings.is_multiple_of(30) {
                    ws.send(Message::Text(r#"{"method":"ping"}"#.into())).await?;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trade(ts: i64, price: f64, qty: f64, buy: bool) -> Trade {
        Trade { ts, price, qty, buy }
    }

    #[test]
    fn parses_binance_events() {
        let v: Value = serde_json::from_str(r#"{"stream":"btcusdt@aggTrade","data":{"e":"aggTrade","E":1760936400010,"a":5933014,"s":"BTCUSDT","p":"67000.10","q":"0.250","f":100,"l":105,"T":1760936400005,"m":true}}"#).unwrap();
        assert_eq!(parse_binance_agg_trade(&v["data"]), Some(trade(1_760_936_400_005, 67_000.1, 0.25, false)));
        let v: Value = serde_json::from_str(r#"{"stream":"btcusdt@depth@100ms","data":{"e":"depthUpdate","E":1,"T":1,"s":"BTCUSDT","U":157,"u":160,"pu":149,"b":[["66999.9","1.5"],["66990.0","0"]],"a":[["67000.1","2"]]}}"#).unwrap();
        let u = parse_binance_depth_update(&v["data"]).unwrap();
        assert_eq!((u.first, u.last, u.prev), (157, 160, 149));
        assert_eq!(u.bids, vec![(66_999.9, 1.5), (66_990.0, 0.0)]);
        let (id, b, a) = parse_binance_depth_snapshot(
            r#"{"lastUpdateId":158,"E":1,"T":1,"bids":[["66999.0","3"]],"asks":[["67001.0","4"]]}"#,
        )
        .unwrap();
        assert_eq!((id, b.len(), a.len()), (158, 1, 1));
    }

    #[test]
    fn binance_sync_rules() {
        let up = |first, last, prev, bid: f64| DepthUpdate { first, last, prev, bids: vec![(bid, 1.0)], asks: vec![] };
        let mut s = BinanceSync::default();
        assert_eq!(s.update(&up(1, 2, 0, 10.0)), SyncResult::Gap, "no snapshot yet");
        s.snapshot(100, &[(9.0, 1.0)], &[(11.0, 1.0)]);
        assert_eq!(s.update(&up(90, 99, 89, 10.0)), SyncResult::Stale);
        assert_eq!(s.update(&up(101, 105, 99, 10.0)), SyncResult::Gap, "first event must straddle the snapshot");
        assert_eq!(s.update(&up(95, 102, 94, 10.0)), SyncResult::Applied);
        assert_eq!(s.book.best_bid(), Some(10.0));
        assert_eq!(s.update(&up(103, 104, 102, 10.5)), SyncResult::Applied);
        assert_eq!(s.update(&up(106, 107, 105, 10.6)), SyncResult::Gap, "pu must equal the previous u");
    }

    #[test]
    fn hyperliquid_messages() {
        let v: Value = serde_json::from_str(r#"{"channel":"trades","data":[{"coin":"BTC","side":"B","px":"67000.0","sz":"0.01","time":1760936400000,"hash":"0x0","tid":1,"users":["0xa","0xb"]},{"coin":"BTC","side":"A","px":"66999.0","sz":"0.02","time":1760936400001,"hash":"0x0","tid":2,"users":["0xa","0xb"]}]}"#).unwrap();
        let t = parse_hl_trades(&v);
        assert_eq!(t.len(), 2);
        assert!(t[0].buy && !t[1].buy);
        let v: Value = serde_json::from_str(r#"{"channel":"l2Book","data":{"coin":"BTC","time":1,"levels":[[{"px":"66999.0","sz":"1.0","n":2}],[{"px":"67001.0","sz":"2.0","n":1},{"px":"67002.0","sz":"3.0","n":4}]]}}"#).unwrap();
        let (b, a) = parse_hl_book(&v).unwrap();
        assert_eq!((b.len(), a.len()), (1, 2));
        assert!(parse_hl_book(&serde_json::json!({"channel":"subscriptionResponse"})).is_none());
    }

    #[test]
    fn book_and_heatmap() {
        let mut book = Book::default();
        book.replace(&[(99.0, 2.0), (98.0, 5.0)], &[(101.0, 1.0), (103.0, 7.0)]);
        book.apply(&[(98.0, 0.0)], &[(102.0, 4.0)]);
        assert_eq!((book.best_bid(), book.best_ask(), book.mid()), (Some(99.0), Some(101.0), Some(100.0)));
        assert_eq!(book.bids().count(), 1);
        let mut h = Heatmap::new(1.0);
        h.range = 0.05;
        h.trade(&trade(0, 100.0, 2.0, true));
        h.trade(&trade(0, 101.0, 2.0, false));
        h.sample(&book, 1_000);
        let c = &h.cols[0];
        assert_eq!(c.size_at(h.bucket(103.0)), 7.0);
        assert_eq!(c.size_at(h.bucket(102.0)), 4.0);
        assert_eq!(c.size_at(h.bucket(500.0)), 0.0);
        assert_eq!((c.buy, c.sell, c.vwap), (2.0, 2.0, Some(100.5)));
        assert_eq!(c.max_size(), 7.0);
        h.sample(&book, 2_000);
        assert_eq!(h.cols[1].buy, 0.0, "pending volume resets per column");
    }

    #[test]
    fn footprint_and_cvd() {
        let mut f = Footprint::new(60, 10.0);
        f.add(&trade(60_000, 100.0, 1.0, true));
        f.add(&trade(61_000, 105.0, 2.0, false));
        f.add(&trade(62_000, 112.0, 2.5, true));
        f.add(&trade(120_500, 111.0, 1.0, false));
        assert_eq!(f.candles.len(), 2);
        let c = &f.candles[0];
        assert_eq!((c.ts, c.open, c.high, c.low, c.close), (60, 100.0, 112.0, 100.0, 112.0));
        assert_eq!(c.levels[&10], (1.0, 2.0));
        assert_eq!(c.levels[&11], (2.5, 0.0));
        assert_eq!((c.delta(), c.cvd, c.poc()), (1.5, 1.5, Some(10)));
        assert_eq!(f.candles[1].cvd, 0.5);
        assert_eq!(f.cvd, 0.5);
    }

    #[test]
    fn flow_rebuilds_footprint_at_new_interval() {
        let mut fl = Flow::new(Exchange::Binance, "pepe", 60);
        assert_eq!(fl.symbol, "1000PEPEUSDT");
        for i in 0..300 {
            fl.on_trade(trade(i * 1_000, 100.0 + (i % 7) as f64, 1.0, i % 3 != 0));
        }
        assert_eq!(fl.foot.candles.len(), 5);
        fl.set_timeframe(300);
        assert_eq!(fl.foot.candles.len(), 1);
        assert!((fl.foot.cvd - 100.0).abs() < 1e-9, "200 buys - 100 sells");
        assert!(footprint_step(67_000.0, 60) == 10.0 && heatmap_step(67_000.0) == 10.0);
    }
}
