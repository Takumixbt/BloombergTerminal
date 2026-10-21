//! Deterministic simulated data for offline use (`HD_PROVIDER=sample`): every structure is
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

// ---- Hyperliquid, whale prints and options flow -------------------------------------

fn fake_address(r: &mut Rng) -> String {
    format!("0x{:016x}{:016x}{:08x}", r.next_u64(), r.next_u64(), r.next_u64() as u32)
}

/// A simulated leaderboard (300 wallets, largest first).
pub fn leaderboard() -> Vec<crate::hyperliquid::Leader> {
    use crate::hyperliquid::{Leader, Perf};
    let mut r = Rng::new(7);
    let names = ["whale.hl", "loracle", "degen-capital", "hlp-hunter", "funding-farmer"];
    let mut out: Vec<Leader> = (0..300)
        .map(|i| {
            let av = 8e7 * (-(i as f64) / 40.0).exp() * (0.6 + 0.8 * r.f());
            let perf = |r: &mut Rng, scale: f64| {
                let roi = r.n() * scale;
                Perf { pnl: av * roi, roi, volume: av * (1.0 + 20.0 * r.f()) * scale * 10.0 }
            };
            Leader {
                address: fake_address(&mut r),
                name: (i < names.len()).then(|| names[i].to_string()),
                account_value: av,
                day: perf(&mut r, 0.03),
                week: perf(&mut r, 0.08),
                month: perf(&mut r, 0.2),
                all_time: perf(&mut r, 1.2),
            }
        })
        .collect();
    out.sort_by(|a, b| b.account_value.total_cmp(&a.account_value));
    out
}

pub fn vaults() -> Vec<crate::hyperliquid::Vault> {
    let mut r = Rng::new(9);
    let names = [
        "Hyperliquidity Provider (HLP)",
        "Growi HF",
        "Elsewhere",
        "pmalt",
        "Systemic Strategies",
        "Bitcoin Momentum",
        "Funding Arb Vault",
        "MC Recovery Fund",
    ];
    names
        .iter()
        .enumerate()
        .map(|(i, n)| crate::hyperliquid::Vault {
            name: (*n).into(),
            address: fake_address(&mut r),
            leader: fake_address(&mut r),
            tvl: 3.8e8 / (i as f64 * 3.0 + 1.0),
            apr: 0.05 + r.n() * 0.2,
            closed: false,
        })
        .collect()
}

/// A simulated account for any address (deterministic per address).
pub fn account(address: &str) -> crate::hyperliquid::Account {
    use crate::hyperliquid::{Account, Position};
    let mut r = Rng::new(seed(address));
    let av = 2e4 * (r.f() * 9.0).exp();
    let n = 1 + (r.f() * 6.0) as usize;
    let positions = (0..n)
        .map(|_| {
            let (coin, px, _) = COINS[(r.f().powi(2) * COINS.len() as f64) as usize];
            let long = r.f() < 0.55;
            let lev = [2.0, 3.0, 5.0, 10.0, 20.0, 40.0][(r.f() * 6.0) as usize];
            let value = av * (0.2 + r.f() * 1.5);
            let entry = px * (1.0 + r.n() * 0.03);
            let size = value / px * if long { 1.0 } else { -1.0 };
            let upnl = (px - entry) * size;
            let liq = if long { px * (1.0 - 0.9 / lev) } else { px * (1.0 + 0.9 / lev) };
            Position {
                coin: coin.into(),
                size,
                entry,
                value,
                upnl,
                roe: upnl / (value / lev),
                liq_px: Some(liq),
                leverage: lev,
                cross: r.f() < 0.8,
                margin: value / lev,
                funding: r.n() * value * 0.0005,
            }
        })
        .collect::<Vec<_>>();
    let notional = positions.iter().map(|p| p.value).sum();
    Account {
        address: address.to_string(),
        account_value: av,
        margin_used: positions.iter().map(|p| p.margin).sum(),
        notional,
        withdrawable: av * 0.4,
        positions,
        time: chrono::Utc::now().timestamp_millis(),
    }
}

