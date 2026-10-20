//! Hyperliquid at a glance (hl.eco-style): the core perps market (open interest, volume,
//! markets, HYPE), HIP-3 builder-deployed perp DEXs and their markets, spot tokens by market
//! cap, the HLP vault, and protocol fees and revenue.
//!
//! Sources: `POST api.hyperliquid.xyz/info` (`metaAndAssetCtxs` per DEX, `perpDexs`,
//! `spotMetaAndAssetCtxs`) and DefiLlama's free fees API (`api.llama.fi/summary/fees`).

use std::collections::HashMap;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::hyperliquid::{INFO, Vault};
use crate::{Http, int, num};

pub const LLAMA_FEES: &str = "https://api.llama.fi/summary/fees/hyperliquid";

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Market {
    pub name: String,
    pub price: f64,
    pub change_24h: Option<f64>,
    pub volume_usd: f64,
    pub oi_usd: f64,
    /// Hourly funding.
    pub funding: Option<f64>,
}

/// One perp DEX: the core one (`name` empty) or a HIP-3 builder DEX.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Dex {
    pub name: String,
    pub full_name: String,
    pub deployer: String,
    pub markets: Vec<Market>,
}

impl Dex {
    pub fn oi(&self) -> f64 {
        self.markets.iter().map(|m| m.oi_usd).sum()
    }

