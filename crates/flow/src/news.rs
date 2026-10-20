//! The news wire: headlines from many crypto and macro RSS/Atom feeds merged into one tape,
//! each tagged with the coins it mentions and what kind of story it is (hack, regulation,
//! ETF, listing, macro…), so the important ones stand out.
//!
//! Feeds are configurable (`news_feeds.json` in the data folder); [`DEFAULT_FEEDS`] is used
//! when that file does not exist.

use std::collections::{HashMap, HashSet, VecDeque};

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use crate::Http;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Feed {
    pub name: String,
    pub url: String,
}

/// Public feeds used when no `news_feeds.json` exists.
pub const DEFAULT_FEEDS: [(&str, &str); 10] = [
    ("CoinDesk", "https://www.coindesk.com/arc/outboundfeeds/rss/"),
    ("Cointelegraph", "https://cointelegraph.com/rss"),
    ("The Block", "https://www.theblock.co/rss.xml"),
    ("Decrypt", "https://decrypt.co/feed"),
    ("Blockworks", "https://blockworks.co/feed"),
    ("Bitcoin Magazine", "https://bitcoinmagazine.com/.rss/full/"),
    ("CryptoSlate", "https://cryptoslate.com/feed/"),
    ("Hyperliquid", "https://medium.com/feed/@hyperliquid"),
    ("Federal Reserve", "https://www.federalreserve.gov/feeds/press_all.xml"),
    ("SEC", "https://www.sec.gov/news/pressreleases.rss"),
];

pub fn default_feeds() -> Vec<Feed> {
    DEFAULT_FEEDS.iter().map(|(n, u)| Feed { name: (*n).into(), url: (*u).into() }).collect()
}

/// Read `news_feeds.json` (`[{"name","url"}]`), or the defaults when it is missing or invalid.
pub fn load_feeds(path: &std::path::Path) -> Vec<Feed> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str::<Vec<Feed>>(&s).ok())
        .filter(|f| !f.is_empty())
        .unwrap_or_else(default_feeds)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Tag {
    Hack,
    Regulation,
    Etf,
    Listing,
    Macro,
    Funding,
    Partnership,
    Hyperliquid,
}

impl Tag {
    pub fn label(self) -> &'static str {
        match self {
            Tag::Hack => "HACK",
            Tag::Regulation => "REG",
            Tag::Etf => "ETF",
            Tag::Listing => "LIST",
            Tag::Macro => "MACRO",
            Tag::Funding => "RAISE",
            Tag::Partnership => "DEAL",
            Tag::Hyperliquid => "HL",
        }
    }

    /// Stories that move markets get highlighted.
    pub fn important(self) -> bool {
        matches!(self, Tag::Hack | Tag::Regulation | Tag::Etf | Tag::Listing | Tag::Macro)
    }

    const KEYWORDS: [(Tag, &'static [&'static str]); 8] = [
        (Tag::Hack, &["hack", "exploit", "drained", "stolen", "breach", "attacker", "rug pull", "rugpull", "phishing"]),
        (
            Tag::Regulation,
            &[
                "sec ",
                "sec's",
                "cftc",
                "regulat",
                "lawsuit",
                "court",
                "senate",
                "congress",
                "bill ",
                "ban ",
                "sanction",
                "doj",
                "indict",
                "enforcement",
            ],
        ),
        (Tag::Etf, &["etf", "exchange-traded"]),
        (Tag::Listing, &["listing", "lists ", "listed", "delist", "launchpool", "airdrop", "tge"]),
        (
            Tag::Macro,
            &[
                "fed ",
                "fomc",
                "rate cut",
                "rate hike",
                "inflation",
                "cpi",
                "jobs report",
                "payrolls",
                "powell",
                "treasury yield",
                "tariff",
                "recession",
            ],
        ),
        (Tag::Funding, &["raises", "funding round", "series a", "series b", "seed round", "valuation"]),
        (Tag::Partnership, &["partner", "acquire", "acquisition", "merger", "integrat"]),
        (Tag::Hyperliquid, &["hyperliquid", "hyperevm", "hip-3", "hip3", "$hype"]),
    ];
}