pub fn fills(address: &str, now_ms: i64) -> Vec<crate::hyperliquid::Fill> {
    let a = account(address);
    let mut r = Rng::new(seed(address) ^ 3);
    let mut out: Vec<crate::hyperliquid::Fill> = (0..40)
        .map(|k| {
            let p = &a.positions[k % a.positions.len()];
            let open = r.f() < 0.6;
            let side = if p.is_long() { "Long" } else { "Short" };
            crate::hyperliquid::Fill {
                coin: p.coin.clone(),
                price: p.mark() * (1.0 + r.n() * 0.01),
                size: p.size.abs() * 0.1 * (0.2 + r.f()),
                buy: p.is_long() == open,
                time: now_ms - (k as i64) * 1_800_000 - (r.f() * 900_000.0) as i64,
                dir: format!("{} {side}", if open { "Open" } else { "Close" }),
                closed_pnl: if open { 0.0 } else { r.n() * p.value * 0.01 },
                fee: p.value * 0.00004,
            }
        })
        .collect();
    out.sort_by_key(|f| std::cmp::Reverse(f.time));
    out
}

pub fn positioning(wallets: &[String]) -> crate::hyperliquid::Positioning {
    let accounts: Vec<_> = wallets.iter().map(|w| account(w)).collect();
    crate::hyperliquid::Positioning {
        wallets: accounts.len(),
        coins: crate::hyperliquid::aggregate(&accounts),
        updated: chrono::Utc::now().timestamp(),
        simulated: true,
        errors: 0,
    }
}

/// Simulated whale prints until the tape is dropped by everyone else.
pub async fn run_prints(tape: crate::prints::SharedTape) {
    let wallets: Vec<String> = leaderboard().into_iter().take(60).map(|l| l.address).collect();
    if let Ok(mut t) = tape.lock() {
        t.simulated = true;
        t.status = "simulated".into();
    }
    let mut r = Rng::new(11);
    while Arc::strong_count(&tape) > 1 {
        tokio::time::sleep(Duration::from_millis(300 + (r.f() * 1500.0) as u64)).await;
        let (coin, px, _) = COINS[(r.f().powi(2) * 8.0) as usize];
        let usd = crate::prints::MIN_USD * (r.f() * 5.0).exp();
        let pick = |r: &mut Rng| wallets[(r.f() * wallets.len() as f64) as usize].clone();
        let p = crate::prints::Print {
            coin: coin.into(),
            price: px,
            size: usd / px,
            usd,
            buy: r.f() < 0.5,
            ts: chrono::Utc::now().timestamp_millis(),
            taker: pick(&mut r),
            maker: pick(&mut r),
        };
        if let Ok(mut t) = tape.lock() {
            t.push(p);
        }
    }
}

/// Simulated Deribit option trades.
pub fn option_trades(now_ms: i64, n: usize, seed_v: u64) -> Vec<crate::options_flow::OptionTrade> {
    let mut r = Rng::new(seed_v);
    let now = now_ms / 1000;
    let expiries: Vec<i64> = [3, 10, 38, 66, 157, 248].iter().map(|d| (now / 86_400 + d) * 86_400 + 8 * 3600).collect();
    (0..n)
        .map(|k| {
            let btc = r.f() < 0.7;
            let (ccy, index, step) = if btc { ("BTC", 67_000.0, 1_000.0) } else { ("ETH", 2_560.0, 50.0) };
            let call = r.f() < 0.55;
            let strike = ((index * (1.0 + r.n() * 0.12)) / step).round() * step;
            let expiry = expiries[(r.f() * expiries.len() as f64) as usize];
            let block = r.f() < 0.06;
            let amount = if block { 50.0 + r.f() * 400.0 } else { 0.1 + (r.f() * 4.0).exp() };
            let moneyness = if call { index - strike } else { strike - index };
            let price = ((moneyness.max(0.0) / index) + 0.004 + 0.02 * r.f()).max(0.0005);
            crate::options_flow::OptionTrade {
                id: format!("sim-{seed_v}-{k}"),
                ts: now_ms - (k as i64) * 4_000,
                instrument: format!(
                    "{ccy}-{}-{strike:.0}-{}",
                    chrono::DateTime::from_timestamp(expiry, 0)
                        .map(|d| d.format("%-d%b%y").to_string().to_uppercase())
                        .unwrap_or_default(),
                    if call { "C" } else { "P" }
                ),
                currency: ccy.into(),
                expiry,
                strike,
                call,
                buy: r.f() < 0.52,
                amount,
                price,
                index,
                iv: Some(0.4 + r.f() * 0.25),
                block,
            }
        })
        .collect()
}

