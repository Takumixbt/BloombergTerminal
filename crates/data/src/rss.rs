//! Minimal RSS 2.0 reader for headline feeds (title, link, pubDate). Used for crypto
//! news, which Yahoo's search endpoint does not index.

use anyhow::{Result, bail};

use crate::NewsItem;

/// CoinDesk's public feed.
pub const COINDESK: &str = "https://www.coindesk.com/arc/outboundfeeds/rss/";

fn tag<'a>(item: &'a str, name: &str) -> Option<&'a str> {
    let open = format!("<{name}");
    let start = item.find(&open)?;
    let body = &item[start + open.len()..];
    // Skip attributes; a self-closing tag has no text.
    let gt = body.find('>')?;
    if body[..gt].ends_with('/') {
        return None;
    }
    let body = &body[gt + 1..];
    let end = body.find(&format!("</{name}>"))?;
    Some(body[..end].trim())
}

fn text(raw: &str) -> String {
    let raw = raw.strip_prefix("<![CDATA[").and_then(|r| r.strip_suffix("]]>")).unwrap_or(raw);
    raw.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
        .trim()
        .to_string()
}

/// Parse `<item>`s into headlines (newest first), tagging the feed's publisher.
pub fn parse_rss(xml: &str, publisher: &str) -> Result<Vec<NewsItem>> {
    if !xml.contains("<rss") && !xml.contains("<channel") {
        bail!("not an RSS feed");
    }
    let mut out = Vec::new();
    for chunk in xml.split("<item>").skip(1) {
        let item = chunk.split("</item>").next().unwrap_or(chunk);
        let (Some(title), Some(link)) = (tag(item, "title"), tag(item, "link")) else { continue };
        let ts = tag(item, "pubDate")
            .and_then(|d| chrono::DateTime::parse_from_rfc2822(&text(d)).ok())
            .map_or(0, |d| d.timestamp());
        out.push(NewsItem {
            title: text(title),
            publisher: tag(item, "dc:creator")
                .map(text)
                .filter(|s| !s.is_empty())
                .map_or_else(|| publisher.to_string(), |a| format!("{publisher} · {a}")),
            link: text(link),
            ts,
            tickers: Vec::new(),
        });
    }
    out.sort_by_key(|a| std::cmp::Reverse(a.ts));
    Ok(out)
}

/// Keep headlines mentioning any of `words` (case-insensitive); all of them if none match.
pub fn filter_relevant(items: Vec<NewsItem>, words: &[&str]) -> Vec<NewsItem> {
    let hit = |n: &NewsItem| {
        let t = n.title.to_lowercase();
        words.iter().any(|w| t.contains(&w.to_lowercase()))
    };
    if items.iter().any(hit) { items.into_iter().filter(hit).collect() } else { items }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_coindesk_fixture() {
        let n = parse_rss(include_str!("../tests/fixtures/coindesk_rss.xml"), "CoinDesk").unwrap();
        assert_eq!(n.len(), 5);
        assert!(n.iter().all(|x| x.link.starts_with("https://www.coindesk.com/") && x.ts > 1_700_000_000));
        assert!(n.iter().all(|x| !x.title.contains("CDATA") && x.publisher.starts_with("CoinDesk")));
        assert!(n.windows(2).all(|w| w[0].ts >= w[1].ts));
        assert!(parse_rss("<html></html>", "x").is_err());
    }

    #[test]
    fn entities_and_filtering() {
        let xml = "<rss><channel><item><title>Bitcoin &amp; Ether rally</title><link>https://a/1</link>\
                   <pubDate>Mon, 12 Oct 2026 12:09:09 +0000</pubDate></item><item><title>Fed holds</title>\
                   <link>https://a/2</link></item></channel></rss>";
        let n = parse_rss(xml, "T").unwrap();
        assert_eq!(n[0].title, "Bitcoin & Ether rally");
        assert_eq!(filter_relevant(n.clone(), &["bitcoin"]).len(), 1);
        assert_eq!(filter_relevant(n, &["solana"]).len(), 2);
    }
}
