//! Deterministic simulated data for offline use (`TR_PROVIDER=sample`): every structure is
//! flagged `simulated` so panes can label it.

use std::sync::Arc;
use std::time::Duration;

use crate::deribit::{TermPoint, TermStructure};
use crate::history::CoinHistory;
use crate::liq::{Liquidation, SharedTape, Side};
use crate::orderflow::{Book, SharedFlow, Trade};
use crate::perps::{Board, PerpSnapshot, aggregate};
use crate::polymarket::{self, Market};
use crate::venue::Exchange;

/// xorshift64*: small, fast and reproducible.
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform in [0, 1).
    pub fn f(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Roughly normal (sum of uniforms), mean 0, sd 1.
    pub fn n(&mut self) -> f64 {
        // Six uniforms: mean 3, variance 1/2.
        ((0..6).map(|_| self.f()).sum::<f64>() - 3.0) * std::f64::consts::SQRT_2
    }
}

fn seed(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3))
}

/// (coin, price, open interest in $bn)
const COINS: [(&str, f64, f64); 20] = [
    ("BTC", 67_000.0, 32.0),
    ("ETH", 2_560.0, 14.0),
    ("SOL", 152.0, 4.2),
    ("XRP", 0.54, 1.6),
    ("DOGE", 0.118, 1.4),
    ("HYPE", 24.5, 1.3),
    ("BNB", 585.0, 0.9),
    ("SUI", 1.95, 0.8),
    ("LINK", 11.4, 0.55),
    ("AVAX", 26.3, 0.5),
    ("PEPE", 0.0000123, 0.48),
    ("ADA", 0.35, 0.45),
    ("WIF", 2.1, 0.4),
    ("TON", 5.2, 0.33),
    ("LTC", 68.0, 0.3),
    ("ARB", 0.56, 0.24),
    ("NEAR", 4.9, 0.22),
    ("APT", 8.7, 0.2),
    ("OP", 1.62, 0.18),
    ("ENA", 0.32, 0.17),
];

/// A simulated board; values drift slowly with `now` (Unix seconds).
pub fn board(now: i64) -> Board {
    let mut r = Rng::new((now / 30) as u64);
    let mut snaps = Vec::new();
    let share =
        [(Exchange::Binance, 0.38), (Exchange::Bybit, 0.27), (Exchange::Okx, 0.17), (Exchange::Hyperliquid, 0.18)];
    for (i, (coin, px, oi_bn)) in COINS.iter().enumerate() {
        let mut cr = Rng::new(seed(coin));
        let bias = cr.n() * 0.00008;
        let chg = cr.n() * 0.03 + r.n() * 0.002;
        for (e, s) in share {
            let hours = if e == Exchange::Hyperliquid { 1.0 } else { 8.0 };
            let f8 = 0.0001 + bias + r.n() * 0.00003;
            let price = px * (1.0 + r.n() * 0.0003);
            snaps.push(PerpSnapshot {
                exchange: e,
                coin: (*coin).into(),
                symbol: match e {
                    Exchange::Okx => format!("{coin}-USDT-SWAP"),
                    Exchange::Hyperliquid => (*coin).into(),
                    _ => format!("{coin}USDT"),
                },
                price,
                mark: Some(price * (1.0 + 0.0002 * r.n())),
                index: Some(price * (1.0 - 0.0001)),
                funding: Some(f8 * hours / 8.0),
                funding_hours: hours,
                next_funding: Some((now / 28_800 + 1) * 28_800 * 1000),
                oi_usd: Some(oi_bn * 1e9 * s * (1.0 + 0.05 * cr.n())),
                volume_usd: Some(oi_bn * 1e9 * s * (1.4 + 0.4 * cr.f())),
                change_24h: Some(chg),
            });
        }
        if i < 2 {
            snaps.push(PerpSnapshot {
                exchange: Exchange::Deribit,
                coin: (*coin).into(),
                symbol: format!("{coin}-PERPETUAL"),
                price: *px,
                mark: Some(*px),
                index: Some(px * 0.9998),
                funding: Some(0.00005),
                funding_hours: 8.0,
                next_funding: None,
                oi_usd: Some(oi_bn * 1e9 * 0.03),
                volume_usd: Some(oi_bn * 1e9 * 0.02),
                change_24h: Some(chg),
            });
        }
    }
    let mut rows = aggregate(snaps);
    for row in &mut rows {
        let mut cr = Rng::new(seed(&row.coin) ^ 7);
        row.long_short = Some(1.0 + cr.f() * 1.8);
        row.oi_change_24h = Some(cr.n() * 0.06);
    }
    Board { rows, errors: Vec::new(), updated: now, simulated: true }
}