pub async fn run_option_flow(tape: Arc<std::sync::Mutex<crate::options_flow::Tape>>) {
    let now = chrono::Utc::now().timestamp_millis();
    if let Ok(mut t) = tape.lock() {
        t.simulated = true;
        t.merge(option_trades(now, 600, 1));
    }
    let mut k = 2;
    while Arc::strong_count(&tape) > 1 {
        tokio::time::sleep(Duration::from_secs(3)).await;
        k += 1;
        if let Ok(mut t) = tape.lock() {
            t.merge(option_trades(chrono::Utc::now().timestamp_millis(), 1, k));
        }
    }
}

// ---- News, KOL calls, ecosystem, market dashboard ----------------------------------

/// Simulated headlines. Sources and accounts are fictional so the demo never puts words in a
/// real outlet's mouth.
const SAMPLE_NEWS: [(crate::news::Kind, &str, &str); 24] = {
    use crate::news::Kind::{Exchange, News, Official, Social};
    [
        (Social, "Sim Wire (@sim_wire)", "JUST IN: $BTC spot ETFs record $640M of net inflows today"),
        (Exchange, "CEX Notices", "New listing: HYPEUSDT perpetual contract with up to 25x leverage"),
        (News, "Chain Daily", "Hyperliquid HIP-3 markets top $1B in open interest as builders add equities"),
        (Official, "Central Bank", "FOMC statement: Committee holds the target range, signals patience on cuts"),
        (Social, "Sim Flows (@sim_flows)", "Whale opens $48M $ETH long on Hyperliquid at 2,561"),
        (News, "Block Report", "DeFi lending protocol drained of $12M in oracle exploit"),
        (Official, "Regulator", "Regulator delays decision on Solana ETF applications"),
        (Social, "Sim Macro (@sim_macro)", "BREAKING: US CPI comes in at 2.4% vs 2.6% expected"),
        (Exchange, "CEX Notices", "Notice on delisting of three spot trading pairs"),
        (News, "Ledger Times", "Ethena raises $100M at a $3B valuation"),
        (Social, "Sim Wire (@sim_wire)", "$SOL network upgrade goes live, fees down 40%"),
        (News, "Chain Daily", "Ether options traders pile into December $3,000 calls"),
        (Exchange, "DEX Notices", "Pre-market trading opens for a new token, launchpool starts tomorrow"),
        (Social, "Sim Flows (@sim_flows)", "$XRP open interest falls 12% as funding flips negative"),
        (News, "Block Report", "Miners send 15,000 BTC to exchanges in a week"),
        (Social, "Sim Wire (@sim_wire)", "JUST IN: $DOGE jumps 9% as meme coins rally"),
        (News, "Ledger Times", "Stablecoin supply reaches a record $310B"),
        (Social, "Sim Macro (@sim_macro)", "Treasury yields slip after weaker jobs report"),
        (Exchange, "CEX Notices", "Will list $LINK and $AVAX margin pairs"),
        (Social, "Sim Flows (@sim_flows)", "$PEPE whale moves 2T tokens to an exchange"),
        (News, "Chain Daily", "HyperEVM upgrade lowers gas and adds new precompiles"),
        (Official, "Regulator", "Regulator charges promoters of unregistered crypto offering"),
        (Social, "Sim Wire (@sim_wire)", "BREAKING: $ETH ETF options approved"),
        (News, "Block Report", "Bitcoin climbs past $67K as ETF inflows hit two-week high"),
    ]
};

fn sample_headline(k: usize, ts: i64, uni: &std::collections::HashSet<String>) -> crate::news::Headline {
    let (kind, who, text) = SAMPLE_NEWS[k % SAMPLE_NEWS.len()];
    let social = kind == crate::news::Kind::Social;
    let source = if social { "X" } else { who };
    let mut h = crate::news::Headline::new(
        text.into(),
        source,
        format!("https://example.invalid/news/{k}"),
        ts,
        kind,
        &[],
        uni,
    );
    h.author = social.then(|| who.to_string());
    h
}

/// The wire's opening batch: the last few hours, newest first.
pub fn news(now: i64) -> Vec<crate::news::Headline> {
    let uni: std::collections::HashSet<String> = COINS.iter().map(|c| c.0.to_string()).collect();
    (0..14).map(|k| sample_headline(k + 10, now - 90 - k as i64 * 640, &uni)).collect()
}

