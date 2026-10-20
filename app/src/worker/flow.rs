//! Worker side of the crypto derivatives, order flow and prediction market panes (`tr-flow`).
//!
//! Live streams (liquidations, per-pane trade/depth feeds) hand the UI an `Arc<Mutex<…>>`
//! that the stream task writes into; the task stops by itself once the UI drops its handle,
//! so closing a pane or changing its market needs no explicit cancel.

use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use tr_flow::deribit::TermStructure;
use tr_flow::history::CoinHistory;
use tr_flow::liq::{SharedTape, Tape};
use tr_flow::orderflow::{Flow, SharedFlow};
use tr_flow::perps::{Board, PerpsClient};
use tr_flow::polymarket::{self, Detail, Market};
use tr_flow::{Exchange, sample};

use super::{PaneId, Response, Shared};

/// Per-symbol requests (open interest, long/short) are limited to the most traded contracts.
const BOARD_DETAIL: usize = 30;
/// The board is shared by every derivatives pane; refetch at most this often.
const BOARD_TTL: Duration = Duration::from_secs(20);

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
}

pub enum FlowResponse {
    Board(Arc<Board>),
    Coin { pane: PaneId, coin: String, history: Box<CoinHistory>, term: Option<Box<TermStructure>> },
    Liquidations(SharedTape),
    Stream { pane: PaneId, flow: SharedFlow },
    Markets(Result<Vec<Market>, String>),
    MarketDetail { pane: PaneId, detail: Box<Detail> },
}

#[derive(Default)]
pub struct FlowShared {
    client: Mutex<Option<PerpsClient>>,
    board: Mutex<Option<(Instant, Arc<Board>)>>,
    tape: Mutex<Weak<Mutex<Tape>>>,
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
    let offline = tr_flow::offline() || s.provider.name().eq_ignore_ascii_case("sample");
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
                let term = tr_flow::deribit::supported(&coin).then(|| sample::term_structure(&coin, now()));
                (sample::history(&coin, now()), term)
            } else {
                match s.flow.client() {
                    Ok(c) => {
                        let http = c.http().clone();
                        let term = async {
                            if tr_flow::deribit::supported(&coin) {
                                Some(tr_flow::deribit::fetch(&http, &coin).await)
                            } else {
                                None
                            }
                        };
                        tokio::join!(tr_flow::history::fetch(c.http(), &coin), term)
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
                                tokio::spawn(tr_flow::liq::run(tape.clone(), c.http().clone(), syms));
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
                        tokio::spawn(tr_flow::orderflow::run(flow.clone(), c.http().clone()));
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
