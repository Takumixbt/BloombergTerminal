//! Hyperliquid trader intelligence from public, on-chain data: any wallet's positions and
//! fills, the leaderboard, vaults, and positioning aggregated over the top traders (long/short
//! exposure, size cohorts, liquidation levels from real positions, largest positions).
//!
//! Sources: `POST api.hyperliquid.xyz/info` (`clearinghouseState`, `userFills`) and the
//! leaderboard / vault lists the Hyperliquid app loads from `stats-data.hyperliquid.xyz`.
//! Positioning is computed from the wallets scanned (the top of the leaderboard by account
//! value), so it describes those traders, not the whole market.

use std::collections::BTreeMap;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{Http, int, num};

pub const INFO: &str = "https://api.hyperliquid.xyz/info";
pub const LEADERBOARD: &str = "https://stats-data.hyperliquid.xyz/Mainnet/leaderboard";
pub const VAULTS: &str = "https://stats-data.hyperliquid.xyz/Mainnet/vaults";

/// A 0x-prefixed 20-byte hex address (lower-cased), or `None`.
pub fn parse_address(s: &str) -> Option<String> {
    let s = s.trim();
    let hex = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X"))?;
    (hex.len() == 40 && hex.chars().all(|c| c.is_ascii_hexdigit())).then(|| format!("0x{}", hex.to_ascii_lowercase()))
}