/// Drip a new simulated headline onto the wire every ~20 s so the tape feels live offline.
pub async fn run_wire(wire: Arc<std::sync::Mutex<crate::news::Wire>>) {
    let uni: std::collections::HashSet<String> = COINS.iter().map(|c| c.0.to_string()).collect();
    let mut k = 0usize;
    while Arc::strong_count(&wire) > 1 {
        tokio::time::sleep(Duration::from_secs(if k < 2 { 6 } else { 20 })).await;
        if let Ok(mut w) = wire.lock() {
            let mut h = sample_headline(k, chrono::Utc::now().timestamp(), &uni);
            // Unique link per drip so repeats of the list still land on the wire.
            h.link = format!("https://example.invalid/news/live-{k}");
            h.title =
                if k >= SAMPLE_NEWS.len() { format!("{} ({})", h.title, k / SAMPLE_NEWS.len() + 1) } else { h.title };
            w.merge(vec![h]);
        }
        k += 1;
    }
}

pub fn kol_posts(now: i64) -> Vec<crate::kol::Post> {
    let posts = [
        ("sample_trader", "Longed $BTC at 66.8k, invalidation 65.9k, targets 69k / 72k"),
        ("perp_wizard", "Shorting $SOL into resistance 158, stop 162"),
        ("hype_maxi", "Bought more $HYPE here, bullish into the HIP-3 catalyst"),
        ("macro_mike", "Fading this $ETH pump, bearish below 2.6k"),
        ("degen_dan", "$DOGE send it, longing 0.118"),
        ("sample_trader", "Took profit on BTC, flat now"),
        ("perp_wizard", "Long $LINK 11.2, target 13"),
        ("chart_nun", "$XRP looks heavy, shorting the retest of 0.55"),
    ];
    posts
        .iter()
        .enumerate()
        .map(|(k, (h, t))| crate::kol::Post {
            id: format!("sim-{k}"),
            handle: (*h).into(),
            text: (*t).into(),
            ts: now - k as i64 * 5_400,
            url: format!("https://x.com/{h}"),
        })
        .collect()
}

pub fn eco(now: i64) -> crate::hl_eco::Eco {
    use crate::hl_eco::{Dex, Eco, Market, Revenue, SpotToken};
    let mut r = Rng::new(17);
    let core_markets: Vec<Market> = COINS
        .iter()
        .map(|(c, px, oi)| Market {
            name: (*c).into(),
            price: *px,
            change_24h: Some(r.n() * 0.03),
            volume_usd: oi * 1e9 * 0.18 * (1.0 + r.f()),
            oi_usd: oi * 1e9 * 0.18,
            funding: Some(0.0000125 + r.n() * 0.00001),
        })
        .collect();
    let dex = |name: &str, full: &str, tick: &[(&str, f64)], r: &mut Rng| Dex {
        name: name.into(),
        full_name: full.into(),
        deployer: fake_address(r),
        markets: tick
            .iter()
            .map(|(t, px)| Market {
                name: format!("{name}:{t}"),
                price: *px,
                change_24h: Some(r.n() * 0.02),
                volume_usd: 5e6 * (1.0 + 10.0 * r.f()),
                oi_usd: 2e6 * (1.0 + 30.0 * r.f()),
                funding: Some(0.00001),
            })
            .collect(),
    };
    let hip3 = vec![
        dex("xyz", "XYZ Equities", &[("TSLA", 251.0), ("NVDA", 182.0), ("AAPL", 247.0), ("SPX", 6_650.0)], &mut r),
        dex("flx", "Felix Markets", &[("GOLD", 4_050.0), ("SILVER", 48.0), ("OIL", 61.0)], &mut r),
        dex("vntl", "Ventuals", &[("SPACEX", 420.0), ("OPENAI", 650.0)], &mut r),
    ];
    let spot = [
        ("HYPE", 24.5, 3.3e8),
        ("PURR", 0.21, 6e8),
        ("UBTC", 67_000.0, 4_200.0),
        ("UETH", 2_560.0, 52_000.0),
        ("FEUSD", 1.0, 9e7),
        ("HFUN", 18.0, 8e5),
    ]
    .iter()
    .map(|(n, p, sup)| SpotToken {
        name: (*n).into(),
        price: *p,
        change_24h: Some(r.n() * 0.04),
        volume_usd: p * sup * 0.01,
        market_cap: p * sup,
    })
    .collect::<Vec<_>>();
    let mut spot = spot;
    spot.sort_by(|a, b| b.market_cap.total_cmp(&a.market_cap));
    let series = |base: f64, r: &mut Rng| -> Revenue {
        let daily: Vec<(i64, f64)> =
            (0..120).map(|d| ((now / 86_400 - 119 + d) * 86_400, base * (0.7 + 0.6 * r.f()))).collect();
        let sum = |n: usize| daily.iter().rev().take(n).map(|x| x.1).sum::<f64>();
        Revenue { total_24h: Some(sum(1)), total_7d: Some(sum(7)), total_30d: Some(sum(30)), daily }
    };
    let fees = series(2.4e6, &mut r);
    let revenue = series(2.1e6, &mut r);
    Eco {
        core: Dex {
            name: String::new(),
            full_name: "Hyperliquid".into(),
            deployer: String::new(),
            markets: core_markets,
        },
        hip3,
        spot,
        hlp: vaults().into_iter().next(),
        fees: Some(fees),
        revenue: Some(revenue),
        errors: Vec::new(),
        updated: now,
        simulated: true,
    }
}

