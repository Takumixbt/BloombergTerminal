//! Senate Electronic Financial Disclosures (efdsearch.senate.gov).
//!
//! Access requires accepting the site's prohibition agreement (a Django form
//! guarded by a CSRF token) in a cookie session. Searches go to
//! `/search/report/data/` (a DataTables JSON endpoint); report type 11 is the
//! Periodic Transaction Report. Electronic PTRs render as an HTML table at
//! `/search/view/ptr/{uuid}/`; paper PTRs live under `/search/view/paper/` and
//! are scanned images.

use std::sync::LazyLock;

use anyhow::{Context, Result, anyhow};
use chrono::NaiveDate;
use regex::Regex;
use scraper::{ElementRef, Html, Selector};
use serde::Deserialize;

use crate::model::{AmountRange, ParsedTrade, TxType, normalize_ticker, parse_us_date};

pub const BASE: &str = "https://efdsearch.senate.gov";
pub const HOME_URL: &str = "https://efdsearch.senate.gov/search/home/";
pub const SEARCH_URL: &str = "https://efdsearch.senate.gov/search/";
pub const DATA_URL: &str = "https://efdsearch.senate.gov/search/report/data/";
/// eFD report-type code for Periodic Transaction Reports.
pub const REPORT_TYPE_PTR: u32 = 11;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportKind {
    /// Electronically filed; transactions are in an HTML table.
    Electronic,
    /// Scanned paper filing; images only.
    Paper,
}

/// One row of search results.
#[derive(Debug, Clone, PartialEq)]
pub struct ReportRef {
    pub id: String,
    pub kind: ReportKind,
    pub first: String,
    pub last: String,
    pub office: String,
    pub title: String,
    pub date_received: Option<NaiveDate>,
    pub url: String,
}

impl ReportRef {
    pub fn is_amendment(&self) -> bool {
        self.title.contains("Amendment")
    }

    /// `First Last` with the trailing suffix in the last-name field moved to the end
    /// (`McConnell, Jr.` -> `McConnell Jr.`).
    pub fn display_name(&self) -> String {
        let first = title_case(self.first.trim());
        let last = title_case(&self.last.trim().replace(',', ""));
        format!("{first} {last}")
    }
}