/// Tags for a headline, from keyword lists (case-insensitive).
pub fn classify(title: &str) -> Vec<Tag> {
    let t = format!(" {} ", title.to_lowercase());
    let mut tags: Vec<Tag> =
        Tag::KEYWORDS.iter().filter(|(_, ws)| ws.iter().any(|w| t.contains(w))).map(|(tag, _)| *tag).collect();
    tags.sort();
    tags
}

/// Coin names that headlines use instead of tickers.
const NAMES: [(&str, &str); 24] = [
    ("bitcoin", "BTC"),
    ("ether ", "ETH"),
    ("ethereum", "ETH"),
    ("solana", "SOL"),
    ("ripple", "XRP"),
    ("dogecoin", "DOGE"),
    ("hyperliquid", "HYPE"),
    ("cardano", "ADA"),
    ("chainlink", "LINK"),
    ("avalanche", "AVAX"),
    ("polkadot", "DOT"),
    ("litecoin", "LTC"),
    ("toncoin", "TON"),
    ("binance coin", "BNB"),
    ("tron", "TRX"),
    ("shiba inu", "SHIB"),
    ("pepe", "PEPE"),
    ("arbitrum", "ARB"),
    ("optimism", "OP"),
    ("aptos", "APT"),
    ("sui ", "SUI"),
    ("near protocol", "NEAR"),
    ("ethena", "ENA"),
    ("tether", "USDT"),
];

/// Tickers that are also common words or too ambiguous to match alone.
const AMBIGUOUS: [&str; 12] = ["ONE", "OP", "NEAR", "SUN", "GAS", "MAGIC", "BLUR", "ID", "IT", "S", "ME", "AI"];

/// Coins a headline mentions: `$TICKER` cashtags, known tickers as whole upper-case words
/// (from `universe`), and common coin names. Ambiguous short tickers count only as cashtags.
pub fn coins_in(title: &str, universe: &HashSet<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |c: &str| {
        if !out.iter().any(|x| x == c) {
            out.push(c.to_string());
        }
    };
    for raw in title.split(|c: char| !(c.is_ascii_alphanumeric() || c == '$')) {
        if let Some(tag) = raw.strip_prefix('$') {
            let t = tag.to_ascii_uppercase();
            if t.len() >= 2 && t.starts_with(|c: char| c.is_ascii_alphabetic()) {
                push(&t);
            }
        } else if raw.len() >= 2
            && raw.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
            && universe.contains(raw)
            && !AMBIGUOUS.contains(&raw)
        {
            push(raw);
        }
    }
    let lower = format!(" {} ", title.to_lowercase());
    for (name, coin) in NAMES {
        if lower.contains(name) {
            push(coin);
        }
    }
    out
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Headline {
    pub title: String,
    pub source: String,
    pub link: String,
    /// Unix seconds.
    pub ts: i64,
    pub coins: Vec<String>,
    pub tags: Vec<Tag>,
}

impl Headline {
    pub fn important(&self) -> bool {
        self.tags.iter().any(|t| t.important())
    }
}

fn tag<'a>(item: &'a str, name: &str) -> Option<&'a str> {
    let open = format!("<{name}");
    let mut from = 0;
    // Skip longer tag names that share the prefix (`<link` vs `<linkedin`).
    loop {
        let start = from + item[from..].find(&open)?;
        let after = &item[start + open.len()..];
        if after.starts_with(['>', ' ', '/', '\t', '\n', '\r']) {
            let gt = after.find('>')?;
            if after[..gt].ends_with('/') {
                return Some(&after[..gt]);
            }
            let body = &after[gt + 1..];
            let end = body.find(&format!("</{name}>"))?;
            return Some(body[..end].trim());
        }
        from = start + open.len();
    }
}

fn attr<'a>(tag_attrs: &'a str, name: &str) -> Option<&'a str> {
    let key = format!("{name}=\"");
    let start = tag_attrs.find(&key)? + key.len();
    let end = tag_attrs[start..].find('"')? + start;
    Some(&tag_attrs[start..end])
}

