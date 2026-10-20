//! Parser tests against real filings captured from the official sources.

use chrono::{Datelike, NaiveDate};
use tr_congress::Chamber;
use tr_congress::house::{self, PdfError};
use tr_congress::model::{AmountRange, Party, TxType};
use tr_congress::roster::{self, Roster};
use tr_congress::senate::{self, ReportKind};

const FIX: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");

fn read(p: &str) -> Vec<u8> {
    std::fs::read(format!("{FIX}/{p}")).unwrap_or_else(|e| panic!("{p}: {e}"))
}

fn text(p: &str) -> String {
    String::from_utf8(read(p)).unwrap()
}

fn d(y: i32, m: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, day).unwrap()
}

#[test]
fn house_index_xml() {
    let entries = house::parse_index_xml(&text("house/2026FD.xml")).unwrap();
    assert_eq!(entries.len(), 4);
    let ptrs: Vec<_> = entries.iter().filter(|e| e.is_ptr()).collect();
    assert_eq!(ptrs.len(), 3);
    let alford = entries.iter().find(|e| e.doc_id == "20034201").unwrap();
    assert_eq!(alford.display_name(), "Mark Alford");
    assert_eq!(alford.state_district, "MO04");
    assert_eq!(alford.state().as_deref(), Some("MO"));
    assert!(alford.filing_date.is_some());
    assert_eq!(alford.pdf_url(), "https://disclosures-clerk.house.gov/public_disc/ptr-pdfs/2026/20034201.pdf");
}

#[test]
fn house_pdf_multi_page_partial_sales() {
    let p = house::parse_ptr_pdf(&read("house/20034201.pdf")).unwrap();
    assert_eq!(p.filer_name.as_deref(), Some("Mark Alford"));
    assert_eq!(p.state_district.as_deref(), Some("MO04"));
    let tickers: Vec<_> = p.trades.iter().filter_map(|t| t.ticker.as_deref()).collect();
    for want in ["AMZN", "AAPL", "T", "BRK.B", "DIA"] {
        assert!(tickers.contains(&want), "missing {want} in {tickers:?}");
    }
    // A row spanning a page break must still be parsed once.
    assert_eq!(tickers.iter().filter(|t| **t == "BRK.B").count(), 1);
    let amzn = p.trades.iter().find(|t| t.ticker.as_deref() == Some("AMZN")).unwrap();
    assert_eq!(amzn.tx_type, TxType::SalePartial);
    assert_eq!(amzn.tx_date, d(2026, 3, 16));
    assert_eq!(amzn.notification_date, Some(d(2026, 3, 16)));
    assert_eq!(amzn.amount, Some(AmountRange { min: 1_001, max: Some(15_000) }));
    assert_eq!(amzn.asset_type.as_deref(), Some("ST"));
    assert!(amzn.comment.as_deref().unwrap_or("").contains("full transaction"));
    assert!(p.trades.iter().all(|t| t.tx_type.is_sell()));
}

#[test]
fn house_pdf_spouse_owner_and_wrapped_amount() {
    let p = house::parse_ptr_pdf(&read("house/20033751.pdf")).unwrap();
    assert_eq!(p.filer_name.as_deref(), Some("Richard W. Allen"));
    assert_eq!(p.trades.len(), 2);
    let ferg = &p.trades[0];
    assert_eq!(ferg.ticker.as_deref(), Some("FERG"));
    assert_eq!(ferg.owner.as_deref(), Some("Spouse"));
    assert_eq!(ferg.tx_type, TxType::Purchase);
    assert_eq!(ferg.tx_date, d(2025, 12, 12));
    assert_eq!(ferg.notification_date, Some(d(2026, 1, 6)));
    assert_eq!(ferg.amount, Some(AmountRange { min: 15_001, max: Some(50_000) }));
    assert_eq!(p.trades[1].ticker.as_deref(), Some("NFLX"));
    assert_eq!(p.trades[1].tx_type, TxType::Sale);
}

#[test]
fn house_pdf_open_ended_amount_and_treasuries() {
    let p = house::parse_ptr_pdf(&read("house/20033695.pdf")).unwrap();
    let over: Vec<_> = p.trades.iter().filter(|t| t.amount.is_some_and(|a| a.max.is_none())).collect();
    assert!(!over.is_empty());
    assert_eq!(over[0].amount.unwrap().min, 1_000_001);
    assert!(over.iter().all(|t| t.ticker.is_none() && t.asset_type.as_deref() == Some("GS")));
}

#[test]
fn house_pdf_multi_line_asset_names_and_large_amounts() {
    let p = house::parse_ptr_pdf(&read("house/20034653.pdf")).unwrap();
    let note = p.trades.iter().find(|t| t.asset_name.contains("Auto Callable")).unwrap();
    assert!(note.asset_name.starts_with("JP Morgan"), "{}", note.asset_name);
    assert_eq!(note.amount, Some(AmountRange { min: 5_000_001, max: Some(25_000_000) }));
}

