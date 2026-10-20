//! Worker side of the crypto derivatives, order flow and prediction market panes (`hd-flow`).
//!
//! Live streams (liquidations, per-pane trade/depth feeds) hand the UI an `Arc<Mutex<…>>`
//! that the stream task writes into; the task stops by itself once the UI drops its handle,
//! so closing a pane or changing its market needs no explicit cancel.

use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use hd_flow::deribit::TermStructure;
use hd_flow::history::CoinHistory;
use hd_flow::hyperliquid::{self, Account, Fill, Leader, Positioning, Vault};
use hd_flow::liq::{SharedTape, Tape};
use hd_flow::orderflow::{Flow, SharedFlow};
use hd_flow::perps::{Board, PerpsClient};
use hd_flow::polymarket::{self, Detail, Market};
use hd_flow::{Exchange, sample};

use super::{PaneId, Response, Shared};

/// Per-symbol requests (open interest, long/short) are limited to the most traded contracts.
const BOARD_DETAIL: usize = 30;
/// The board is shared by every derivatives pane; refetch at most this often.
const BOARD_TTL: Duration = Duration::from_secs(20);
/// The Hyperliquid leaderboard is large and slow-moving.
const LEADERS_TTL: Duration = Duration::from_secs(600);
/// Positioning rescans the top wallets at most this often.
const POSITIONING_TTL: Duration = Duration::from_secs(180);
/// Wallets scanned for positioning (two info calls' weight each, well inside the limit).
pub const SCAN_WALLETS: usize = 200;

pub type SharedPrints = hd_flow::prints::SharedTape;
/// When the leaderboard was fetched, the leaders and the vaults.
type CachedLeaders = (Instant, Arc<Vec<Leader>>, Vec<Vault>);
pub type SharedOptionFlow = Arc<Mutex<hd_flow::options_flow::Tape>>;

pub enum FlowRequest {
    /// The aggregated derivatives board (cached for a few seconds).
    Board,
    /// History and (for BTC/ETH) the Deribit term structure for one coin.
    Coin {
        pane: PaneId,
        coin: String,
    },
    /// The shared live liquidation tape.
    Liquidations,
    /// A live trade + depth stream for a heatmap or footprint pane.
    Stream {
        pane: PaneId,
        venue: Exchange,
        coin: String,
        tf_secs: i64,
    },
    /// Prediction markets by 24-hour volume.
    Markets,
    MarketDetail {
        pane: PaneId,
        market: Box<Market>,
    },
    /// A Hyperliquid wallet's positions and recent fills.
    Wallet {
        pane: PaneId,
        address: String,
    },
    /// The Hyperliquid leaderboard and vaults (cached).
    Leaders,
    /// Positioning across the top wallets (cached).
    Positioning,
    /// The shared whale-prints stream.
    Prints,
    /// The shared Deribit options-flow tape.
    OptionFlow,
}

pub enum FlowResponse {
    Board(Arc<Board>),
    Coin { pane: PaneId, coin: String, history: Box<CoinHistory>, term: Option<Box<TermStructure>> },
    Liquidations(SharedTape),
    Stream { pane: PaneId, flow: SharedFlow },
    Markets(Result<Vec<Market>, String>),
    MarketDetail { pane: PaneId, detail: Box<Detail> },
    Wallet { pane: PaneId, address: String, account: Result<Box<Account>, String>, fills: Vec<Fill> },
    Leaders { leaders: Result<Arc<Vec<Leader>>, String>, vaults: Vec<Vault> },
    Positioning(Result<Arc<Positioning>, String>),
    Prints(SharedPrints),
    OptionFlow(SharedOptionFlow),
}