/// Decode CDATA, the common entities and strip inline HTML tags.
fn clean(raw: &str) -> String {
    let raw = raw.trim();
    let raw = raw.strip_prefix("<![CDATA[").and_then(|r| r.strip_suffix("]]>")).unwrap_or(raw);
    let mut s = String::with_capacity(raw.len());
    let mut in_tag = false;
    for c in raw.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => s.push(c),
            _ => {}
        }
    }
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&#039;", "'")
        .replace("&apos;", "'")
        .replace("&#8217;", "’")
        .replace("&#8216;", "‘")
        .replace("&#8220;", "“")
        .replace("&#8221;", "”")
        .replace("&#8211;", "–")
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn parse_date(s: &str) -> Option<i64> {
    let s = clean(s);
    chrono::DateTime::parse_from_rfc2822(&s)
        .or_else(|_| chrono::DateTime::parse_from_rfc3339(&s))
        .ok()
        .map(|d| d.timestamp())
}

/// One RSS item or Atom entry.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub title: String,
    pub link: String,
    /// Unix seconds (0 when the feed gives no date).
    pub ts: i64,
    pub author: Option<String>,
}

/// Entries of an RSS 2.0 or Atom feed.
pub fn parse_feed(xml: &str) -> Result<Vec<Entry>> {
    let (open, close) = if xml.contains("<item") {
        ("<item", "</item>")
    } else if xml.contains("<entry") {
        ("<entry", "</entry>")
    } else {
        bail!("not an RSS or Atom feed");
    };
    let mut out = Vec::new();
    for chunk in xml.split(open).skip(1) {
        if !chunk.starts_with(['>', ' ']) {
            continue;
        }
        let item = chunk.split(close).next().unwrap_or(chunk);
        let Some(title) = tag(item, "title").map(clean).filter(|t| !t.is_empty()) else { continue };
        let link = match tag(item, "link") {
            // Atom: <link href="…"/> (possibly several; prefer rel="alternate" or no rel).
            Some(l) if l.contains("href=") => attr(l, "href").map(str::to_string),
            Some(l) => Some(clean(l)),
            None => None,
        };
        let Some(link) = link.filter(|l| l.starts_with("http")) else { continue };
        let ts = ["pubDate", "published", "updated", "dc:date"]
            .iter()
            .find_map(|t| tag(item, t).and_then(parse_date))
            .unwrap_or(0);
        let author = tag(item, "dc:creator").or_else(|| tag(item, "name")).map(clean).filter(|a| !a.is_empty());
        out.push(Entry { title, link, ts, author });
    }
    Ok(out)
}

pub fn headlines(xml: &str, source: &str, universe: &HashSet<String>) -> Result<Vec<Headline>> {
    Ok(parse_feed(xml)?
        .into_iter()
        .map(|e| Headline {
            coins: coins_in(&e.title, universe),
            tags: classify(&e.title),
            title: e.title,
            source: source.to_string(),
            link: e.link,
            ts: e.ts,
        })
        .collect())
}

/// The merged wire, newest first, with per-feed status.
#[derive(Debug, Default)]
pub struct Wire {
    pub items: VecDeque<Headline>,
    seen: HashSet<String>,
    pub status: HashMap<String, Result<usize, String>>,
    pub simulated: bool,
}

impl Wire {
    pub const CAP: usize = 2_000;

    /// Merge headlines, skipping links (and identical titles) already on the wire.
    pub fn merge(&mut self, items: Vec<Headline>) -> usize {
        let mut added = 0;
        for h in items {
            let key_title = h.title.to_lowercase();
            if self.seen.contains(&h.link) || self.seen.contains(&key_title) {
                continue;
            }
            self.seen.insert(h.link.clone());
            self.seen.insert(key_title);
            let at = self.items.iter().position(|x| x.ts <= h.ts).unwrap_or(self.items.len());
            self.items.insert(at, h);
            added += 1;
        }
        while self.items.len() > Self::CAP {
            if let Some(h) = self.items.pop_back() {
                self.seen.remove(&h.link);
                self.seen.remove(&h.title.to_lowercase());
            }
        }
        added
    }
}