fn sample_bars(coin: &str, n: usize, now: i64) -> Vec<crate::market::Bar> {
    let mut r = Rng::new(seed(coin) ^ 23);
    let px = COINS.iter().find(|c| c.0 == coin).map_or(1.0, |c| c.1);
    let drift = r.n() * 0.0004;
    let mut p = px / (1.0 + drift).powi(n as i32);
    let start = (now / 3600 - n as i64) * 3600;
    (0..n)
        .map(|i| {
            let ts = start + i as i64 * 3600;
            let hour = (ts / 3600) % 24;
            // A mild US-session bias so seasonality shows structure.
            let bias = if (13..21).contains(&hour) { 0.0006 } else { -0.0002 };
            let o = p;
            p *= 1.0 + drift + bias + r.n() * 0.004;
            let v = px * 1e4 * (1.0 + r.f());
            crate::market::Bar {
                ts,
                open: o,
                close: p,
                volume: v,
                taker_buy: v * (0.5 + (p / o - 1.0) * 20.0).clamp(0.3, 0.7),
            }
        })
        .collect()
}

pub fn market(now: i64) -> crate::market::Dashboard {
    let rows = COINS
        .iter()
        .filter_map(|(c, _, _)| crate::market::row_from_bars(c, &sample_bars(c, 744, now)))
        .collect::<Vec<_>>();
    let mut d = crate::market::Dashboard { rows, updated: now, simulated: true, errors: 0 };
    crate::market::join_board(&mut d.rows, &board(now));
    d
}

pub fn seasonality(coin: &str, now: i64) -> crate::market::Seasonality {
    let mut s = crate::market::seasonality(coin, &sample_bars(coin, 3_000, now));
    s.simulated = true;
    s
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
        let lb = leaderboard();
        assert!(lb.windows(2).all(|w| w[0].account_value >= w[1].account_value));
        assert!(crate::hyperliquid::parse_address(&lb[0].address).is_some(), "valid addresses");
        let wallets: Vec<String> = lb.iter().take(50).map(|l| l.address.clone()).collect();
        let pos = positioning(&wallets);
        assert!(pos.coin("BTC").is_some_and(|c| !c.liq_levels.is_empty()));
        assert_eq!(account(&wallets[0]), account(&wallets[0]), "deterministic per address");
        let ot = option_trades(1_760_936_400_000, 50, 1);
        assert!(
            ot.iter().all(|t| crate::options_flow::parse_instrument(&t.instrument).is_some()),
            "{}",
            ot[0].instrument
        );
        assert!(!vaults().is_empty() && !fills(&wallets[0], 0).is_empty());
        assert!(news(1_760_936_400).iter().any(|h| h.important()));
        let uni: std::collections::HashSet<String> = COINS.iter().map(|c| c.0.to_string()).collect();
        let calls: Vec<_> = kol_posts(0).iter().flat_map(|p| crate::kol::calls_in(p, &uni)).collect();
        assert!(calls.len() >= 6, "{}", calls.len());
        let e = eco(1_760_936_400);
        assert!(e.hype().is_some() && e.hip3.len() == 3 && e.spot[0].name == "HYPE");
        let m = market(1_760_936_400);
        assert_eq!(m.rows.len(), COINS.len());
        assert!(m.rows.iter().all(|r| r.returns.iter().all(Option::is_some) && r.oi_usd > 0.0));
        let s = seasonality("BTC", 1_760_936_400);
        assert!(s.sessions[2].1 > s.sessions[0].1, "US-session bias shows");
    }
}