/// eFD sometimes returns names in all caps (`RICHARD BLUMENTHAL`).
fn title_case(s: &str) -> String {
    if s.chars().any(|c| c.is_ascii_lowercase()) {
        return s.to_string();
    }
    s.split(' ')
        .map(|w| {
            let mut cs = w.chars();
            match cs.next() {
                Some(f) => f.to_uppercase().chain(cs.flat_map(char::to_lowercase)).collect(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[derive(Deserialize)]
struct SearchResponse {
    #[serde(rename = "recordsTotal", default)]
    records_total: u64,
    #[serde(default)]
    data: Vec<Vec<String>>,
    #[serde(default)]
    result: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SearchPage {
    pub total: u64,
    pub reports: Vec<ReportRef>,
}

static LINK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"href="(/search/view/(ptr|paper)/([0-9a-fA-F-]{36})/)"[^>]*>([^<]*)<"#).unwrap());

/// Parse the DataTables JSON returned by `/search/report/data/`.
pub fn parse_search_json(json: &str) -> Result<SearchPage> {
    let resp: SearchResponse = serde_json::from_str(json).context("parsing eFD search JSON")?;
    if !resp.result.is_empty() && resp.result != "ok" {
        return Err(anyhow!("eFD search returned result={}", resp.result));
    }
    let reports = resp
        .data
        .into_iter()
        .filter_map(|row| {
            let [first, last, office, link, date, ..] = row.as_slice() else { return None };
            let c = LINK.captures(link)?;
            Some(ReportRef {
                id: c[3].to_ascii_lowercase(),
                kind: if &c[2] == "ptr" { ReportKind::Electronic } else { ReportKind::Paper },
                first: first.trim().to_string(),
                last: last.trim().to_string(),
                office: office.trim().to_string(),
                title: c[4].trim().to_string(),
                date_received: parse_us_date(date),
                url: format!("{BASE}{}", &c[1]),
            })
        })
        .collect();
    Ok(SearchPage { total: resp.records_total, reports })
}

/// Form body for a PTR search page. `since` filters by submission date.
pub fn search_form(csrf: &str, start: u64, length: u64, since: Option<NaiveDate>) -> Vec<(String, String)> {
    let since = since.map(|d| d.format("%m/%d/%Y 00:00:00").to_string()).unwrap_or_default();
    [
        ("start", start.to_string()),
        ("length", length.to_string()),
        ("report_types", format!("[{REPORT_TYPE_PTR}]")),
        ("filer_types", "[]".into()),
        ("submitted_start_date", since),
        ("submitted_end_date", String::new()),
        ("candidate_state", String::new()),
        ("senator_state", String::new()),
        ("office_id", String::new()),
        ("first_name", String::new()),
        ("last_name", String::new()),
        ("order[0][column]", "4".into()),
        ("order[0][dir]", "desc".into()),
        ("csrfmiddlewaretoken", csrf.to_string()),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect()
}

/// The CSRF token embedded in the agreement form on the home page.
pub fn csrf_from_form(html: &str) -> Option<String> {
    static RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"name="csrfmiddlewaretoken"\s+value="([^"]+)""#).unwrap());
    RE.captures(html).map(|c| c[1].to_string())
}

/// Parsed electronic PTR page.
#[derive(Debug, Clone, PartialEq)]
pub struct SenatePtr {
    pub filer_name: Option<String>,
    pub filed_date: Option<NaiveDate>,
    pub trades: Vec<ParsedTrade>,
}

fn sel(s: &str) -> Selector {
    Selector::parse(s).expect("static selector")
}

fn text_of(el: ElementRef<'_>) -> String {
    el.text().collect::<Vec<_>>().join(" ").split_whitespace().collect::<Vec<_>>().join(" ")
}

fn non_empty(s: String) -> Option<String> {
    let t = s.trim();
    (!t.is_empty() && t != "--").then(|| t.to_string())
}

/// Parse the transaction table of `/search/view/ptr/{uuid}/`.
pub fn parse_ptr_html(html: &str) -> Result<SenatePtr> {
    let doc = Html::parse_document(html);
    let filer_name = doc.select(&sel("h2.filedReport")).next().map(|h| {
        // "The Honorable John Fetterman (Fetterman, John)"
        let t = text_of(h);
        let t = t.trim_start_matches("The Honorable").trim();
        t.split(" (").next().unwrap_or(t).trim().to_string()
    });
    let filed_date =
        doc.select(&sel("p.muted strong")).map(text_of).find(|t| t.contains("Filed")).and_then(|t| parse_us_date(&t));

    let table = doc
        .select(&sel("table.table"))
        .find(|t| t.select(&sel("th")).any(|th| text_of(th).contains("Transaction Date")))
        .ok_or_else(|| anyhow!("no transaction table in Senate PTR page"))?;
    let headers: Vec<String> = table.select(&sel("thead th")).map(text_of).collect();
    let col = |name: &str| headers.iter().position(|h| h.eq_ignore_ascii_case(name));
    let (c_num, c_date, c_owner, c_ticker, c_asset, c_atype, c_type, c_amount, c_comment) = (
        col("#"),
        col("Transaction Date"),
        col("Owner"),
        col("Ticker"),
        col("Asset Name"),
        col("Asset Type"),
        col("Type"),
        col("Amount"),
        col("Comment"),
    );
    let td_sel = sel("td");
    let muted = sel("div.text-muted");

    let mut trades = Vec::new();
    for tr in table.select(&sel("tbody tr")) {
        let cells: Vec<ElementRef> = tr.select(&td_sel).collect();
        let get = |c: Option<usize>| c.and_then(|i| cells.get(i).copied());
        let txt = |c: Option<usize>| get(c).map(text_of).unwrap_or_default();
        let Some(tx_date) = parse_us_date(&txt(c_date)) else { continue };
        // Asset name without the "Rate/Coupon / Matures" detail block.
        let asset_name = get(c_asset)
            .map(|td| {
                let detail: String = td.select(&muted).map(text_of).collect::<Vec<_>>().join(" ");
                let full = text_of(td);
                full.strip_suffix(&detail).unwrap_or(&full).trim().to_string()
            })
            .unwrap_or_default();
        let detail = get(c_asset).map(|td| td.select(&muted).map(text_of).collect::<Vec<_>>().join(" "));
        let comment = non_empty(txt(c_comment));
        let comment = match (comment, detail.and_then(non_empty)) {
            (Some(c), Some(d)) => Some(format!("{c}; {d}")),
            (c, d) => c.or(d),
        };
        trades.push(ParsedTrade {
            row: txt(c_num).parse().unwrap_or(trades.len() as u32 + 1),
            owner: non_empty(txt(c_owner)),
            ticker: normalize_ticker(&txt(c_ticker)),
            asset_name,
            asset_type: non_empty(txt(c_atype)),
            tx_type: TxType::parse(&txt(c_type)),
            tx_date,
            notification_date: None,
            amount: AmountRange::parse(&txt(c_amount)),
            comment,
        });
    }
    trades.sort_by_key(|t| t.row);
    Ok(SenatePtr { filer_name, filed_date, trades })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        let r = ReportRef {
            id: "x".into(),
            kind: ReportKind::Electronic,
            first: "RICHARD ".into(),
            last: "BLUMENTHAL".into(),
            office: String::new(),
            title: String::new(),
            date_received: None,
            url: String::new(),
        };
        assert_eq!(r.display_name(), "Richard Blumenthal");
        let r = ReportRef { first: "A. Mitchell".into(), last: "McConnell, Jr.".into(), ..r };
        assert_eq!(r.display_name(), "A. Mitchell McConnell Jr.");
    }

    #[test]
    fn form_has_report_type() {
        let f = search_form("tok", 0, 100, NaiveDate::from_ymd_opt(2026, 1, 2));
        assert!(f.contains(&("report_types".into(), "[11]".into())));
        assert!(f.contains(&("submitted_start_date".into(), "01/02/2026 00:00:00".into())));
    }
}
