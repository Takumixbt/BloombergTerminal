//! The news wire: a real-time tape of tweets and exchange announcements (Tree of Alpha's
//! public feed, Binance, Bybit and OKX listing notices) merged with crypto and macro RSS/Atom
//! outlets, each headline tagged with the coins it mentions and what kind of story it is
//! (breaking, hack, regulation, ETF, listing, macro…), so the important ones stand out.
//!
//! Feeds are configurable (`news_feeds.json` in the data folder); [`DEFAULT_FEEDS`] is used
//! when that file does not exist.

use std::collections::{HashMap, HashSet, VecDeque};

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use crate::Http;

/// Where a headline came from; the wire colours and filters by it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Kind {
    /// News outlets.
    #[default]
    News,
    /// Posts on X.
    Social,
    /// Exchange announcements (listings, delistings, launches).
    Exchange,
    /// Central banks and regulators.
    Official,
}

impl Kind {
    pub const ALL: [Kind; 4] = [Kind::Social, Kind::Exchange, Kind::News, Kind::Official];

    pub fn label(self) -> &'static str {
        match self {
            Kind::News => "NEWS",
            Kind::Social => "X",
            Kind::Exchange => "EXCH",
            Kind::Official => "GOV",
        }
    }
}

/// How a feed is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Format {
    /// RSS 2.0 or Atom.
    #[default]
    Rss,
    /// `news.treeofalpha.com/api/news`: tweets and announcements as JSON.
    TreeOfAlpha,
    /// Bybit `v5/announcements/index`.
    Bybit,
    /// OKX `api/v5/support/announcements`.
    Okx,
    /// Binance's announcement list (`bapi/composite/v1/public/cms/article/list/query`).
    Binance,
}

