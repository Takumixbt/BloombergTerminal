//! KOL calls: posts from a list of X (Twitter) accounts you choose, with the coin, direction
//! and price levels each "call" names, the price when it was first seen, and how it has done
//! since, rolled up into a per-account scorecard.
//!
//! X has no free read API. Posts come from either:
//! - the X API v2 recent search (`api.x.com/2/tweets/search/recent`), with your own bearer
//!   token in `X_BEARER_TOKEN` (requires a paid X API tier), or
//! - RSS/Atom bridges you run or subscribe to (RSSHub, Nitter, rss.app…), one URL per account.

use std::collections::HashSet;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::Http;

pub const X_SEARCH: &str = "https://api.x.com/2/tweets/search/recent";

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct KolSource {
    /// X handle without `@`.
    pub handle: String,
    /// Optional RSS/Atom URL serving this account's posts.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub rss: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Post {
    pub id: String,
    pub handle: String,
    pub text: String,
    /// Unix seconds.
    pub ts: i64,
    pub url: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Side {
    Long,
    Short,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Call {
    pub post: Post,
    pub coin: String,
    pub side: Side,
    /// Prices named in the post (entries, targets, stops).
    pub levels: Vec<f64>,
    /// Price when the call was first seen (from the derivatives board).
    pub price_at_call: Option<f64>,
}

impl Call {
    /// Move since the call in the call's direction (positive = the call is working).
    pub fn performance(&self, now_price: f64) -> Option<f64> {
        let p0 = self.price_at_call.filter(|p| *p > 0.0)?;
        let r = now_price / p0 - 1.0;
        Some(if self.side == Side::Long { r } else { -r })
    }
}

const LONG_WORDS: [&str; 16] = [
    "long",
    "longed",
    "longing",
    "longs",
    "bought",
    "buying",
    "buy",
    "bid",
    "bidding",
    "bullish",
    "accumulate",
    "accumulating",
    "send it",
    "moon",
    "breakout",
    "aped",
];
const SHORT_WORDS: [&str; 12] =
    ["short", "shorted", "shorting", "shorts", "sold", "selling", "sell", "bearish", "fade", "fading", "dump", "puts"];

/// Direction words found in `text`, as whole words.
fn side_of(text: &str) -> Option<Side> {
    let lower = text.to_lowercase();
    let words: Vec<&str> = lower.split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).collect();
    let has = |list: &[&str]| list.iter().any(|w| if w.contains(' ') { lower.contains(w) } else { words.contains(w) });
    match (has(&LONG_WORDS), has(&SHORT_WORDS)) {
        (true, false) => Some(Side::Long),
        (false, true) => Some(Side::Short),
        // Mixed or none: not a clear call.
        _ => None,
    }
}

/// Numbers that look like prices (`67k`, `$2,450`, `0.85`), excluding percentages and times.
pub fn levels_in(text: &str) -> Vec<f64> {
    let mut out = Vec::new();
    for raw in text.split(|c: char| c.is_whitespace() || matches!(c, '(' | ')' | '/' | ',' | ';')) {
        let cashtag = raw.starts_with('$') && raw[1..].starts_with(|c: char| c.is_ascii_alphabetic());
        if raw.ends_with('%') || raw.contains(':') || cashtag {
            continue;
        }
        let t = raw.trim_start_matches('$').trim_end_matches(['.', '!', '?']);
        let (num, mult) = match t.strip_suffix(['k', 'K']) {
            Some(n) => (n, 1e3),
            None => (t, 1.0),
        };
        if num.is_empty() || !num.chars().all(|c| c.is_ascii_digit() || c == '.') || num.matches('.').count() > 1 {
            continue;
        }
        if let Ok(v) = num.parse::<f64>()
            && v > 0.0
            && !(mult == 1.0 && v.fract() == 0.0 && (1..=10).contains(&(v as i64)))
        {
            out.push(v * mult);
        }
    }
    out
}

/// Turn a post into calls: one per coin it names (cashtags or known tickers) when the post has
/// a clear direction.
pub fn calls_in(post: &Post, universe: &HashSet<String>) -> Vec<Call> {
    let Some(side) = side_of(&post.text) else { return Vec::new() };
    let coins = crate::news::coins_in(&post.text, universe);
    let levels = levels_in(&post.text);
    coins
        .into_iter()
        .filter(|c| universe.is_empty() || universe.contains(c))
        .take(3)
        .map(|coin| Call { post: post.clone(), coin, side, levels: levels.clone(), price_at_call: None })
        .collect()
}

/// X API v2 recent search response: `data[]` (`id`, `text`, `created_at`, `author_id`) with
/// `includes.users[]` (`id`, `username`).
pub fn parse_x_search(json: &str) -> Result<Vec<Post>> {
    let v: Value = serde_json::from_str(json).context("parsing X search")?;
    if let Some(e) = v["errors"].as_array().and_then(|e| e.first()).filter(|_| v["data"].is_null()) {
        anyhow::bail!("X API: {}", e["message"].as_str().or(e["detail"].as_str()).unwrap_or("error"));
    }
    if let Some(t) = v["title"].as_str().filter(|_| v["data"].is_null()) {
        anyhow::bail!("X API: {t}: {}", v["detail"].as_str().unwrap_or(""));
    }
    let users: std::collections::HashMap<&str, &str> = v["includes"]["users"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|u| Some((u["id"].as_str()?, u["username"].as_str()?)))
        .collect();
    Ok(v["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|t| {
            let id = t["id"].as_str()?.to_string();
            let handle = users.get(t["author_id"].as_str()?).copied().unwrap_or("unknown").to_string();
            let ts = chrono::DateTime::parse_from_rfc3339(t["created_at"].as_str()?).ok()?.timestamp();
            Some(Post {
                url: format!("https://x.com/{handle}/status/{id}"),
                id,
                handle,
                text: t["text"].as_str()?.to_string(),
                ts,
            })
        })
        .collect())
}

