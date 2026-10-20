//! Live checks against the real venues (network required):
//! `cargo test -p tr-flow --test live -- --ignored --nocapture`.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tr_flow::{Exchange, Http, deribit, history, liq, orderflow, perps, polymarket};

#[tokio::test]
#[ignore]
async fn board_from_every_venue() {
    let c = perps::PerpsClient::new().unwrap();
    let b = c.board(10).await;
    for (e, err) in &b.errors {
        eprintln!("{}: {err}", e.label());
    }
    let btc = b.row("BTC").expect("BTC on the board");
    eprintln!("BTC OI {} across {:?}", tr_flow::fmt_usd(btc.oi_usd), btc.venues.keys().collect::<Vec<_>>());
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
    let hl = tr_flow::hyperliquid::Client::new(http.clone());
    let lb = hl.leaderboard().await.unwrap();
    eprintln!("leaderboard: {} wallets, top {}", lb.len(), lb[0].label());
    assert!(lb.len() > 100);
    let wallets: Vec<String> = lb.iter().take(20).map(|l| l.address.clone()).collect();
    let pos = hl.positioning(&wallets).await;
    assert!(pos.wallets > 10, "scanned {} ({} errors)", pos.wallets, pos.errors);
    let _ = hl.fills(&wallets[0]).await.unwrap();
    assert!(!hl.vaults().await.unwrap().is_empty());
    let trades = tr_flow::options_flow::fetch(&http, "BTC").await.unwrap();
    assert!(!trades.is_empty());
}