fn walk(r: &mut Rng, n: usize, start: f64, vol: f64) -> Vec<f64> {
    let mut v = start;
    (0..n)
        .map(|_| {
            v *= 1.0 + r.n() * vol;
            v
        })
        .collect()
}

pub fn history(coin: &str, now: i64) -> CoinHistory {
    let mut r = Rng::new(seed(coin));
    let base = COINS.iter().find(|c| c.0.eq_ignore_ascii_case(coin)).map_or(0.3, |c| c.2) * 0.38e9;
    let hours: Vec<i64> = (0..168).map(|i| (now / 3600 - 167 + i) * 3600).collect();
    let oi = walk(&mut r, 168, base, 0.01);
    let ls = walk(&mut r, 168, 1.6, 0.01);
    let tk = walk(&mut r, 168, 1.0, 0.03);
    let funding: Vec<(i64, f64)> =
        (0..90).map(|i| ((now / 28_800 - 89 + i) * 28_800, 0.0001 + r.n() * 0.00006)).collect();
    CoinHistory {
        coin: coin.to_ascii_uppercase(),
        oi_usd: hours.iter().copied().zip(oi).collect(),
        funding,
        long_short: hours.iter().copied().zip(ls).collect(),
        taker_ratio: hours.iter().copied().zip(tk).collect(),
        errors: Vec::new(),
        simulated: true,
    }
}

pub fn term_structure(ccy: &str, now: i64) -> TermStructure {
    let index = if ccy.eq_ignore_ascii_case("ETH") { 2_560.0 } else { 67_000.0 };
    let mut points = vec![TermPoint {
        instrument: format!("{}-PERPETUAL", ccy.to_ascii_uppercase()),
        expiry: None,
        mark: index * 1.0002,
        index,
        oi_usd: 8e8,
        volume_usd: 4e8,
    }];
    for (k, days) in [7.0, 28.0, 66.0, 157.0, 248.0].into_iter().enumerate() {
        let ann = 0.085 + 0.01 * k as f64;
        points.push(TermPoint {
            instrument: format!("{}-T+{days:.0}D", ccy.to_ascii_uppercase()),
            expiry: Some(now + (days * 86_400.0) as i64),
            mark: index * (1.0 + ann * days / 365.0),
            index,
            oi_usd: 4e8 / (k as f64 + 1.0),
            volume_usd: 1e8 / (k as f64 + 1.0),
        });
    }
    let mut r = Rng::new(seed(ccy));
    let dvol = (0..168).map(|i| now - (167 - i) * 3600).zip(walk(&mut r, 168, 48.0, 0.01)).collect();
    TermStructure { currency: ccy.to_ascii_uppercase(), points, dvol, error: None, simulated: true }
}

/// Simulated liquidations until the tape is dropped by everyone else.
pub async fn run_liquidations(tape: SharedTape) {
    if let Ok(mut t) = tape.lock() {
        t.simulated = true;
        for e in [Exchange::Binance, Exchange::Bybit, Exchange::Okx] {
            t.status.insert(e, "simulated".into());
        }
    }
    let mut r = Rng::new(42);
    let venues = [Exchange::Binance, Exchange::Bybit, Exchange::Okx];
    while Arc::strong_count(&tape) > 1 {
        tokio::time::sleep(Duration::from_millis(150 + (r.f() * 900.0) as u64)).await;
        let (coin, px, _) = COINS[(r.f().powi(2) * COINS.len() as f64) as usize];
        let usd = 2_000.0 * (r.f() * 7.0).exp();
        let l = Liquidation {
            exchange: venues[(r.f() * 3.0) as usize],
            coin: coin.into(),
            side: if r.f() < 0.55 { Side::Long } else { Side::Short },
            price: px,
            qty: usd / px,
            usd,
            ts: chrono::Utc::now().timestamp_millis(),
        };
        if let Ok(mut t) = tape.lock() {
            t.push(l);
        }
    }
}