impl Format {
    /// Seconds between polls: real-time sources often, outlets politely.
    pub fn interval_secs(self) -> u64 {
        match self {
            Format::TreeOfAlpha => 15,
            Format::Bybit | Format::Okx | Format::Binance => 60,
            Format::Rss => 90,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Feed {
    pub name: String,
    pub url: String,
    #[serde(default)]
    pub format: Format,
    /// Overrides the kind implied by the format (e.g. `Official` for a central bank's RSS).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<Kind>,
}

impl Feed {
    pub fn kind(&self) -> Kind {
        self.kind.unwrap_or(match self.format {
            Format::Bybit | Format::Okx | Format::Binance => Kind::Exchange,
            Format::Rss | Format::TreeOfAlpha => Kind::News,
        })
    }
}

/// Public feeds used when no `news_feeds.json` exists: real-time sources first.
pub const DEFAULT_FEEDS: [(&str, &str, Format, Option<Kind>); 14] = [
    ("Tree News", "https://news.treeofalpha.com/api/news?limit=100", Format::TreeOfAlpha, None),
    (
        "Binance",
        "https://www.binance.com/bapi/composite/v1/public/cms/article/list/query?type=1&pageNo=1&pageSize=20",
        Format::Binance,
        None,
    ),
    ("Bybit", "https://api.bybit.com/v5/announcements/index?locale=en-US&limit=20", Format::Bybit, None),
    ("OKX", "https://www.okx.com/api/v5/support/announcements", Format::Okx, None),
    ("CoinDesk", "https://www.coindesk.com/arc/outboundfeeds/rss/", Format::Rss, None),
    ("Cointelegraph", "https://cointelegraph.com/rss", Format::Rss, None),
    ("The Block", "https://www.theblock.co/rss.xml", Format::Rss, None),
    ("Decrypt", "https://decrypt.co/feed", Format::Rss, None),
    ("Blockworks", "https://blockworks.co/feed", Format::Rss, None),
    ("Bitcoin Magazine", "https://bitcoinmagazine.com/.rss/full/", Format::Rss, None),
    ("CryptoSlate", "https://cryptoslate.com/feed/", Format::Rss, None),
    ("Hyperliquid", "https://medium.com/feed/@hyperliquid", Format::Rss, None),
    ("Federal Reserve", "https://www.federalreserve.gov/feeds/press_all.xml", Format::Rss, Some(Kind::Official)),
    ("SEC", "https://www.sec.gov/news/pressreleases.rss", Format::Rss, Some(Kind::Official)),
];

pub fn default_feeds() -> Vec<Feed> {
    DEFAULT_FEEDS
        .iter()
        .map(|(n, u, format, kind)| Feed { name: (*n).into(), url: (*u).into(), format: *format, kind: *kind })
        .collect()
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
    Breaking,
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
            Tag::Breaking => "BREAKING",
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
        matches!(self, Tag::Breaking | Tag::Hack | Tag::Regulation | Tag::Etf | Tag::Listing | Tag::Macro)
    }

    const KEYWORDS: [(Tag, &'static [&'static str]); 9] = [
        (Tag::Breaking, &["breaking", "just in", "🚨", "urgent:"]),
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
        (
            Tag::Listing,
            &[
                "listing",
                "lists ",
                "listed",
                "will list",
                "delist",
                "launchpool",
                "airdrop",
                "tge",
                "perpetual contract",
                "pre-market",
            ],
        ),
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
    #[serde(default)]
    pub kind: Kind,
    /// Who posted it (X account, byline), when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
}

impl Headline {
    /// Build a headline, tagging coins and story kinds from `title` (plus any `extra_coins` the
    /// source already identified).
    pub fn new(
        title: String,
        source: &str,
        link: String,
        ts: i64,
        kind: Kind,
        extra_coins: &[String],
        universe: &HashSet<String>,
    ) -> Self {
        let mut coins: Vec<String> = Vec::new();
        for c in extra_coins.iter().cloned().chain(coins_in(&title, universe)) {
            if !coins.contains(&c) {
                coins.push(c);
            }
        }
        coins.truncate(6);
        let mut tags = classify(&title);
        if kind == Kind::Exchange && !tags.contains(&Tag::Listing) {
            let t = title.to_lowercase();
            if ["list", "launch", "perpetual", "pre-market", "futures"].iter().any(|w| t.contains(w)) {
                tags.push(Tag::Listing);
                tags.sort();
            }
        }
        Headline { title, source: source.to_string(), link, ts, coins, tags, kind, author: None }
    }

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
        .map(|e| {
            let mut h = Headline::new(e.title, source, e.link, e.ts, Kind::News, &[], universe);
            h.author = e.author;
            h
        })
        .collect())
}

/// A ticker-like coin symbol from a source's symbol field (`BTC`, `BTC_USDT`, `BTCUSDT`, `1000PEPE`).
fn coin_symbol(raw: &str) -> Option<String> {
    let s = raw.trim().to_ascii_uppercase();
    let s = s.split(['_', '-', '/', ':']).next().unwrap_or(&s).to_string();
    let s = ["USDT", "USDC", "PERP"].iter().find_map(|q| s.strip_suffix(q).filter(|b| b.len() >= 2)).unwrap_or(&s);
    let s = s.trim_start_matches("1000");
    (s.len() >= 2 && s.len() <= 10 && s.chars().all(|c| c.is_ascii_alphanumeric())).then(|| s.to_string())
}

fn ms_or_secs(v: &serde_json::Value) -> Option<i64> {
    let n = v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse::<f64>().ok()))?;
    Some(if n > 1e11 { (n / 1000.0) as i64 } else { n as i64 })
}

const EXCHANGES: [&str; 10] =
    ["binance", "bybit", "okx", "coinbase", "upbit", "bithumb", "kraken", "bitget", "robinhood", "hyperliquid"];