/// Fetch every feed concurrently.
pub async fn fetch_all(
    http: &Http,
    feeds: &[Feed],
    universe: &HashSet<String>,
) -> Vec<(String, Result<Vec<Headline>, String>)> {
    let futs = feeds.iter().map(|f| async move {
        let r = http.get(&f.url, &[]).await.and_then(|x| headlines(&x, &f.name, universe));
        (f.name.clone(), r.map_err(|e| format!("{e:#}")))
    });
    futures::future::join_all(futs).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uni() -> HashSet<String> {
        ["BTC", "ETH", "SOL", "HYPE", "OP", "XRP"].iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn rss_and_atom() {
        let rss = r#"<?xml version="1.0"?><rss><channel><title>Feed</title>
            <item><title><![CDATA[Bitcoin tops $70K as ETF inflows surge]]></title><link>https://ex.com/a</link>
              <pubDate>Mon, 19 Oct 2026 14:00:00 +0000</pubDate><dc:creator>Jane</dc:creator></item>
            <item><title>Hyperliquid &amp; HYPE: exploit drains $4M from vault</title><link>https://ex.com/b</link>
              <pubDate>Mon, 19 Oct 2026 15:30:00 GMT</pubDate></item>
            <item><title>No link</title></item>
        </channel></rss>"#;
        let h = headlines(rss, "Ex", &uni()).unwrap();
        assert_eq!(h.len(), 2);
        assert_eq!(h[0].coins, ["BTC"]);
        assert_eq!(h[0].tags, [Tag::Etf]);
        assert!(h[0].important());
        assert_eq!(h[1].title, "Hyperliquid & HYPE: exploit drains $4M from vault");
        assert!(h[1].coins.contains(&"HYPE".to_string()));
        assert_eq!(h[1].tags, [Tag::Hack, Tag::Hyperliquid]);
        let atom = r#"<feed xmlns="http://www.w3.org/2005/Atom"><title>A</title>
            <entry><title type="html">FOMC holds rates; Powell signals patience</title>
              <link rel="alternate" href="https://ex.com/c"/><updated>2026-10-19T18:00:00Z</updated></entry></feed>"#;
        let a = headlines(atom, "Fed", &uni()).unwrap();
        assert_eq!(a[0].link, "https://ex.com/c");
        assert_eq!(a[0].ts, 1_792_432_800);
        assert_eq!(a[0].tags, [Tag::Macro]);
        assert!(parse_feed("<html></html>").is_err());
    }

    #[test]
    fn coin_tagging() {
        let u = uni();
        assert_eq!(coins_in("$SOL and Ethereum rally; OP stack upgrade", &u), ["SOL", "ETH"], "OP alone is ambiguous");
        assert_eq!(coins_in("$OP pumps", &u), ["OP"]);
        assert!(coins_in("BTC-denominated", &u).contains(&"BTC".to_string()));
        assert!(coins_in("The best Ripple news", &u).contains(&"XRP".to_string()));
        assert!(coins_in("Fed speakers today", &u).is_empty());
    }

    #[test]
    fn wire_dedupes_and_orders() {
        let h = |t: &str, l: &str, ts| Headline {
            title: t.into(),
            source: "x".into(),
            link: l.into(),
            ts,
            coins: vec![],
            tags: vec![],
        };
        let mut w = Wire::default();
        assert_eq!(w.merge(vec![h("a", "1", 10), h("b", "2", 30)]), 2);
        assert_eq!(w.merge(vec![h("A", "3", 20), h("c", "2", 40), h("d", "4", 25)]), 1, "same title or link skipped");
        let ts: Vec<i64> = w.items.iter().map(|x| x.ts).collect();
        assert_eq!(ts, [30, 25, 10]);
        let tmp = std::env::temp_dir().join(format!("hd-feeds-{}.json", std::process::id()));
        assert_eq!(load_feeds(&tmp).len(), DEFAULT_FEEDS.len());
        std::fs::write(&tmp, r#"[{"name":"Mine","url":"https://x/rss"}]"#).unwrap();
        assert_eq!(load_feeds(&tmp)[0].name, "Mine");
        let _ = std::fs::remove_file(tmp);
    }
}