/// Search queries covering `handles` (`from:a OR from:b …`), each within X's 512-character limit.
pub fn x_queries(handles: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur: Vec<String> = Vec::new();
    for h in handles {
        let term = format!("from:{}", h.trim_start_matches('@'));
        let len = cur.iter().map(|t| t.len() + 4).sum::<usize>() + term.len() + 20;
        if !cur.is_empty() && len > 480 {
            out.push(format!("({}) -is:retweet", cur.join(" OR ")));
            cur.clear();
        }
        cur.push(term);
    }
    if !cur.is_empty() {
        out.push(format!("({}) -is:retweet", cur.join(" OR ")));
    }
    out
}

/// Fetch recent posts from X (when a token is given) and the RSS bridges.
pub async fn fetch(http: &Http, token: Option<&str>, sources: &[KolSource]) -> (Vec<Post>, Vec<String>) {
    let mut posts = Vec::new();
    let mut errors = Vec::new();
    if let Some(token) = token.filter(|t| !t.trim().is_empty()) {
        let handles: Vec<String> = sources.iter().filter(|s| s.rss.is_empty()).map(|s| s.handle.clone()).collect();
        for q in x_queries(&handles) {
            let query = [
                ("query", q),
                ("max_results", "100".to_string()),
                ("tweet.fields", "created_at,author_id".to_string()),
                ("expansions", "author_id".to_string()),
                ("user.fields", "username".to_string()),
            ];
            match http.get_bearer(X_SEARCH, &query, token).await.and_then(|j| parse_x_search(&j)) {
                Ok(p) => posts.extend(p),
                Err(e) => errors.push(format!("X: {e:#}")),
            }
        }
    }
    for s in sources.iter().filter(|s| !s.rss.is_empty()) {
        match http.get(&s.rss, &[]).await.and_then(|x| crate::news::parse_feed(&x)) {
            Ok(items) => posts.extend(items.into_iter().map(|e| Post {
                id: e.link.clone(),
                handle: s.handle.trim_start_matches('@').to_string(),
                text: e.title,
                ts: e.ts,
                url: e.link,
            })),
            Err(e) => errors.push(format!("@{}: {e:#}", s.handle)),
        }
    }
    (posts, errors)
}

/// Per-account results over calls that have a price at the call and a current price.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Score {
    pub handle: String,
    pub calls: usize,
    pub wins: usize,
    /// Mean move in the call's direction.
    pub avg: f64,
}

impl Score {
    pub fn hit_rate(&self) -> f64 {
        if self.calls > 0 { self.wins as f64 / self.calls as f64 } else { 0.0 }
    }
}

pub fn scorecard(calls: &[Call], price: impl Fn(&str) -> Option<f64>) -> Vec<Score> {
    let mut by: std::collections::BTreeMap<String, Score> = Default::default();
    for c in calls {
        let Some(perf) = price(&c.coin).and_then(|p| c.performance(p)) else { continue };
        let s = by
            .entry(c.post.handle.clone())
            .or_insert_with(|| Score { handle: c.post.handle.clone(), ..Default::default() });
        s.calls += 1;
        s.wins += usize::from(perf > 0.0);
        s.avg += perf;
    }
    let mut v: Vec<Score> = by
        .into_values()
        .map(|mut s| {
            s.avg /= s.calls.max(1) as f64;
            s
        })
        .collect();
    v.sort_by(|a, b| b.avg.total_cmp(&a.avg));
    v
}