    pub fn volume(&self) -> f64 {
        self.markets.iter().map(|m| m.volume_usd).sum()
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct SpotToken {
    pub name: String,
    pub price: f64,
    pub change_24h: Option<f64>,
    pub volume_usd: f64,
    pub market_cap: f64,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Revenue {
    /// Daily `(unix secs, usd)`, oldest first.
    pub daily: Vec<(i64, f64)>,
    pub total_24h: Option<f64>,
    pub total_7d: Option<f64>,
    pub total_30d: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Eco {
    pub core: Dex,
    pub hip3: Vec<Dex>,
    pub spot: Vec<SpotToken>,
    pub hlp: Option<Vault>,
    pub fees: Option<Revenue>,
    pub revenue: Option<Revenue>,
    pub errors: Vec<String>,
    pub updated: i64,
    pub simulated: bool,
}

impl Eco {
    pub fn hype(&self) -> Option<&Market> {
        self.core.markets.iter().find(|m| m.name == "HYPE")
    }
}

/// `metaAndAssetCtxs` (any DEX) into markets, keeping names as given (`xyz:TSLA` on HIP-3).
pub fn parse_markets(json: &str) -> Result<Vec<Market>> {
    let v: Vec<Value> = serde_json::from_str(json).context("parsing metaAndAssetCtxs")?;
    let universe = v.first().and_then(|m| m["universe"].as_array()).context("no universe")?;
    let ctxs = v.get(1).and_then(Value::as_array).context("no asset contexts")?;
    Ok(universe
        .iter()
        .zip(ctxs)
        .filter(|(m, _)| m["isDelisted"].as_bool() != Some(true))
        .filter_map(|(m, c)| {
            let mark = num(&c["markPx"]).filter(|x| *x > 0.0)?;
            Some(Market {
                name: m["name"].as_str()?.to_string(),
                price: mark,
                change_24h: num(&c["prevDayPx"]).filter(|p| *p > 0.0).map(|p| mark / p - 1.0),
                volume_usd: num(&c["dayNtlVlm"]).unwrap_or(0.0),
                oi_usd: num(&c["openInterest"]).unwrap_or(0.0) * mark,
                funding: num(&c["funding"]),
            })
        })
        .collect())
}

/// `perpDexs`: `[null, {"name","fullName","deployer",…}, …]` (the first entry is the core DEX).
pub fn parse_perp_dexs(json: &str) -> Result<Vec<Dex>> {
    let v: Vec<Value> = serde_json::from_str(json).context("parsing perpDexs")?;
    Ok(v.iter()
        .filter_map(|d| {
            Some(Dex {
                name: d["name"].as_str()?.to_string(),
                full_name: d["fullName"].as_str().unwrap_or_default().to_string(),
                deployer: d["deployer"].as_str().unwrap_or_default().to_ascii_lowercase(),
                markets: Vec::new(),
            })
        })
        .collect())
}

/// `spotMetaAndAssetCtxs`: tokens and pairs, with contexts keyed by pair name (`PURR/USDC`,
/// `@107`). Market cap is circulating supply × mark.
pub fn parse_spot(json: &str) -> Result<Vec<SpotToken>> {
    let v: Vec<Value> = serde_json::from_str(json).context("parsing spotMetaAndAssetCtxs")?;
    let meta = v.first().context("no spot meta")?;
    let tokens: HashMap<i64, &str> = meta["tokens"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|t| Some((int(&t["index"])?, t["name"].as_str()?)))
        .collect();
    let pairs: HashMap<&str, &str> = meta["universe"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|u| {
            let base = int(u["tokens"].get(0)?)?;
            Some((u["name"].as_str()?, *tokens.get(&base)?))
        })
        .collect();
    let mut out: Vec<SpotToken> = v
        .get(1)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|c| {
            let base = *pairs.get(c["coin"].as_str()?)?;
            let price = num(&c["markPx"]).filter(|p| *p > 0.0)?;
            Some(SpotToken {
                name: base.to_string(),
                price,
                change_24h: num(&c["prevDayPx"]).filter(|p| *p > 0.0).map(|p| price / p - 1.0),
                volume_usd: num(&c["dayNtlVlm"]).unwrap_or(0.0),
                market_cap: num(&c["circulatingSupply"]).unwrap_or(0.0) * price,
            })
        })
        .collect();
    out.sort_by(|a, b| b.market_cap.total_cmp(&a.market_cap));
    Ok(out)
}

/// DefiLlama `summary/fees/<protocol>`: `totalDataChart` `[[unix secs, usd], …]` and totals.
pub fn parse_llama(json: &str) -> Result<Revenue> {
    let v: Value = serde_json::from_str(json).context("parsing DefiLlama fees")?;
    let mut daily: Vec<(i64, f64)> = v["totalDataChart"]
        .as_array()
        .context("no totalDataChart")?
        .iter()
        .filter_map(|p| Some((int(p.get(0)?)?, num(p.get(1)?)?)))
        .collect();
    daily.sort_by_key(|p| p.0);
    Ok(Revenue { daily, total_24h: num(&v["total24h"]), total_7d: num(&v["total7d"]), total_30d: num(&v["total30d"]) })
}

/// Fetch everything concurrently; parts that fail are listed in `errors`.
pub async fn fetch(http: &Http, vaults: &[Vault]) -> Eco {
    let info = |body: Value| http.post_json(INFO, &body);
    let (core, dexs, spot, fees, rev) = tokio::join!(
        info(json!({"type": "metaAndAssetCtxs"})),
        info(json!({"type": "perpDexs"})),
        info(json!({"type": "spotMetaAndAssetCtxs"})),
        http.get(LLAMA_FEES, &[("dataType", "dailyFees".into())]),
        http.get(LLAMA_FEES, &[("dataType", "dailyRevenue".into())]),
    );
    let mut eco = Eco { updated: chrono::Utc::now().timestamp(), ..Default::default() };
    let mut errors = Vec::new();
    match core.and_then(|j| parse_markets(&j)) {
        Ok(m) => {
            eco.core = Dex { name: String::new(), full_name: "Hyperliquid".into(), deployer: String::new(), markets: m }
        }
        Err(e) => errors.push(eco_err("core perps", e)),
    }
    match spot.and_then(|j| parse_spot(&j)) {
        Ok(s) => eco.spot = s,
        Err(e) => errors.push(eco_err("spot", e)),
    }
    eco.fees = fees.and_then(|j| parse_llama(&j)).map_err(|e| errors.push(eco_err("fees", e))).ok();
    eco.revenue = rev.and_then(|j| parse_llama(&j)).map_err(|e| errors.push(eco_err("revenue", e))).ok();
    match dexs.and_then(|j| parse_perp_dexs(&j)) {
        Ok(ds) => {
            let futs = ds.into_iter().map(|mut d| async move {
                let r = http.post_json(INFO, &json!({"type": "metaAndAssetCtxs", "dex": d.name})).await;
                if let Ok(m) = r.and_then(|j| parse_markets(&j)) {
                    d.markets = m;
                }
                d
            });
            let mut hip3 = futures::future::join_all(futs).await;
            hip3.sort_by(|a, b| b.oi().total_cmp(&a.oi()));
            eco.hip3 = hip3;
        }
        Err(e) => errors.push(eco_err("HIP-3 DEXs", e)),
    }
    eco.hlp = vaults.iter().find(|v| v.name.contains("HLP") || v.name.starts_with("Hyperliquidity Provider")).cloned();
    eco.errors = errors;
    eco
}

fn eco_err(what: &str, e: anyhow::Error) -> String {
    format!("{what}: {e:#}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spot_dexs_and_llama() {
        let spot = r#"[{"tokens":[{"name":"USDC","index":0},{"name":"PURR","index":1},{"name":"HYPE","index":150}],
            "universe":[{"tokens":[1,0],"name":"PURR/USDC","index":0},{"tokens":[150,0],"name":"@107","index":107}]},
            [{"coin":"PURR/USDC","markPx":"0.2","prevDayPx":"0.25","dayNtlVlm":"100000","circulatingSupply":"600000000"},
             {"coin":"@107","markPx":"25.0","prevDayPx":"24.0","dayNtlVlm":"9000000","circulatingSupply":"330000000"},
             {"coin":"@999","markPx":"1.0","prevDayPx":"1.0","dayNtlVlm":"0","circulatingSupply":"1"}]]"#;
        let s = parse_spot(spot).unwrap();
        assert_eq!(s.len(), 2, "unknown pair skipped");
        assert_eq!((s[0].name.as_str(), s[0].market_cap), ("HYPE", 8.25e9));
        assert!((s[1].change_24h.unwrap() + 0.2).abs() < 1e-12);
        let d = parse_perp_dexs(r#"[null,{"name":"xyz","fullName":"XYZ","deployer":"0xABC","oracleUpdater":null}]"#)
            .unwrap();
        assert_eq!((d.len(), d[0].name.as_str(), d[0].deployer.as_str()), (1, "xyz", "0xabc"));
        let m = parse_markets(
            r#"[{"universe":[{"name":"xyz:TSLA","szDecimals":2},{"name":"xyz:OLD","isDelisted":true}]},
              [{"markPx":"250.0","prevDayPx":"240.0","dayNtlVlm":"5000000","openInterest":"1000","funding":"0.00001"},
               {"markPx":"1.0","openInterest":"1"}]]"#,
        )
        .unwrap();
        assert_eq!(m.len(), 1);
        assert_eq!((m[0].name.as_str(), m[0].oi_usd), ("xyz:TSLA", 250_000.0));
        let dex = Dex { markets: m, ..Default::default() };
        assert_eq!((dex.oi(), dex.volume()), (250_000.0, 5e6));
        let r = parse_llama(r#"{"totalDataChart":[[1760918400,2100000.5],[1760832000,1900000]],"total24h":2100000.5,"total7d":14000000,"total30d":61000000}"#).unwrap();
        assert_eq!(r.daily[0].0, 1_760_832_000);
        assert_eq!(r.total_7d, Some(1.4e7));
    }
}