/// A synthetic book around `mid` with a few large resting walls. Liquidity at each price
/// level is stable over time (seeded by the level), so a heatmap shows bands as it would
/// for a real book.
pub fn book(r: &mut Rng, mid: f64, step: f64, walls: &[(f64, f64)]) -> Book {
    let mut bids = Vec::new();
    let mut asks = Vec::new();
    let mut level = |p: f64| {
        let mut lr = Rng::new((p / step).round() as i64 as u64);
        let band = if lr.f() < 0.08 { 4.0 + 8.0 * lr.f() } else { 0.3 + lr.f() };
        band * (0.85 + 0.3 * r.f())
    };
    let snap = |p: f64| (p / step).round() * step;
    for i in 1..=300 {
        let d = i as f64 * step;
        let depth = 1.0 + i as f64 / 120.0;
        let (b, a) = (snap(mid - d), snap(mid + d));
        bids.push((b, level(b) * depth));
        asks.push((a, level(a) * depth));
    }
    for &(p, q) in walls {
        let side = if p < mid { &mut bids } else { &mut asks };
        if let Some(l) = side.iter_mut().min_by(|a, b| (a.0 - p).abs().total_cmp(&(b.0 - p).abs())) {
            l.1 += q;
        }
    }
    let mut b = Book::default();
    b.replace(&bids, &asks);
    b
}

/// Simulated trades and book for a flow pane, ten updates a second.
pub async fn run_flow(flow: SharedFlow) {
    let (coin, venue) = flow.lock().map(|f| (f.coin.clone(), f.venue)).unwrap_or_default();
    let mut r = Rng::new(seed(&coin) ^ venue as u64);
    let mut mid = COINS.iter().find(|c| c.0 == coin).map_or(100.0, |c| c.1);
    let step = crate::orderflow::heatmap_step(mid);
    let mut walls: Vec<(f64, f64)> =
        (0..6).map(|k| (mid * (1.0 + (k as f64 - 2.5) * 0.004), 40.0 + r.f() * 80.0)).collect();
    if let Ok(mut f) = flow.lock() {
        f.simulated = true;
        f.live = true;
        f.status = "simulated".into();
        // Seed a few minutes of history so the panes are not empty.
        let now = chrono::Utc::now().timestamp_millis();
        for k in (0..1_800).rev() {
            let ts = now - k * 100;
            mid *= 1.0 + r.n() * 0.00004;
            for _ in 0..(r.f() * 3.0) as usize {
                f.on_trade(Trade {
                    ts,
                    price: mid + (r.f() - 0.5) * step,
                    qty: 0.002 * (r.f() * 6.0).exp(),
                    buy: r.f() < 0.5,
                });
            }
            if k % 10 == 0 {
                f.book = book(&mut r, mid, step, &walls);
                f.tick(ts);
            }
        }
    }
    let mut n = 0u64;
    while Arc::strong_count(&flow) > 1 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        n += 1;
        mid *= 1.0 + r.n() * 0.00004;
        if n.is_multiple_of(50) {
            let k = (r.f() * walls.len() as f64) as usize;
            walls[k] = (mid * (1.0 + r.n() * 0.006), 30.0 + r.f() * 120.0);
        }
        let now = chrono::Utc::now().timestamp_millis();
        if let Ok(mut f) = flow.lock() {
            for _ in 0..(r.f() * 4.0) as usize {
                f.on_trade(Trade {
                    ts: now,
                    price: mid + (r.f() - 0.5) * step,
                    qty: 0.002 * (r.f() * 6.0).exp(),
                    buy: r.f() < 0.5,
                });
            }
            if n.is_multiple_of(10) {
                f.book = book(&mut r, mid, step, &walls);
                f.tick(now);
            }
        }
    }
}