/// Tree of Alpha's public news API: an array of tweets (`title` = account, `body` = text,
/// `link`) and announcements/articles (`title`, `source`, `url`), with `time` in milliseconds
/// and `suggestions[].coin` for the coins it detected. Unknown or malformed items are skipped.
pub fn parse_tree(json: &str, universe: &HashSet<String>) -> Result<Vec<Headline>> {
    let items: Vec<serde_json::Value> = serde_json::from_str(json)?;
    fn s<'a>(v: &'a serde_json::Value, k: &str) -> Option<&'a str> {
        v.get(k).and_then(|x| x.as_str()).map(str::trim).filter(|x| !x.is_empty())
    }
    let mut out = Vec::new();
    for it in &items {
        let body = s(it, "body").map(clean);
        let title = s(it, "title").map(clean);
        let link = s(it, "url").or_else(|| s(it, "link")).map(str::to_string);
        let Some(ts) = it.get("time").and_then(ms_or_secs) else { continue };
        let tweet = it.get("info").and_then(|i| i.get("twitterId")).is_some()
            || link.as_deref().is_some_and(|l| l.contains("twitter.com/") || l.contains("x.com/"));
        let (text, author) = match (body.filter(|b| !b.is_empty()), title) {
            (Some(b), t) => (b, t),
            (None, Some(t)) => (s(it, "en").map(clean).unwrap_or(t), None),
            (None, None) => continue,
        };
        let source = s(it, "source").map(str::to_string);
        let src_l = source.as_deref().unwrap_or_default().to_lowercase();
        let kind = if tweet {
            Kind::Social
        } else if EXCHANGES.iter().any(|e| src_l.contains(e)) {
            Kind::Exchange
        } else {
            Kind::News
        };
        let mut coins: Vec<String> = it
            .get("suggestions")
            .and_then(|x| x.as_array())
            .into_iter()
            .flatten()
            .filter_map(|x| x.get("coin").and_then(|c| c.as_str()).and_then(coin_symbol))
            .collect();
        coins.extend(
            it.get("symbols")
                .and_then(|x| x.as_array())
                .into_iter()
                .flatten()
                .filter_map(|x| x.as_str().and_then(coin_symbol)),
        );
        let id = s(it, "_id").unwrap_or_default();
        let link = link.unwrap_or_else(|| format!("https://news.treeofalpha.com/#{id}"));
        let label = match kind {
            Kind::Social => "X".to_string(),
            _ => source.unwrap_or_else(|| "Tree News".into()),
        };
        let mut h = Headline::new(text, &label, link, ts, kind, &coins, universe);
        h.author = author;
        out.push(h);
    }
    Ok(out)
}

/// Bybit announcements (`result.list[]`: `title`, `url`, `type.key`, `publishTime`/`dateTimestamp` ms).
pub fn parse_bybit(json: &str, universe: &HashSet<String>) -> Result<Vec<Headline>> {
    let v: serde_json::Value = serde_json::from_str(json)?;
    if v.get("retCode").and_then(|c| c.as_i64()).is_some_and(|c| c != 0) {
        bail!("Bybit: {}", v.get("retMsg").and_then(|m| m.as_str()).unwrap_or("error"));
    }
    let list = v.pointer("/result/list").and_then(|l| l.as_array()).cloned().unwrap_or_default();
    Ok(list
        .iter()
        .filter_map(|a| {
            let title = clean(a.get("title")?.as_str()?);
            let url = a.get("url")?.as_str()?.to_string();
            let ts =
                a.get("publishTime").and_then(ms_or_secs).or_else(|| a.get("dateTimestamp").and_then(ms_or_secs))?;
            let mut h = Headline::new(title, "Bybit", url, ts, Kind::Exchange, &[], universe);
            let key = a.pointer("/type/key").and_then(|k| k.as_str()).unwrap_or_default();
            if (key.contains("new_crypto") || key.contains("delist")) && !h.tags.contains(&Tag::Listing) {
                h.tags.push(Tag::Listing);
                h.tags.sort();
            }
            Some(h)
        })
        .collect())
}

/// OKX announcements (`data[].details[]`: `title`, `url`, `annType`, `pTime` ms as a string).
pub fn parse_okx(json: &str, universe: &HashSet<String>) -> Result<Vec<Headline>> {
    let v: serde_json::Value = serde_json::from_str(json)?;
    if v.get("code").and_then(|c| c.as_str()).is_some_and(|c| c != "0") {
        bail!("OKX: {}", v.get("msg").and_then(|m| m.as_str()).unwrap_or("error"));
    }
    let mut out = Vec::new();
    for page in v.get("data").and_then(|d| d.as_array()).into_iter().flatten() {
        for a in page.get("details").and_then(|d| d.as_array()).into_iter().flatten() {
            let (Some(title), Some(url), Some(ts)) = (
                a.get("title").and_then(|t| t.as_str()),
                a.get("url").and_then(|u| u.as_str()),
                a.get("pTime").and_then(ms_or_secs),
            ) else {
                continue;
            };
            let mut h = Headline::new(clean(title), "OKX", url.to_string(), ts, Kind::Exchange, &[], universe);
            let ann = a.get("annType").and_then(|t| t.as_str()).unwrap_or_default();
            if (ann.contains("listing") || ann.contains("delisting")) && !h.tags.contains(&Tag::Listing) {
                h.tags.push(Tag::Listing);
                h.tags.sort();
            }
            out.push(h);
        }
    }
    Ok(out)
}