/// Calls kept across restarts (`kol_calls.json`), newest first, deduplicated by post and coin.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Book {
    pub calls: Vec<Call>,
}

impl Book {
    pub const CAP: usize = 3_000;

    pub fn load(path: &std::path::Path) -> Book {
        std::fs::read_to_string(path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    pub fn save(&self, path: &std::path::Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec(self)?)?;
        std::fs::rename(tmp, path)?;
        Ok(())
    }

    /// Add new calls, stamping each with the current price; returns how many were new.
    pub fn add(&mut self, calls: Vec<Call>, price: impl Fn(&str) -> Option<f64>) -> usize {
        let mut n = 0;
        for mut c in calls {
            if self.calls.iter().any(|x| x.post.id == c.post.id && x.coin == c.coin) {
                continue;
            }
            c.price_at_call = price(&c.coin);
            self.calls.push(c);
            n += 1;
        }
        self.calls.sort_by_key(|c| std::cmp::Reverse(c.post.ts));
        self.calls.truncate(Self::CAP);
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uni() -> HashSet<String> {
        ["BTC", "ETH", "SOL", "HYPE"].iter().map(|s| s.to_string()).collect()
    }

    fn post(text: &str) -> Post {
        Post { id: "1".into(), handle: "trader".into(), text: text.into(), ts: 100, url: String::new() }
    }

    #[test]
    fn detects_calls() {
        let c = calls_in(&post("Longed $SOL here at 152, targets 165 / 180k? stop 147. 5x"), &uni());
        assert_eq!(c.len(), 1);
        assert_eq!((c[0].coin.as_str(), c[0].side), ("SOL", Side::Long));
        assert_eq!(c[0].levels, [152.0, 165.0, 180_000.0, 147.0], "5x and small integers are not levels");
        assert_eq!(calls_in(&post("Shorting $HYPE and ETH, bearish"), &uni()).len(), 2);
        assert!(calls_in(&post("BTC long or short? idk"), &uni()).is_empty(), "mixed direction");
        assert!(calls_in(&post("gm, bullish on life"), &uni()).is_empty(), "no coin");
        assert!(levels_in("up 12% at 10:30 on $BTC").is_empty());
    }

    #[test]
    fn performance_and_scorecard() {
        let mut book = Book::default();
        let mut p1 = post("long $BTC");
        p1.id = "a".into();
        let mut p2 = post("short $ETH");
        p2.id = "b".into();
        p2.handle = "bear".into();
        let calls: Vec<Call> = [p1, p2].iter().flat_map(|p| calls_in(p, &uni())).collect();
        let at = |c: &str| Some(if c == "BTC" { 100.0 } else { 50.0 });
        assert_eq!(book.add(calls.clone(), at), 2);
        assert_eq!(book.add(calls, at), 0, "deduplicated");
        let now = |c: &str| Some(if c == "BTC" { 110.0 } else { 55.0 });
        let s = scorecard(&book.calls, now);
        assert_eq!(s[0].handle, "trader");
        assert!((s[0].avg - 0.1).abs() < 1e-12 && s[0].hit_rate() == 1.0);
        assert!((s[1].avg + 0.1).abs() < 1e-12 && s[1].wins == 0, "short call lost as ETH rose");
        let tmp = std::env::temp_dir().join(format!("hd-kol-{}.json", std::process::id()));
        book.save(&tmp).unwrap();
        assert_eq!(Book::load(&tmp).calls.len(), 2);
        let _ = std::fs::remove_file(tmp);
    }

    #[test]
    fn x_api() {
        let j = r#"{"data":[{"id":"1850000000000000001","text":"Long $BTC 67k, target 72k","created_at":"2026-10-19T14:00:00.000Z","author_id":"42","edit_history_tweet_ids":["1"]}],
            "includes":{"users":[{"id":"42","name":"Trader","username":"trader"}]},"meta":{"result_count":1}}"#;
        let p = parse_x_search(j).unwrap();
        assert_eq!(p[0].handle, "trader");
        assert_eq!(p[0].url, "https://x.com/trader/status/1850000000000000001");
        assert!(
            parse_x_search(r#"{"title":"Unauthorized","detail":"Unauthorized","type":"about:blank","status":401}"#)
                .is_err()
        );
        let handles: Vec<String> = (0..60).map(|i| format!("handle_number_{i}")).collect();
        let qs = x_queries(&handles);
        assert!(qs.len() > 1 && qs.iter().all(|q| q.len() <= 512));
        assert_eq!(qs.iter().map(|q| q.matches("from:").count()).sum::<usize>(), 60);
    }
}