#[derive(Default)]
pub struct FlowShared {
    client: Mutex<Option<PerpsClient>>,
    board: Mutex<Option<(Instant, Arc<Board>)>>,
    tape: Mutex<Weak<Mutex<Tape>>>,
    leaders: Mutex<Option<CachedLeaders>>,
    positioning: Mutex<Option<(Instant, Arc<Positioning>)>>,
    prints: Mutex<Weak<Mutex<hd_flow::prints::Tape>>>,
    oflow: Mutex<Weak<Mutex<hd_flow::options_flow::Tape>>>,
}

/// Hand out the shared stream behind `slot`, starting it with `start` if nobody holds it.
fn shared_stream<T: Default>(slot: &Mutex<Weak<Mutex<T>>>, start: impl FnOnce(Arc<Mutex<T>>)) -> Arc<Mutex<T>> {
    if let Some(t) = slot.lock().ok().and_then(|w| w.upgrade()) {
        return t;
    }
    let t: Arc<Mutex<T>> = Arc::default();
    if let Ok(mut w) = slot.lock() {
        *w = Arc::downgrade(&t);
    }
    start(t.clone());
    t
}

impl FlowShared {
    fn client(&self) -> Result<PerpsClient, String> {
        let mut c = self.client.lock().map_err(|_| "client lock poisoned".to_string())?;
        if c.is_none() {
            *c = Some(PerpsClient::new().map_err(|e| format!("{e:#}"))?);
        }
        Ok(c.clone().expect("set above"))
    }
}