#[test]
fn house_pdf_exchange_and_private_holdings() {
    let p = house::parse_ptr_pdf(&read("house/20034035.pdf")).unwrap();
    let wat = p.trades.iter().find(|t| t.ticker.as_deref() == Some("WAT")).unwrap();
    assert_eq!(wat.tx_type, TxType::Exchange);
    let p = house::parse_ptr_pdf(&read("house/20034213.pdf")).unwrap();
    assert_eq!(p.trades.len(), 10);
    assert_eq!(p.trades.iter().filter(|t| t.asset_name == "Andersen Corporation").count(), 7);
    assert!(p.trades.iter().all(|t| t.ticker.is_none()));
    assert_eq!(p.trades[9].asset_name, "StarCloud");
    assert_eq!(p.trades[9].tx_type, TxType::Purchase);
    assert_eq!(p.trades[0].tx_date, d(2025, 1, 20));
}

#[test]
fn house_pdf_every_sample_parses_rows() {
    for f in ["20033927", "20034201", "20033751", "20033695", "20034653", "20034035", "20034213"] {
        let p = house::parse_ptr_pdf(&read(&format!("house/{f}.pdf"))).unwrap();
        assert!(!p.trades.is_empty(), "{f}: no trades");
        for t in &p.trades {
            assert!(t.amount.is_some(), "{f}: no amount for {}", t.asset_name);
            assert!(!t.asset_name.is_empty(), "{f}: empty asset");
            assert!(t.tx_date.year_ce().1 >= 2020);
        }
    }
}

#[test]
fn house_scanned_paper_filing_detected() {
    assert!(matches!(house::parse_ptr_pdf(&read("house/8221321.pdf")), Err(PdfError::Scanned)));
}

#[test]
fn senate_search_results() {
    let page = senate::parse_search_json(&text("senate/search.json")).unwrap();
    assert!(page.total >= page.reports.len() as u64);
    let first = &page.reports[0];
    assert_eq!(first.kind, ReportKind::Electronic);
    assert_eq!(first.display_name(), "John Fetterman");
    assert_eq!(first.id, "0541be4f-4f96-4d84-8d6f-d349f218bb2a");
    assert_eq!(first.url, "https://efdsearch.senate.gov/search/view/ptr/0541be4f-4f96-4d84-8d6f-d349f218bb2a/");
    assert_eq!(first.date_received, Some(d(2026, 10, 7)));
    assert!(page.reports.iter().any(|r| r.kind == ReportKind::Paper));
    assert!(page.reports.iter().any(|r| r.is_amendment()));
}

#[test]
fn senate_csrf_token() {
    assert!(senate::csrf_from_form(&text("senate/home.html")).is_some_and(|t| t.len() > 20));
}

#[test]
fn senate_ptr_bonds() {
    let p = senate::parse_ptr_html(&text("senate/ptr_bonds.html")).unwrap();
    assert_eq!(p.filer_name.as_deref(), Some("John Fetterman"));
    assert_eq!(p.filed_date, Some(d(2026, 10, 7)));
    assert_eq!(p.trades.len(), 2);
    let t = &p.trades[0];
    assert_eq!(t.row, 1);
    assert_eq!(t.tx_date, d(2026, 9, 15));
    assert_eq!(t.owner.as_deref(), Some("Child"));
    assert_eq!(t.ticker, None);
    assert_eq!(t.asset_name, "DELL INTL LLC/EMC CORP");
    assert_eq!(t.asset_type.as_deref(), Some("Corporate Bond"));
    assert_eq!(t.tx_type, TxType::Purchase);
    assert!(t.comment.as_deref().unwrap().contains("Matures"));
}

#[test]
fn senate_ptr_stocks_and_many_rows() {
    let p = senate::parse_ptr_html(&text("senate/ptr_stocks.html")).unwrap();
    let tickers: Vec<_> = p.trades.iter().filter_map(|t| t.ticker.as_deref()).collect();
    assert!(tickers.contains(&"JPM") && tickers.contains(&"ADI"), "{tickers:?}");
    assert!(p.trades.iter().all(|t| t.amount.is_some()));
    let p = senate::parse_ptr_html(&text("senate/ptr_many.html")).unwrap();
    assert!(p.trades.len() >= 30);
    assert!(p.trades.windows(2).all(|w| w[0].row < w[1].row));
}

#[test]
fn rosters_resolve_party() {
    let house = roster::parse_house_roster(&text("roster/house_members.xml")).unwrap();
    let senate = roster::parse_senate_roster(&text("roster/senators.xml")).unwrap();
    let r = Roster::new(house, senate);
    let alford = r.house_by_district("MO04").unwrap();
    assert_eq!(alford.last, "Alford");
    assert_eq!(alford.party, Party::Republican);
    assert_eq!(alford.name, "Mark Alford");
    let f = r.find_by_name(Chamber::Senate, "John", "Fetterman").unwrap();
    assert_eq!(f.party, Party::Democrat);
    assert_eq!(f.state, "PA");
    assert_eq!(f.name, "John Fetterman");
    assert!(r.find_by_name(Chamber::Senate, "A. Mitchell", "McConnell, Jr.").is_some());
    assert!(r.find_by_name(Chamber::Senate, "James Conley", "Justice, II").is_some());
    // Independents caucusing with Democrats keep their own party code.
    if let Some(k) = r.find_by_name(Chamber::Senate, "Angus", "King") {
        assert_eq!(k.party, Party::Independent);
    }
}