pub fn markets() -> Vec<Market> {
    let qs: [(&str, &str, f64); 10] = [
        ("Will the Fed cut rates in December?", "Economics", 0.62),
        ("Bitcoin above $100k on December 31?", "Crypto", 0.31),
        ("Will ETH ETF net inflows top $1B in November?", "Crypto", 0.44),
        ("US recession in 2026?", "Economics", 0.18),
        ("Will SpaceX launch Starship to orbit this year?", "Science", 0.71),
        ("Government shutdown before January?", "Politics", 0.27),
        ("Will gold close the year above $3,000?", "Commodities", 0.83),
        ("Will Nvidia be the largest company by market cap on Dec 31?", "Business", 0.57),
        ("Will a new Fed chair be confirmed by March?", "Politics", 0.49),
        ("Will Hyperliquid daily volume exceed $20B in Q4?", "Crypto", 0.36),
    ];
    qs.iter()
        .enumerate()
        .map(|(i, (q, cat, p))| {
            let mut r = Rng::new(seed(q));
            Market {
                id: format!("sample-{i}"),
                question: (*q).into(),
                slug: format!("sample-{i}"),
                event: None,
                category: Some((*cat).into()),
                outcomes: vec!["Yes".into(), "No".into()],
                prices: vec![*p, 1.0 - p],
                tokens: vec![format!("y{i}"), format!("n{i}")],
                volume: 1e6 * (1.0 + 20.0 * r.f()),
                volume_24h: 1e5 * (1.0 + 30.0 * r.f()) / (i as f64 + 1.0),
                liquidity: 1e5 * (1.0 + 5.0 * r.f()),
                end_date: Some("2026-12-31".into()),
                best_bid: Some(p - 0.01),
                best_ask: Some(p + 0.01),
                change_1d: Some(r.n() * 0.03),
            }
        })
        .collect()
}

pub fn detail(m: &Market, now: i64) -> polymarket::Detail {
    let mut r = Rng::new(seed(&m.id));
    let p = m.prices.first().copied().unwrap_or(0.5);
    let mut v = (p + r.n() * 0.1).clamp(0.02, 0.98);
    let yes: Vec<(i64, f64)> = (0..168)
        .map(|i| {
            v = (v + r.n() * 0.012 + (p - v) * 0.03).clamp(0.01, 0.99);
            ((now / 3600 - 167 + i) * 3600, v)
        })
        .collect();
    let no = yes.iter().map(|&(t, y)| (t, 1.0 - y)).collect();
    let book = polymarket::Book {
        bids: (1..=8).map(|k| (((p - 0.01 * k as f64) * 100.0).round() / 100.0, 500.0 * r.f() * k as f64)).collect(),
        asks: (1..=8).map(|k| (((p + 0.01 * k as f64) * 100.0).round() / 100.0, 500.0 * r.f() * k as f64)).collect(),
    };
    polymarket::Detail { market_id: m.id.clone(), history: vec![yes, no], book: Some(book), error: None }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simulated_data_is_sane() {
        let b = board(1_760_936_400);
        assert!(b.simulated && b.rows.len() == COINS.len());
        assert_eq!(b.rows[0].coin, "BTC");
        assert!(b.rows.iter().all(|r| r.funding_8h.is_some() && r.oi_usd > 0.0 && r.venues.len() >= 4));
        assert_eq!(board(1_760_936_400), b, "deterministic");
        let h = history("ETH", 1_760_936_400);
        assert_eq!(h.oi_usd.len(), 168);
        let ts = term_structure("BTC", 1_760_936_400);
        assert!(ts.points[1..].windows(2).all(|w| w[0].expiry < w[1].expiry));
        let ms = markets();
        let d = detail(&ms[0], 1_760_936_400);
        assert!(d.history[0].iter().all(|p| (0.0..=1.0).contains(&p.1)));
        let mut r = Rng::new(1);
        let bk = book(&mut r, 100.0, 0.1, &[(99.0, 50.0)]);
        assert!(bk.best_bid().unwrap() < bk.best_ask().unwrap());
        assert!((0..1000).map(|_| r.f()).all(|x| (0.0..1.0).contains(&x)));
    }
}
