//! Live checks against the real venues (network required):
//! `cargo test -p hd-flow --test live -- --ignored --nocapture`.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use hd_flow::{Exchange, Http, deribit, history, liq, orderflow, perps, polymarket};

#[tokio::test]
#[ignore]
async fn board_from_every_venue() {
    let c = perps::PerpsClient::new().unwrap();
    let b = c.board(10).await;
    for (e, err) in &b.errors {
        eprintln!("{}: {err}", e.label());
    }
    let btc = b.row("BTC").expect("BTC on the board");
    eprintln!("BTC OI {} across {:?}", hd_flow::fmt_usd(btc.oi_usd), btc.venues.keys().collect::<Vec<_>>());
    assert!(btc.oi_usd > 1e9);
    assert!(btc.venues.len() >= 3, "at least three venues answered");
}

#[tokio::test]
#[ignore]
async fn history_term_structure_and_polymarket() {
    let http = Http::new().unwrap();
    let h = history::fetch(&http, "BTC").await;
    assert!(h.errors.is_empty(), "{:?}", h.errors);
    assert!(h.oi_usd.len() > 100 && !h.funding.is_empty());
    let ts = deribit::fetch(&http, "BTC").await;
    assert!(ts.error.is_none() && ts.points.len() > 3 && !ts.dvol.is_empty());
    let pm = polymarket::Client::new(http);
    let ms = pm.top(20).await.unwrap();
    assert!(!ms.is_empty());
    let d = pm.detail(&ms[0]).await;
    assert!(d.history.first().is_some_and(|h| !h.is_empty()), "{:?}", d.error);
}

#[tokio::test]
#[ignore]
async fn streams_deliver() {
    let http = Http::new().unwrap();
    let tape: liq::SharedTape = Arc::default();
    tokio::spawn(liq::run(tape.clone(), http.clone(), vec!["BTCUSDT".into(), "ETHUSDT".into()]));
    for venue in [Exchange::Binance, Exchange::Hyperliquid] {
        let flow = Arc::new(Mutex::new(orderflow::Flow::new(venue, "BTC", 60)));
        tokio::spawn(orderflow::run(flow.clone(), http.clone()));
        tokio::time::sleep(Duration::from_secs(8)).await;
        let f = flow.lock().unwrap();
        eprintln!("{venue:?}: {} trades, {} heat cols, status {}", f.trades.len(), f.heat.cols.len(), f.status);
        assert!(f.live && !f.trades.is_empty() && !f.heat.cols.is_empty());
    }
    eprintln!("liquidation feeds: {:?}", tape.lock().unwrap().status);
}

#[tokio::test]
#[ignore]
async fn hyperliquid_wallets_leaderboard_and_options_flow() {
    let http = Http::new().unwrap();
    let hl = hd_flow::hyperliquid::Client::new(http.clone());
    let lb = hl.leaderboard().await.unwrap();
    eprintln!("leaderboard: {} wallets, top {}", lb.len(), lb[0].label());
    assert!(lb.len() > 100);
    let wallets: Vec<String> = lb.iter().take(20).map(|l| l.address.clone()).collect();
    let pos = hl.positioning(&wallets).await;
    assert!(pos.wallets > 10, "scanned {} ({} errors)", pos.wallets, pos.errors);
    let _ = hl.fills(&wallets[0]).await.unwrap();
    assert!(!hl.vaults().await.unwrap().is_empty());
    let trades = hd_flow::options_flow::fetch(&http, "BTC").await.unwrap();
    assert!(!trades.is_empty());
}

#[tokio::test]
#[ignore]
async fn news_sources_parse() {
    // Every default news source should answer and parse; prints what each returned so a changed
    // format is easy to spot.
    let http = Http::new().unwrap();
    let universe: std::collections::HashSet<String> = ["BTC", "ETH", "SOL"].iter().map(|s| s.to_string()).collect();
    let mut ok = 0;
    for feed in hd_flow::news::default_feeds() {
        match hd_flow::news::fetch_feed(&http, &feed, &universe).await {
            Ok(items) => {
                println!("{:<16} {:>4} items  {:?}", feed.name, items.len(), items.first().map(|h| &h.title));
                ok += usize::from(!items.is_empty());
            }
            Err(e) => println!("{:<16} ERROR {e:#}", feed.name),
        }
    }
    assert!(ok >= hd_flow::news::DEFAULT_FEEDS.len() / 2, "most news sources should work");
}