/// Binance announcements (`data.catalogs[].articles[]`: `title`, `code`, `releaseDate` ms).
pub fn parse_binance(json: &str, universe: &HashSet<String>) -> Result<Vec<Headline>> {
    let v: serde_json::Value = serde_json::from_str(json)?;
    let mut out = Vec::new();
    for cat in v.pointer("/data/catalogs").and_then(|c| c.as_array()).into_iter().flatten() {
        let listing = cat.get("catalogName").and_then(|n| n.as_str()).is_some_and(|n| n.contains("isting"));
        for a in cat.get("articles").and_then(|a| a.as_array()).into_iter().flatten() {
            let (Some(title), Some(code), Some(ts)) = (
                a.get("title").and_then(|t| t.as_str()),
                a.get("code").and_then(|c| c.as_str()),
                a.get("releaseDate").and_then(ms_or_secs),
            ) else {
                continue;
            };
            let url = format!("https://www.binance.com/en/support/announcement/{code}");
            let mut h = Headline::new(clean(title), "Binance", url, ts, Kind::Exchange, &[], universe);
            if listing && !h.tags.contains(&Tag::Listing) {
                h.tags.push(Tag::Listing);
                h.tags.sort();
            }
            out.push(h);
        }
    }
    Ok(out)
}

/// Fetch and parse one feed.
pub async fn fetch_feed(http: &Http, feed: &Feed, universe: &HashSet<String>) -> Result<Vec<Headline>> {
    let body = http.get(&feed.url, &[]).await?;
    let mut items = match feed.format {
        Format::Rss => headlines(&body, &feed.name, universe)?,
        Format::TreeOfAlpha => parse_tree(&body, universe)?,
        Format::Bybit => parse_bybit(&body, universe)?,
        Format::Okx => parse_okx(&body, universe)?,
        Format::Binance => parse_binance(&body, universe)?,
    };
    if feed.format == Format::Rss {
        for h in &mut items {
            h.kind = feed.kind();
        }
    }
    Ok(items)
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
            kind: Kind::News,
            author: None,
        };
        let mut w = Wire::default();
        assert_eq!(w.merge(vec![h("a", "1", 10), h("b", "2", 30)]), 2);
        assert_eq!(w.merge(vec![h("A", "3", 20), h("c", "2", 40), h("d", "4", 25)]), 1, "same title or link skipped");
        let ts: Vec<i64> = w.items.iter().map(|x| x.ts).collect();
        assert_eq!(ts, [30, 25, 10]);
        let tmp = std::env::temp_dir().join(format!("hd-feeds-{}.json", std::process::id()));
        assert_eq!(load_feeds(&tmp).len(), DEFAULT_FEEDS.len());
        std::fs::write(
            &tmp,
            r#"[{"name":"Mine","url":"https://x/rss"},{"name":"T","url":"https://t","format":"tree_of_alpha"}]"#,
        )
        .unwrap();
        let feeds = load_feeds(&tmp);
        assert_eq!((feeds[0].name.as_str(), feeds[0].format, feeds[0].kind()), ("Mine", Format::Rss, Kind::News));
        assert_eq!(feeds[1].format, Format::TreeOfAlpha);
        let _ = std::fs::remove_file(tmp);
    }

    #[test]
    fn tree_of_alpha_tweets_and_announcements() {
        let json = r#"[
          {"title":"Watcher.Guru (@WatcherGuru)","body":"JUST IN: $BTC reclaims $70,000","icon":"https://pbs.twimg.com/x.jpg",
           "link":"https://twitter.com/WatcherGuru/status/1","time":1792432800123,"_id":"a1",
           "info":{"twitterId":"123","isReply":false},"suggestions":[{"coin":"BTC","found":["BTC"]}]},
          {"title":"Binance Will List Foo (FOO) with Seed Tag Applied","source":"Binance EN","url":"https://www.binance.com/en/support/announcement/x",
           "time":1792432700000,"symbols":["FOO_USDT","FOOUSDT"],"_id":"a2"},
          {"title":"Ethereum devs schedule upgrade","source":"Blogs","url":"https://blog.example/eth","time":1792432600000,"_id":"a3","en":"Ethereum devs schedule upgrade"},
          {"title":"no time"},
          42
        ]"#;
        let h = parse_tree(json, &uni()).unwrap();
        assert_eq!(h.len(), 3);
        assert_eq!((h[0].kind, h[0].source.as_str(), h[0].ts), (Kind::Social, "X", 1_792_432_800));
        assert_eq!(h[0].title, "JUST IN: $BTC reclaims $70,000");
        assert_eq!(h[0].author.as_deref(), Some("Watcher.Guru (@WatcherGuru)"));
        assert_eq!(h[0].coins, ["BTC"]);
        assert!(h[0].tags.contains(&Tag::Breaking) && h[0].important());
        assert_eq!((h[1].kind, h[1].coins.as_slice()), (Kind::Exchange, ["FOO".to_string()].as_slice()));
        assert!(h[1].tags.contains(&Tag::Listing));
        assert_eq!((h[2].kind, h[2].coins.as_slice()), (Kind::News, ["ETH".to_string()].as_slice()));
        assert!(parse_tree("{}", &uni()).is_err());
    }

    #[test]
    fn exchange_announcements() {
        let bybit = r#"{"retCode":0,"retMsg":"OK","result":{"total":2,"list":[
          {"title":"New Listing: FOOUSDT Perpetual Contract","description":"d","type":{"title":"New Listings","key":"new_crypto"},
           "tags":["Derivatives"],"url":"https://announcements.bybit.com/a","dateTimestamp":1792432800000,"publishTime":1792432805000},
          {"title":"Scheduled maintenance","type":{"key":"maintenance_updates"},"url":"https://announcements.bybit.com/b","dateTimestamp":1792430000000}]}}"#;
        let b = parse_bybit(bybit, &uni()).unwrap();
        assert_eq!((b.len(), b[0].ts, b[0].source.as_str()), (2, 1_792_432_805, "Bybit"));
        assert!(b[0].tags.contains(&Tag::Listing) && !b[1].tags.contains(&Tag::Listing));
        assert!(parse_bybit(r#"{"retCode":10001,"retMsg":"bad"}"#, &uni()).is_err());

        let okx = r#"{"code":"0","msg":"","data":[{"details":[
          {"annType":"announcements-new-listings","pTime":"1792432800000","title":"OKX to list SOL margin pairs","url":"https://www.okx.com/help/x"}],
          "totalPage":"1"}]}"#;
        let o = parse_okx(okx, &uni()).unwrap();
        assert_eq!((o[0].ts, o[0].coins.as_slice()), (1_792_432_800, ["SOL".to_string()].as_slice()));
        assert!(o[0].tags.contains(&Tag::Listing));

        let binance = r#"{"code":"000000","data":{"catalogs":[{"catalogId":48,"catalogName":"New Cryptocurrency Listing",
          "articles":[{"id":1,"code":"abc123","title":"Binance Will Add HYPE on Earn","releaseDate":1792432800000}]}]}}"#;
        let n = parse_binance(binance, &uni()).unwrap();
        assert_eq!(n[0].link, "https://www.binance.com/en/support/announcement/abc123");
        assert_eq!((n[0].kind, n[0].coins.as_slice()), (Kind::Exchange, ["HYPE".to_string()].as_slice()));
        assert!(n[0].tags.contains(&Tag::Listing));
    }

    #[test]
    fn coin_symbols() {
        assert_eq!(coin_symbol("btc_usdt").as_deref(), Some("BTC"));
        assert_eq!(coin_symbol("SOLUSDT").as_deref(), Some("SOL"));
        assert_eq!(coin_symbol("1000PEPE").as_deref(), Some("PEPE"));
        assert_eq!(coin_symbol("A"), None);
    }
}