/// `0x1234…abcd`.
pub fn short_address(a: &str) -> String {
    if a.len() > 12 { format!("{}…{}", &a[..6], &a[a.len() - 4..]) } else { a.to_string() }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Position {
    pub coin: String,
    /// Signed size in coins (negative = short).
    pub size: f64,
    pub entry: f64,
    /// Notional value in USD.
    pub value: f64,
    pub upnl: f64,
    /// Return on margin, as a fraction.
    pub roe: f64,
    pub liq_px: Option<f64>,
    pub leverage: f64,
    pub cross: bool,
    pub margin: f64,
    /// Funding paid since the position opened (positive = paid).
    pub funding: f64,
}

impl Position {
    pub fn is_long(&self) -> bool {
        self.size > 0.0
    }

    /// Mark price implied by value and size.
    pub fn mark(&self) -> f64 {
        if self.size != 0.0 { self.value / self.size.abs() } else { 0.0 }
    }

    /// Distance from the mark to the liquidation price, as a fraction of the mark.
    pub fn liq_distance(&self) -> Option<f64> {
        let (m, l) = (self.mark(), self.liq_px?);
        (m > 0.0).then(|| (m - l).abs() / m)
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Account {
    pub address: String,
    pub account_value: f64,
    pub margin_used: f64,
    pub notional: f64,
    pub withdrawable: f64,
    pub positions: Vec<Position>,
    /// Unix ms.
    pub time: i64,
}

impl Account {
    pub fn upnl(&self) -> f64 {
        self.positions.iter().map(|p| p.upnl).sum()
    }

    /// Net notional (long minus short), USD.
    pub fn net(&self) -> f64 {
        self.positions.iter().map(|p| p.value.copysign(p.size)).sum()
    }

    pub fn leverage(&self) -> f64 {
        if self.account_value > 0.0 { self.notional / self.account_value } else { 0.0 }
    }
}

/// `{"type":"clearinghouseState","user":…}`.
pub fn parse_clearinghouse(json: &str, address: &str) -> Result<Account> {
    let v: Value = serde_json::from_str(json).context("parsing clearinghouseState")?;
    let ms = &v["marginSummary"];
    let positions = v["assetPositions"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|ap| {
            let p = &ap["position"];
            let size = num(&p["szi"])?;
            (size != 0.0).then_some(())?;
            Some(Position {
                coin: p["coin"].as_str()?.to_string(),
                size,
                entry: num(&p["entryPx"]).unwrap_or(0.0),
                value: num(&p["positionValue"]).unwrap_or(0.0),
                upnl: num(&p["unrealizedPnl"]).unwrap_or(0.0),
                roe: num(&p["returnOnEquity"]).unwrap_or(0.0),
                liq_px: num(&p["liquidationPx"]).filter(|x| *x > 0.0),
                leverage: num(&p["leverage"]["value"]).unwrap_or(1.0),
                cross: p["leverage"]["type"] != "isolated",
                margin: num(&p["marginUsed"]).unwrap_or(0.0),
                funding: num(&p["cumFunding"]["sinceOpen"]).unwrap_or(0.0),
            })
        })
        .collect();
    Ok(Account {
        address: address.to_string(),
        account_value: num(&ms["accountValue"]).unwrap_or(0.0),
        margin_used: num(&ms["totalMarginUsed"]).unwrap_or(0.0),
        notional: num(&ms["totalNtlPos"]).unwrap_or(0.0),
        withdrawable: num(&v["withdrawable"]).unwrap_or(0.0),
        positions,
        time: int(&v["time"]).unwrap_or(0),
    })
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Fill {
    pub coin: String,
    pub price: f64,
    pub size: f64,
    pub buy: bool,
    /// Unix ms.
    pub time: i64,
    /// `Open Long`, `Close Short`, `Liquidated Isolated Long`, …
    pub dir: String,
    pub closed_pnl: f64,
    pub fee: f64,
}

impl Fill {
    pub fn notional(&self) -> f64 {
        self.price * self.size
    }
}

/// `{"type":"userFills","user":…}`: most recent first.
pub fn parse_fills(json: &str) -> Result<Vec<Fill>> {
    let v: Value = serde_json::from_str(json).context("parsing userFills")?;
    let mut fills: Vec<Fill> = v
        .as_array()
        .context("userFills: not an array")?
        .iter()
        .filter_map(|f| {
            Some(Fill {
                coin: f["coin"].as_str()?.to_string(),
                price: num(&f["px"])?,
                size: num(&f["sz"])?,
                buy: f["side"].as_str()? == "B",
                time: int(&f["time"])?,
                dir: f["dir"].as_str().unwrap_or_default().to_string(),
                closed_pnl: num(&f["closedPnl"]).unwrap_or(0.0),
                fee: num(&f["fee"]).unwrap_or(0.0),
            })
        })
        .collect();
    fills.sort_by_key(|f| std::cmp::Reverse(f.time));
    Ok(fills)
}

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Perf {
    pub pnl: f64,
    pub roi: f64,
    pub volume: f64,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Leader {
    pub address: String,
    pub name: Option<String>,
    pub account_value: f64,
    pub day: Perf,
    pub week: Perf,
    pub month: Perf,
    pub all_time: Perf,
}

impl Leader {
    pub fn label(&self) -> String {
        self.name.clone().unwrap_or_else(|| short_address(&self.address))
    }
}

/// `stats-data…/leaderboard`: `{"leaderboardRows":[{"ethAddress","accountValue","displayName",
/// "windowPerformances":[["day",{"pnl","roi","vlm"}],…]}]}`. Sorted by account value.
pub fn parse_leaderboard(json: &str) -> Result<Vec<Leader>> {
    let v: Value = serde_json::from_str(json).context("parsing leaderboard")?;
    let rows = v["leaderboardRows"].as_array().context("leaderboard: no rows")?;
    let mut out: Vec<Leader> = rows
        .iter()
        .filter_map(|r| {
            let mut l = Leader {
                address: parse_address(r["ethAddress"].as_str()?)?,
                name: r["displayName"].as_str().filter(|s| !s.is_empty()).map(str::to_string),
                account_value: num(&r["accountValue"]).unwrap_or(0.0),
                ..Default::default()
            };
            for w in r["windowPerformances"].as_array().into_iter().flatten() {
                let (Some(k), Some(p)) = (w.get(0).and_then(Value::as_str), w.get(1)) else { continue };
                let perf = Perf {
                    pnl: num(&p["pnl"]).unwrap_or(0.0),
                    roi: num(&p["roi"]).unwrap_or(0.0),
                    volume: num(&p["vlm"]).unwrap_or(0.0),
                };
                match k {
                    "day" => l.day = perf,
                    "week" => l.week = perf,
                    "month" => l.month = perf,
                    "allTime" => l.all_time = perf,
                    _ => {}
                }
            }
            Some(l)
        })
        .collect();
    out.sort_by(|a, b| b.account_value.total_cmp(&a.account_value));
    Ok(out)
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Vault {
    pub name: String,
    pub address: String,
    pub leader: String,
    pub tvl: f64,
    /// Trailing APR, as a fraction.
    pub apr: f64,
    pub closed: bool,
}

/// `stats-data…/vaults`: `[{"apr", "summary":{"name","vaultAddress","leader","tvl","isClosed"}}]`.
/// Open vaults by TVL.
pub fn parse_vaults(json: &str) -> Result<Vec<Vault>> {
    let v: Value = serde_json::from_str(json).context("parsing vaults")?;
    let mut out: Vec<Vault> = v
        .as_array()
        .context("vaults: not an array")?
        .iter()
        .filter_map(|r| {
            let s = &r["summary"];
            Some(Vault {
                name: s["name"].as_str()?.trim().to_string(),
                address: parse_address(s["vaultAddress"].as_str()?)?,
                leader: s["leader"].as_str().and_then(parse_address).unwrap_or_default(),
                tvl: num(&s["tvl"]).unwrap_or(0.0),
                apr: num(&r["apr"]).unwrap_or(0.0),
                closed: s["isClosed"].as_bool().unwrap_or(false),
            })
        })
        .filter(|v| !v.closed)
        .collect();
    out.sort_by(|a, b| b.tvl.total_cmp(&a.tvl));
    Ok(out)
}

// ---- Positioning ---------------------------------------------------------------------

/// Account-size cohorts, Hyperdash-style.
pub const COHORTS: [(&str, f64); 6] = [
    ("Shrimp <$10K", 0.0),
    ("Fish $10K–100K", 1e4),
    ("Dolphin $100K–1M", 1e5),
    ("Whale $1M–10M", 1e6),
    ("Leviathan $10M–50M", 1e7),
    ("Kraken $50M+", 5e7),
];

pub fn cohort_of(account_value: f64) -> usize {
    COHORTS.iter().rposition(|(_, lo)| account_value >= *lo).unwrap_or(0)
}

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Exposure {
    pub long_usd: f64,
    pub short_usd: f64,
    pub longs: usize,
    pub shorts: usize,
    pub upnl: f64,
}

impl Exposure {
    fn add(&mut self, p: &Position) {
        if p.is_long() {
            self.long_usd += p.value;
            self.longs += 1;
        } else {
            self.short_usd += p.value;
            self.shorts += 1;
        }
        self.upnl += p.upnl;
    }

    pub fn net(&self) -> f64 {
        self.long_usd - self.short_usd
    }

    /// Share of notional that is long.
    pub fn long_share(&self) -> f64 {
        let t = self.long_usd + self.short_usd;
        if t > 0.0 { self.long_usd / t } else { 0.5 }
    }
}

/// One coin's positioning across the scanned wallets.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct CoinPositioning {
    pub coin: String,
    pub total: Exposure,
    pub cohorts: Vec<Exposure>,
    /// Liquidation notional per price bucket: `(bucket_low, long_usd, short_usd)`, ascending.
    pub liq_levels: Vec<(f64, f64, f64)>,
    pub bucket: f64,
    /// Largest positions: `(address, account value, position)`, by notional.
    pub top: Vec<(String, f64, Position)>,
    /// Notional-weighted mark across positions.
    pub mark: f64,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Positioning {
    pub wallets: usize,
    pub coins: Vec<CoinPositioning>,
    /// Unix seconds.
    pub updated: i64,
    pub simulated: bool,
    pub errors: usize,
}

impl Positioning {
    pub fn coin(&self, coin: &str) -> Option<&CoinPositioning> {
        self.coins.iter().find(|c| c.coin.eq_ignore_ascii_case(coin))
    }
}

/// Aggregate scanned accounts per coin. Liquidation levels are bucketed at ≈0.5% of the price.
pub fn aggregate(accounts: &[Account]) -> Vec<CoinPositioning> {
    let mut by: BTreeMap<String, Vec<(&Account, &Position)>> = BTreeMap::new();
    for a in accounts {
        for p in &a.positions {
            by.entry(p.coin.clone()).or_default().push((a, p));
        }
    }
    let mut out: Vec<CoinPositioning> = by
        .into_iter()
        .map(|(coin, ps)| {
            let mut total = Exposure::default();
            let mut cohorts = vec![Exposure::default(); COHORTS.len()];
            let mut wsum = 0.0;
            let mut vsum = 0.0;
            for (a, p) in &ps {
                total.add(p);
                cohorts[cohort_of(a.account_value)].add(p);
                wsum += p.mark() * p.value;
                vsum += p.value;
            }
            let mark = if vsum > 0.0 { wsum / vsum } else { 0.0 };
            let bucket = crate::nice_step(mark * 0.005);
            let mut levels: BTreeMap<i64, (f64, f64)> = BTreeMap::new();
            for (_, p) in &ps {
                // Ignore levels far from the price (deeply overcollateralised positions).
                let Some(l) = p.liq_px.filter(|l| mark > 0.0 && (l / mark - 1.0).abs() < 0.5) else { continue };
                let e = levels.entry((l / bucket).floor() as i64).or_default();
                if p.is_long() {
                    e.0 += p.value;
                } else {
                    e.1 += p.value;
                }
            }
            let mut top: Vec<(String, f64, Position)> =
                ps.iter().map(|(a, p)| (a.address.clone(), a.account_value, (*p).clone())).collect();
            top.sort_by(|a, b| b.2.value.total_cmp(&a.2.value));
            top.truncate(50);
            CoinPositioning {
                coin,
                total,
                cohorts,
                liq_levels: levels.into_iter().map(|(k, (l, s))| (k as f64 * bucket, l, s)).collect(),
                bucket,
                top,
                mark,
            }
        })
        .collect();
    out.sort_by(|a, b| (b.total.long_usd + b.total.short_usd).total_cmp(&(a.total.long_usd + a.total.short_usd)));
    out
}

// ---- Network -------------------------------------------------------------------------

#[derive(Clone)]
pub struct Client {
    http: Http,
}

impl Client {
    pub fn new(http: Http) -> Self {
        Self { http }
    }

    pub async fn account(&self, address: &str) -> Result<Account> {
        let j = self.http.post_json(INFO, &json!({"type": "clearinghouseState", "user": address})).await?;
        parse_clearinghouse(&j, address)
    }

    pub async fn fills(&self, address: &str) -> Result<Vec<Fill>> {
        parse_fills(&self.http.post_json(INFO, &json!({"type": "userFills", "user": address})).await?)
    }

    pub async fn leaderboard(&self) -> Result<Vec<Leader>> {
        parse_leaderboard(&self.http.get(LEADERBOARD, &[]).await?)
    }

    pub async fn vaults(&self) -> Result<Vec<Vault>> {
        parse_vaults(&self.http.get(VAULTS, &[]).await?)
    }

    /// Scan the given wallets (a few at a time, to stay well inside the info endpoint's
    /// rate limit) and aggregate their positions.
    pub async fn positioning(&self, wallets: &[String]) -> Positioning {
        use futures::StreamExt;
        let results: Vec<Result<Account>> = futures::stream::iter(wallets.iter().cloned().map(|w| {
            let c = self.clone();
            async move { c.account(&w).await }
        }))
        .buffer_unordered(6)
        .collect()
        .await;
        let errors = results.iter().filter(|r| r.is_err()).count();
        let accounts: Vec<Account> = results.into_iter().flatten().collect();
        Positioning {
            wallets: accounts.len(),
            coins: aggregate(&accounts),
            updated: chrono::Utc::now().timestamp(),
            simulated: false,
            errors,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fx(name: &str) -> String {
        std::fs::read_to_string(format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
    }

    const ADDR: &str = "0x31ca8395cf837de08b24da3f660e77761dfb974b";

    #[test]
    fn addresses() {
        assert_eq!(parse_address(" 0x31CA8395cf837de08b24da3f660e77761dfb974b "), Some(ADDR.into()));
        assert_eq!(parse_address("0x1234"), None);
        assert_eq!(parse_address("31ca8395cf837de08b24da3f660e77761dfb974b"), None);
        assert_eq!(short_address(ADDR), "0x31ca…974b");
    }

    #[test]
    fn clearinghouse_state() {
        let a = parse_clearinghouse(&fx("hl_clearinghouse_state.json"), ADDR).unwrap();
        assert_eq!(a.account_value, 1_250_000.5);
        assert_eq!(a.positions.len(), 2, "zero-size positions dropped");
        let btc = &a.positions[0];
        assert!(btc.is_long() && btc.cross);
        assert_eq!((btc.leverage, btc.liq_px), (10.0, Some(58_120.0)));
        assert!((btc.mark() - 67_000.0).abs() < 1e-9);
        assert!((btc.liq_distance().unwrap() - (67_000.0 - 58_120.0) / 67_000.0).abs() < 1e-12);
        let eth = &a.positions[1];
        assert!(!eth.is_long() && !eth.cross);
        assert_eq!(eth.funding, -12.5);
        assert!((a.net() - (670_000.0 - 256_000.0)).abs() < 1e-6);
        assert!((a.upnl() - (22_000.0 - 3_000.0)).abs() < 1e-9);
    }

    #[test]
    fn fills_newest_first() {
        let f = parse_fills(&fx("hl_user_fills.json")).unwrap();
        assert_eq!(f.len(), 2);
        assert!(f[0].time > f[1].time);
        assert_eq!(f[0].dir, "Close Long");
        assert!(!f[0].buy);
        assert_eq!(f[0].closed_pnl, 1_520.25);
    }

    #[test]
    fn leaderboard_and_vaults() {
        let l = parse_leaderboard(&fx("hl_leaderboard.json")).unwrap();
        assert_eq!(l.len(), 2, "row with a bad address skipped");
        assert_eq!(l[0].account_value, 52_000_000.0, "sorted by account value");
        assert_eq!(l[0].label(), "whale.hl");
        assert_eq!(l[1].label(), "0x31ca…974b");
        assert_eq!(l[0].week.pnl, 1_250_000.0);
        assert_eq!(l[1].all_time.roi, 2.5);
        let v = parse_vaults(&fx("hl_vaults.json")).unwrap();
        assert_eq!(v.len(), 1, "closed vault skipped");
        assert_eq!(v[0].name, "Hyperliquidity Provider (HLP)");
        assert_eq!(v[0].apr, 0.112);
    }

    #[test]
    fn positioning_aggregates_cohorts_and_liq_levels() {
        let pos = |coin: &str, size: f64, value: f64, liq: f64| Position {
            coin: coin.into(),
            size,
            value,
            liq_px: Some(liq),
            ..Default::default()
        };
        let acct = |addr: &str, av: f64, ps: Vec<Position>| Account {
            address: addr.into(),
            account_value: av,
            positions: ps,
            ..Default::default()
        };
        let accounts = vec![
            acct("a", 2e6, vec![pos("BTC", 10.0, 670_000.0, 60_000.0), pos("ETH", -100.0, 256_000.0, 2_900.0)]),
            acct("b", 5e4, vec![pos("BTC", -1.0, 67_000.0, 70_000.0)]),
            acct("c", 6e7, vec![pos("BTC", 20.0, 1_340_000.0, 60_100.0)]),
        ];
        let p = aggregate(&accounts);
        assert_eq!(p[0].coin, "BTC", "largest exposure first");
        let btc = &p[0];
        assert_eq!((btc.total.longs, btc.total.shorts), (2, 1));
        assert_eq!(btc.total.long_usd, 2_010_000.0);
        assert_eq!(btc.cohorts[cohort_of(2e6)].long_usd, 670_000.0);
        assert_eq!(btc.cohorts[5].long_usd, 1_340_000.0, "Kraken cohort");
        assert_eq!(btc.cohorts[1].short_usd, 67_000.0, "Fish cohort");
        assert_eq!(btc.bucket, 500.0);
        // Both long liquidations land in the 60,000 bucket.
        assert!(btc.liq_levels.contains(&(60_000.0, 2_010_000.0, 0.0)));
        assert!(btc.liq_levels.contains(&(70_000.0, 0.0, 67_000.0)));
        assert_eq!(btc.top[0].0, "c");
        assert!((btc.total.long_share() - 2_010_000.0 / 2_077_000.0).abs() < 1e-12);
    }
}