impl Shared {
    /// Whether a liquidation tape handed out earlier is still held by a pane.
    #[cfg(test)]
    pub(super) fn flow_tape_alive(&self) -> bool {
        self.flow.tape.lock().is_ok_and(|w| w.strong_count() > 0)
    }
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

pub(super) async fn handle(s: &Shared, req: FlowRequest) {
    let offline = hd_flow::offline() || s.provider.name().eq_ignore_ascii_case("sample");
    let send = |r: FlowResponse| s.send(Response::Flow(Box::new(r)));
    match req {
        FlowRequest::Board => {
            let cached = s.flow.board.lock().ok().and_then(|b| b.clone()).filter(|(t, _)| t.elapsed() < BOARD_TTL);
            let board = match cached {
                Some((_, b)) => b,
                None => {
                    let b = if offline {
                        sample::board(now())
                    } else {
                        match s.flow.client() {
                            Ok(c) => c.board(BOARD_DETAIL).await,
                            Err(e) => Board { errors: vec![(Exchange::Binance, e)], ..Default::default() },
                        }
                    };
                    let b = Arc::new(b);
                    if let Ok(mut slot) = s.flow.board.lock() {
                        *slot = Some((Instant::now(), b.clone()));
                    }
                    b
                }
            };
            send(FlowResponse::Board(board));
        }
        FlowRequest::Coin { pane, coin } => {
            let (history, term) = if offline {
                let term = hd_flow::deribit::supported(&coin).then(|| sample::term_structure(&coin, now()));
                (sample::history(&coin, now()), term)
            } else {
                match s.flow.client() {
                    Ok(c) => {
                        let http = c.http().clone();
                        let term = async {
                            if hd_flow::deribit::supported(&coin) {
                                Some(hd_flow::deribit::fetch(&http, &coin).await)
                            } else {
                                None
                            }
                        };
                        tokio::join!(hd_flow::history::fetch(c.http(), &coin), term)
                    }
                    Err(e) => (CoinHistory { coin: coin.clone(), errors: vec![e], ..Default::default() }, None),
                }
            };
            send(FlowResponse::Coin { pane, coin, history: Box::new(history), term: term.map(Box::new) });
        }
        FlowRequest::Liquidations => {
            let existing = s.flow.tape.lock().ok().and_then(|w| w.upgrade());
            let tape = match existing {
                Some(t) => t,
                None => {
                    let tape: SharedTape = Arc::default();
                    if let Ok(mut w) = s.flow.tape.lock() {
                        *w = Arc::downgrade(&tape);
                    }
                    if offline {
                        tokio::spawn(sample::run_liquidations(tape.clone()));
                    } else {
                        match s.flow.client() {
                            Ok(c) => {
                                // Bybit needs one topic per contract: follow the biggest markets.
                                let board = s.flow.board.lock().ok().and_then(|b| b.clone()).map(|(_, b)| b);
                                let mut syms: Vec<String> = board
                                    .map(|b| {
                                        b.rows
                                            .iter()
                                            .filter_map(|r| r.venue(Exchange::Bybit).map(|v| v.symbol.clone()))
                                            .take(60)
                                            .collect()
                                    })
                                    .unwrap_or_default();
                                if syms.is_empty() {
                                    syms = ["BTC", "ETH", "SOL", "XRP", "DOGE", "SUI", "BNB", "LINK", "AVAX", "ADA"]
                                        .iter()
                                        .map(|c| format!("{c}USDT"))
                                        .collect();
                                }
                                tokio::spawn(hd_flow::liq::run(tape.clone(), c.http().clone(), syms));
                            }
                            Err(e) => {
                                if let Ok(mut t) = tape.lock() {
                                    t.status.insert(Exchange::Binance, e);
                                }
                            }
                        }
                    }
                    tape
                }
            };
            send(FlowResponse::Liquidations(tape));
        }
        FlowRequest::Stream { pane, venue, coin, tf_secs } => {
            let flow: SharedFlow = Arc::new(Mutex::new(Flow::new(venue, &coin, tf_secs)));
            if offline {
                tokio::spawn(sample::run_flow(flow.clone()));
            } else {
                match s.flow.client() {
                    Ok(c) => {
                        tokio::spawn(hd_flow::orderflow::run(flow.clone(), c.http().clone()));
                    }
                    Err(e) => {
                        if let Ok(mut f) = flow.lock() {
                            f.status = e;
                        }
                    }
                }
            }
            send(FlowResponse::Stream { pane, flow });
        }
        FlowRequest::Markets => {
            let r = if offline {
                Ok(sample::markets())
            } else {
                match s.flow.client() {
                    Ok(c) => polymarket::Client::new(c.http().clone()).top(200).await.map_err(|e| format!("{e:#}")),
                    Err(e) => Err(e),
                }
            };
            send(FlowResponse::Markets(r));
        }
        FlowRequest::Wallet { pane, address } => {
            let (account, fills) = if offline {
                (Ok(Box::new(sample::account(&address))), sample::fills(&address, now() * 1000))
            } else {
                match s.flow.client() {
                    Ok(c) => {
                        let hl = hyperliquid::Client::new(c.http().clone());
                        let (a, f) = tokio::join!(hl.account(&address), hl.fills(&address));
                        (a.map(Box::new).map_err(|e| format!("{e:#}")), f.unwrap_or_default())
                    }
                    Err(e) => (Err(e), Vec::new()),
                }
            };
            send(FlowResponse::Wallet { pane, address, account, fills });
        }
        FlowRequest::Leaders => {
            let (leaders, vaults) = leaders(s, offline).await;
            send(FlowResponse::Leaders { leaders, vaults });
        }
        FlowRequest::Positioning => {
            let cached =
                s.flow.positioning.lock().ok().and_then(|p| p.clone()).filter(|(t, _)| t.elapsed() < POSITIONING_TTL);
            let r = match cached {
                Some((_, p)) => Ok(p),
                None => match leaders(s, offline).await.0 {
                    Err(e) => Err(e),
                    Ok(ls) => {
                        let wallets: Vec<String> = ls.iter().take(SCAN_WALLETS).map(|l| l.address.clone()).collect();
                        let p = if offline {
                            sample::positioning(&wallets)
                        } else {
                            match s.flow.client() {
                                Ok(c) => hyperliquid::Client::new(c.http().clone()).positioning(&wallets).await,
                                Err(e) => return send(FlowResponse::Positioning(Err(e))),
                            }
                        };
                        let p = Arc::new(p);
                        if let Ok(mut slot) = s.flow.positioning.lock() {
                            *slot = Some((Instant::now(), p.clone()));
                        }
                        Ok(p)
                    }
                },
            };
            send(FlowResponse::Positioning(r));
        }
        FlowRequest::Prints => {
            let board = s.flow.board.lock().ok().and_then(|b| b.clone()).map(|(_, b)| b);
            let tape = shared_stream(&s.flow.prints, |t| {
                if offline {
                    tokio::spawn(sample::run_prints(t));
                } else {
                    // Follow the most traded Hyperliquid perps (by the board when loaded).
                    let mut coins: Vec<String> = board
                        .map(|b| {
                            let mut v: Vec<_> = b.rows.iter().filter_map(|r| r.venue(Exchange::Hyperliquid)).collect();
                            v.sort_by(|a, b| b.volume_usd.unwrap_or(0.0).total_cmp(&a.volume_usd.unwrap_or(0.0)));
                            v.into_iter().take(40).map(|x| x.symbol.clone()).collect()
                        })
                        .unwrap_or_default();
                    if coins.is_empty() {
                        coins =
                            ["BTC", "ETH", "SOL", "HYPE", "XRP", "DOGE", "SUI", "kPEPE", "LINK", "AVAX", "ENA", "BNB"]
                                .map(String::from)
                                .to_vec();
                    }
                    tokio::spawn(hd_flow::prints::run(t, coins));
                }
            });
            send(FlowResponse::Prints(tape));
        }
        FlowRequest::OptionFlow => {
            let client = if offline { None } else { s.flow.client().ok() };
            let tape = shared_stream(&s.flow.oflow, |t| match client {
                Some(c) => {
                    tokio::spawn(hd_flow::options_flow::run(t, c.http().clone()));
                }
                None if offline => {
                    tokio::spawn(sample::run_option_flow(t));
                }
                None => {
                    if let Ok(mut g) = t.lock() {
                        g.error = Some("HTTP client unavailable".into());
                    }
                }
            });
            send(FlowResponse::OptionFlow(tape));
        }
        FlowRequest::MarketDetail { pane, market } => {
            let detail = if offline {
                sample::detail(&market, now())
            } else {
                match s.flow.client() {
                    Ok(c) => polymarket::Client::new(c.http().clone()).detail(&market).await,
                    Err(e) => Detail { market_id: market.id.clone(), error: Some(e), ..Default::default() },
                }
            };
            send(FlowResponse::MarketDetail { pane, detail: Box::new(detail) });
        }
    }
}

/// The leaderboard (largest accounts first) and open vaults, cached for ten minutes.
async fn leaders(s: &Shared, offline: bool) -> (Result<Arc<Vec<Leader>>, String>, Vec<Vault>) {
    if let Some((t, l, v)) = s.flow.leaders.lock().ok().and_then(|x| x.clone())
        && t.elapsed() < LEADERS_TTL
    {
        return (Ok(l), v);
    }
    let (l, v) = if offline {
        (Ok(sample::leaderboard()), Ok(sample::vaults()))
    } else {
        match s.flow.client() {
            Ok(c) => {
                let hl = hyperliquid::Client::new(c.http().clone());
                let (l, v) = tokio::join!(hl.leaderboard(), hl.vaults());
                (l.map_err(|e| format!("{e:#}")), v.map_err(|e| format!("{e:#}")))
            }
            Err(e) => (Err(e.clone()), Err(e)),
        }
    };
    let vaults = v.unwrap_or_default();
    match l {
        Ok(l) => {
            let l = Arc::new(l);
            if let Ok(mut slot) = s.flow.leaders.lock() {
                *slot = Some((Instant::now(), l.clone(), vaults.clone()));
            }
            (Ok(l), vaults)
        }
        Err(e) => (Err(e), vaults),
    }
}
