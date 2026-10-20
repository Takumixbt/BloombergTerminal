//! Crypto derivatives and order-flow panes (data from `hd-flow`):
//!
//! - `DERIV`: funding, open interest, volume, premium and long/short for every perp across
//!   Binance, Bybit, OKX and Hyperliquid; `BTC DERIV` drills into one coin (per-venue table,
//!   open interest / funding / long-short history, Deribit term structure and DVOL).
//! - `LIQ`: live liquidations from Binance, Bybit and OKX with long/short totals.
//! - `HEAT`: order book heatmap (resting liquidity over time) with trades and CVD.
//! - `FOOT`: footprint candles (buy × sell volume at each price) with delta and CVD.
//! - `PRED`: Polymarket prediction markets with price history and order book.

use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::{Align2, Color32, ColorImage, Pos2, Rect, RichText, Sense, Shape, Stroke, TextureOptions, Ui, pos2, vec2};
use hd_flow::deribit::TermStructure;
use hd_flow::history::CoinHistory;
use hd_flow::liq::{SharedTape, Side};
use hd_flow::orderflow::SharedFlow;
use hd_flow::perps::{Board, CoinRow};
use hd_flow::polymarket::{Detail, Market};
use hd_flow::{Exchange, fmt_usd};
use serde::{Deserialize, Serialize};

use super::{PaneKind, TerminalApp};
use crate::congress_view::Action;
use crate::theme::{self, amber, dp, mono, muted, text, ui_font};
use crate::widgets;
use crate::worker::{FlowRequest, FlowResponse, Request};

// ---- Pane state ----------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum DerivSort {
    #[default]
    Oi,
    Coin,
    Price,
    Change,
    OiChange,
    Volume,
    Funding,
    Venue(Exchange),
    Premium,
    LongShort,
}

pub struct DerivPane {
    /// `None` = the board; `Some` = one coin's detail.
    pub coin: Option<String>,
    pub sort: DerivSort,
    pub ascending: bool,
    /// Show funding annualised instead of per 8 hours.
    pub apr: bool,
    pub filter: String,
    pub history: Option<Box<CoinHistory>>,
    pub term: Option<Box<TermStructure>>,
    pub fetched: Option<Instant>,
}

impl DerivPane {
    pub fn new(coin: Option<String>, apr: bool) -> Self {
        Self {
            coin: coin.map(|c| hd_flow::venue::coin_of_key(&c)),
            sort: DerivSort::Oi,
            ascending: false,
            apr,
            filter: String::new(),
            history: None,
            term: None,
            fetched: None,
        }
    }

    pub(super) fn set_coin(&mut self, coin: Option<String>) {
        if self.coin != coin {
            self.coin = coin;
            self.history = None;
            self.term = None;
            self.fetched = None;
        }
    }
}

pub struct LiqPane {
    /// Only this coin (`None` = all).
    pub coin: Option<String>,
    pub min_usd: f64,
    pub window_min: i64,
}

impl LiqPane {
    pub fn new(coin: Option<String>, min_usd: f64) -> Self {
        Self { coin: coin.map(|c| hd_flow::venue::coin_of_key(&c)), min_usd, window_min: 60 }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum FlowView {
    #[default]
    Heatmap,
    Footprint,
}

pub struct FlowPane {
    pub coin: String,
    pub venue: Exchange,
    pub view: FlowView,
    pub tf_secs: i64,
    pub flow: Option<SharedFlow>,
    pub requested: Option<Instant>,
    /// Visible price range as a fraction of the price (heatmap), or bucket multiplier
    /// (footprint).
    pub zoom: f32,
    /// Footprint candles scrolled back from the latest.
    pub offset: usize,
    texture: Option<egui::TextureHandle>,
    /// Heatmap columns baked into `texture`: (last column ts, count, price lo, hi).
    baked: (i64, usize, f64, f64),
}

impl FlowPane {
    pub fn new(coin: &str, venue: Exchange, view: FlowView, tf_secs: i64) -> Self {
        let venue = if venue == Exchange::Hyperliquid { venue } else { Exchange::Binance };
        Self {
            coin: hd_flow::venue::coin_of_key(coin),
            venue,
            view,
            tf_secs: tf_secs.max(60),
            flow: None,
            requested: None,
            zoom: 1.0,
            offset: 0,
            texture: None,
            baked: (0, 0, 0.0, 0.0),
        }
    }

    /// Point the pane at another market; the old stream stops once its handle drops.
    pub fn retarget(&mut self, coin: &str, venue: Exchange) {
        let coin = hd_flow::venue::coin_of_key(coin);
        if coin != self.coin || venue != self.venue {
            *self = FlowPane { zoom: self.zoom, ..FlowPane::new(&coin, venue, self.view, self.tf_secs) };
        }
    }
}

pub struct PredPane {
    pub query: String,
    pub selected: Option<String>,
    pub detail: Option<Box<Detail>>,
    pub loading: bool,
}

impl PredPane {
    pub fn new(query: &str) -> Self {
        Self { query: query.into(), selected: None, detail: None, loading: false }
    }
}

/// App-wide flow data shared by every pane of a kind.
#[derive(Default)]
pub struct FlowData {
    pub board: Option<Arc<Board>>,
    board_at: Option<Instant>,
    pub tape: Option<SharedTape>,
    tape_requested: Option<Instant>,
    pub markets: Option<Result<Vec<Market>, String>>,
    markets_at: Option<Instant>,
}

// ---- Polling and responses -----------------------------------------------------------

impl TerminalApp {
    /// Fetch what visible flow panes need; drop the liquidation tape when no pane uses it.
    pub(super) fn flow_periodic(&mut self, visible: &[usize]) {
        let shown = |f: &dyn Fn(&PaneKind) -> bool| visible.iter().any(|&i| f(&self.panes[i].kind));
        let fd = &mut self.flow;
        if shown(&|k| matches!(k, PaneKind::Deriv(_)))
            && fd.board_at.is_none_or(|t| t.elapsed() > Duration::from_secs(30))
        {
            fd.board_at = Some(Instant::now());
            self.worker.send(Request::Flow(FlowRequest::Board));
        }
        if shown(&|k| matches!(k, PaneKind::Predict(_)))
            && fd.markets_at.is_none_or(|t| t.elapsed() > Duration::from_secs(60))
        {
            fd.markets_at = Some(Instant::now());
            self.worker.send(Request::Flow(FlowRequest::Markets));
        }
        let any_liq = self.panes.iter().any(|p| matches!(p.kind, PaneKind::Liq(_)));
        if !any_liq {
            fd.tape = None;
        } else if fd.tape.is_none() && fd.tape_requested.is_none_or(|t| t.elapsed() > Duration::from_secs(10)) {
            fd.tape_requested = Some(Instant::now());
            self.worker.send(Request::Flow(FlowRequest::Liquidations));
        }
        for &i in visible {
            let id = self.panes[i].id;
            match &mut self.panes[i].kind {
                PaneKind::Deriv(d) => {
                    if let Some(coin) = &d.coin
                        && d.fetched.is_none_or(|t| t.elapsed() > Duration::from_secs(300))
                    {
                        d.fetched = Some(Instant::now());
                        self.worker.send(Request::Flow(FlowRequest::Coin { pane: id, coin: coin.clone() }));
                    }
                }
                PaneKind::Flow(f) => {
                    if f.flow.is_none() && f.requested.is_none_or(|t| t.elapsed() > Duration::from_secs(10)) {
                        f.requested = Some(Instant::now());
                        self.worker.send(Request::Flow(FlowRequest::Stream {
                            pane: id,
                            venue: f.venue,
                            coin: f.coin.clone(),
                            tf_secs: f.tf_secs,
                        }));
                    }
                }
                PaneKind::Predict(p) => {
                    if let Some(sel) = &p.selected
                        && p.detail.as_ref().is_none_or(|d| &d.market_id != sel)
                        && !p.loading
                        && let Some(Ok(ms)) = &self.flow.markets
                        && let Some(m) = ms.iter().find(|m| &m.id == sel)
                    {
                        p.loading = true;
                        self.worker
                            .send(Request::Flow(FlowRequest::MarketDetail { pane: id, market: Box::new(m.clone()) }));
                    }
                }
                _ => {}
            }
        }
    }

    pub(super) fn flow_response(&mut self, r: FlowResponse) {
        fn pane(app: &mut TerminalApp, id: crate::dock::PaneId) -> Option<&mut PaneKind> {
            app.panes.iter_mut().find(|p| p.id == id).map(|p| &mut p.kind)
        }
        match r {
            FlowResponse::Board(b) => self.flow.board = Some(b),
            FlowResponse::Coin { pane: id, coin, history, term } => {
                if let Some(PaneKind::Deriv(d)) = pane(self, id)
                    && d.coin.as_deref() == Some(coin.as_str())
                {
                    d.history = Some(history);
                    d.term = term;
                }
            }
            FlowResponse::Liquidations(t) => {
                if self.panes.iter().any(|p| matches!(p.kind, PaneKind::Liq(_))) {
                    self.flow.tape = Some(t);
                }
            }
            FlowResponse::Stream { pane: id, flow } => {
                if let Some(PaneKind::Flow(f)) = pane(self, id) {
                    let ok = flow.lock().is_ok_and(|g| g.coin == f.coin && g.venue == f.venue);
                    if ok {
                        f.flow = Some(flow);
                    }
                }
            }
            FlowResponse::Markets(m) => {
                // Keep the last good list through a transient error.
                if m.is_ok() || !matches!(self.flow.markets, Some(Ok(_))) {
                    self.flow.markets = Some(m);
                }
            }
            FlowResponse::MarketDetail { pane: id, detail } => {
                if let Some(PaneKind::Predict(p)) = pane(self, id) {
                    p.loading = false;
                    if p.selected.as_deref() == Some(detail.market_id.as_str()) {
                        p.detail = Some(detail);
                    }
                }
            }
            // Hyperliquid and options responses are handled in `hl_view` first.
            _ => {}
        }
    }

    /// Render a flow pane. Returns its actions.
    pub(super) fn flow_content(&mut self, ui: &mut Ui, i: usize) -> Vec<Action> {
        let board = self.flow.board.clone();
        let tape = self.flow.tape.clone();
        let markets = self.flow.markets.as_ref();
        match &mut self.panes[i].kind {
            PaneKind::Deriv(d) => match d.coin.clone() {
                None => deriv_board(ui, d, board.as_deref()).into_iter().collect(),
                Some(coin) => deriv_detail(ui, d, &coin, board.as_deref()).into_iter().collect(),
            },
            PaneKind::Liq(l) => liquidations(ui, l, tape.as_ref()).into_iter().collect(),
            PaneKind::Flow(f) => flow_pane(ui, f).into_iter().collect(),
            PaneKind::Predict(p) => {
                predict(ui, p, markets);
                Vec::new()
            }
            _ => Vec::new(),
        }
    }
}

// ---- Shared drawing helpers ----------------------------------------------------------

fn simulated_badge(ui: &mut Ui, what: &str) {
    ui.label(RichText::new(" SIMULATED ").font(ui_font(10.5)).color(Color32::BLACK).background_color(amber()))
        .on_hover_text(format!("Offline sample data (HD_PROVIDER=sample); {what} are synthetic."));
}

fn title(ui: &mut Ui, s: &str) {
    ui.label(RichText::new(s).font(mono(12.5)).color(amber()));
}

fn note(ui: &mut Ui, s: impl Into<String>) {
    ui.label(RichText::new(s.into()).font(ui_font(11.0)).color(muted()));
}

/// Decimals that keep about five significant digits.
fn price_dp(p: f64) -> usize {
    let a = p.abs();
    if a >= 10_000.0 {
        1
    } else if a >= 100.0 {
        2
    } else if a >= 1.0 {
        4
    } else if a > 0.0 {
        ((-a.log10()).floor() as usize + 4).min(10)
    } else {
        2
    }
}

pub fn fmt_price(p: f64) -> String {
    format!("{:.*}", price_dp(p), p)
}

fn pct(x: f64, dp: usize) -> String {
    format!("{:+.*}%", dp, x * 100.0)
}

fn signed_color(x: f64) -> Color32 {
    if x > 0.0 {
        theme::up()
    } else if x < 0.0 {
        theme::down()
    } else {
        muted()
    }
}

fn fmt_funding(f8h: f64, apr: bool) -> String {
    if apr { format!("{:+.1}%", f8h * 3.0 * 365.0 * 100.0) } else { format!("{:+.4}%", f8h * 100.0) }
}

/// Funding cell background: stronger for rates far from the 0.01%/8h baseline.
fn funding_wash(f8h: f64) -> Color32 {
    let dev = f8h - 0.0001;
    let t = (dev.abs() / 0.0004).min(1.0) as f32;
    let c = if dev >= 0.0 { theme::up() } else { theme::down() };
    c.gamma_multiply(0.05 + 0.3 * t)
}

fn local_hms(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|d| d.with_timezone(&chrono::Local).format("%H:%M:%S").to_string())
        .unwrap_or_default()
}

/// A compact line or bar chart of `(unix_secs, value)` with a title, last value and range.
fn mini_chart(
    ui: &Ui,
    rect: Rect,
    title: &str,
    pts: &[(i64, f64)],
    bars: bool,
    fmt: &dyn Fn(f64) -> String,
    color: Color32,
) {
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 6.0, theme::header().gamma_multiply(0.6));
    let head = Rect::from_min_size(rect.min, vec2(rect.width(), dp(18.0)));
    p.text(pos2(head.left() + 6.0, head.center().y), Align2::LEFT_CENTER, title, ui_font(11.0), muted());
    let Some(&(_, last)) = pts.last() else {
        p.text(rect.center(), Align2::CENTER_CENTER, "no data", ui_font(11.0), muted());
        return;
    };
    p.text(pos2(head.right() - 6.0, head.center().y), Align2::RIGHT_CENTER, fmt(last), mono(11.5), text());
    let plot = Rect::from_min_max(
        pos2(rect.left() + 6.0, head.bottom() + 2.0),
        pos2(rect.right() - 6.0, rect.bottom() - 14.0),
    );
    if plot.height() < 8.0 {
        return;
    }
    let (t0, t1) = (pts[0].0 as f64, pts[pts.len() - 1].0.max(pts[0].0 + 1) as f64);
    let mut lo = pts.iter().map(|x| x.1).fold(f64::INFINITY, f64::min);
    let mut hi = pts.iter().map(|x| x.1).fold(f64::NEG_INFINITY, f64::max);
    if bars {
        lo = lo.min(0.0);
        hi = hi.max(0.0);
    }
    if (hi - lo).abs() < 1e-18 {
        hi += 1e-9_f64.max(hi.abs() * 0.01);
        lo -= 1e-9_f64.max(lo.abs() * 0.01);
    }
    let x = |t: i64| plot.left() + ((t as f64 - t0) / (t1 - t0)) as f32 * plot.width();
    let y = |v: f64| plot.bottom() - ((v - lo) / (hi - lo)) as f32 * plot.height();
    if bars {
        let w = (plot.width() / pts.len() as f32 - 1.0).clamp(1.0, 8.0);
        let zero = y(0.0);
        for &(t, v) in pts {
            let (a, b) = (zero.min(y(v)), zero.max(y(v)));
            let c = if v >= 0.0 { theme::up() } else { theme::down() };
            p.rect_filled(
                Rect::from_min_max(pos2(x(t) - w / 2.0, a), pos2(x(t) + w / 2.0, b.max(a + 1.0))),
                0.0,
                c.gamma_multiply(0.85),
            );
        }
        p.hline(plot.x_range(), zero, Stroke::new(1.0, theme::border()));
    } else {
        let line: Vec<Pos2> = pts.iter().map(|&(t, v)| pos2(x(t), y(v))).collect();
        let mut area = line.clone();
        area.push(pos2(plot.right(), plot.bottom()));
        area.push(pos2(plot.left(), plot.bottom()));
        if line.len() > 1 {
            // A convex fill would be wrong for an arbitrary series; draw thin columns instead.
            for w in line.windows(2) {
                p.add(Shape::convex_polygon(
                    vec![w[0], w[1], pos2(w[1].x, plot.bottom()), pos2(w[0].x, plot.bottom())],
                    color.gamma_multiply(0.12),
                    Stroke::NONE,
                ));
            }
            p.add(Shape::line(line, Stroke::new(1.5, color)));
        }
    }
    let foot = rect.bottom() - 7.0;
    p.text(pos2(plot.left(), foot), Align2::LEFT_CENTER, format!("lo {}", fmt(lo)), ui_font(10.0), muted());
    p.text(pos2(plot.right(), foot), Align2::RIGHT_CENTER, format!("hi {}", fmt(hi)), ui_font(10.0), muted());
}

/// One small history chart in a coin's detail view.
struct Spark {
    title: String,
    pts: Vec<(i64, f64)>,
    bars: bool,
    fmt: fn(f64) -> String,
    color: Color32,
}

/// Lay out `n` chart cells in a grid filling `rect`.
fn grid(rect: Rect, n: usize, min_w: f32, h: f32) -> Vec<Rect> {
    let cols = ((rect.width() / min_w).floor() as usize).clamp(1, n.max(1));
    let w = rect.width() / cols as f32;
    (0..n)
        .map(|k| {
            let (r, c) = (k / cols, k % cols);
            Rect::from_min_size(pos2(rect.left() + c as f32 * w, rect.top() + r as f32 * h), vec2(w, h)).shrink(3.0)
        })
        .collect()
}

// ---- DERIV board ---------------------------------------------------------------------

struct Col {
    title: &'static str,
    w: f32,
    sort: DerivSort,
}

fn board_cols() -> Vec<Col> {
    let mut v = vec![
        Col { title: "Coin", w: 64.0, sort: DerivSort::Coin },
        Col { title: "Price", w: 96.0, sort: DerivSort::Price },
        Col { title: "24h", w: 64.0, sort: DerivSort::Change },
        Col { title: "OI", w: 76.0, sort: DerivSort::Oi },
        Col { title: "OI 24h", w: 62.0, sort: DerivSort::OiChange },
        Col { title: "Vol 24h", w: 76.0, sort: DerivSort::Volume },
        Col { title: "Funding", w: 80.0, sort: DerivSort::Funding },
    ];
    for e in Exchange::BOARD {
        v.push(Col { title: e.short(), w: 74.0, sort: DerivSort::Venue(e) });
    }
    v.push(Col { title: "Prem", w: 64.0, sort: DerivSort::Premium });
    v.push(Col { title: "L/S", w: 48.0, sort: DerivSort::LongShort });
    v
}

fn sort_key(r: &CoinRow, s: DerivSort) -> f64 {
    match s {
        DerivSort::Oi | DerivSort::Coin => r.oi_usd,
        DerivSort::Price => r.price,
        DerivSort::Change => r.change_24h.unwrap_or(f64::NEG_INFINITY),
        DerivSort::OiChange => r.oi_change_24h.unwrap_or(f64::NEG_INFINITY),
        DerivSort::Volume => r.volume_usd,
        DerivSort::Funding => r.funding_8h.unwrap_or(f64::NEG_INFINITY),
        DerivSort::Venue(e) => r.venue(e).and_then(|v| v.funding_8h()).unwrap_or(f64::NEG_INFINITY),
        DerivSort::Premium => r.premium.unwrap_or(f64::NEG_INFINITY),
        DerivSort::LongShort => r.long_short.unwrap_or(f64::NEG_INFINITY),
    }
}

fn board_header(ui: &mut Ui, d: &mut DerivPane, b: &Board) {
    ui.horizontal_wrapped(|ui| {
        title(ui, "CRYPTO DERIVATIVES");
        ui.label(RichText::new(format!("OI {}", fmt_usd(b.total_oi()))).font(mono(12.0)).color(text()));
        ui.label(RichText::new(format!("Vol {}", fmt_usd(b.total_volume()))).font(mono(12.0)).color(text()));
        for (e, oi) in b.oi_by_venue() {
            ui.label(RichText::new(format!("{} {}", e.short(), fmt_usd(oi))).font(mono(11.0)).color(muted()));
        }
        if b.simulated {
            simulated_badge(ui, "funding, open interest and prices");
        }
    });
    ui.horizontal_wrapped(|ui| {
        if let Some(i) = widgets::segmented(ui, "deriv-apr", &["8h", "APR"], Some(usize::from(d.apr)), 20.0) {
            d.apr = i == 1;
        }
        ui.add(
            egui::TextEdit::singleline(&mut d.filter).hint_text("filter coins").font(mono(12.0)).desired_width(120.0),
        );
        let when = chrono::DateTime::from_timestamp(b.updated, 0)
            .map(|t| t.with_timezone(&chrono::Local).format("%H:%M:%S").to_string())
            .unwrap_or_default();
        note(ui, format!("{} coins · updated {when} · click a row for detail", b.rows.len()));
        for (e, err) in &b.errors {
            ui.label(RichText::new(format!("{} offline", e.short())).font(ui_font(11.0)).color(theme::down()))
                .on_hover_text(err);
        }
    });
}

fn deriv_board(ui: &mut Ui, d: &mut DerivPane, board: Option<&Board>) -> Option<Action> {
    let Some(b) = board else {
        title(ui, "CRYPTO DERIVATIVES");
        note(ui, "Loading funding and open interest from Binance, Bybit, OKX, Hyperliquid and Deribit…");
        ui.add(egui::Spinner::new());
        return None;
    };
    board_header(ui, d, b);
    let mut rows: Vec<&CoinRow> = b
        .rows
        .iter()
        .filter(|r| d.filter.trim().is_empty() || r.coin.contains(&d.filter.trim().to_ascii_uppercase()))
        .collect();
    if d.sort == DerivSort::Coin {
        rows.sort_by(|a, b| a.coin.cmp(&b.coin));
    } else {
        rows.sort_by(|a, b| sort_key(b, d.sort).total_cmp(&sort_key(a, d.sort)));
    }
    if d.ascending {
        rows.reverse();
    }
    let cols = board_cols();
    let scale = dp(1.0);
    let total_w: f32 = cols.iter().map(|c| c.w * scale).sum();
    let row_h = dp(20.0);
    let mut action = None;
    egui::ScrollArea::both().id_salt("deriv-board").auto_shrink([false, false]).show(ui, |ui| {
        let (hdr, _) = ui.allocate_exact_size(vec2(total_w.max(ui.available_width()), row_h), Sense::hover());
        let p = ui.painter().clone();
        p.rect_filled(hdr, 3.0, theme::header());
        let mut x = hdr.left();
        for c in &cols {
            let r = Rect::from_min_size(pos2(x, hdr.top()), vec2(c.w * scale, row_h));
            let resp = ui.interact(r, ui.id().with(("deriv-col", c.title)), Sense::click());
            let active = d.sort == c.sort;
            let left = c.sort == DerivSort::Coin;
            let (anchor, px) =
                if left { (Align2::LEFT_CENTER, r.left() + 6.0) } else { (Align2::RIGHT_CENTER, r.right() - 14.0) };
            let tr = p.text(
                pos2(px, r.center().y),
                anchor,
                c.title,
                ui_font(11.0),
                if active || resp.hovered() { text() } else { muted() },
            );
            if active {
                // A small triangle after the title (the UI font has no arrow glyphs).
                let (cx, cy) = (tr.right() + 6.0, r.center().y);
                let pts = if d.ascending {
                    vec![pos2(cx - 3.5, cy + 2.0), pos2(cx + 3.5, cy + 2.0), pos2(cx, cy - 2.5)]
                } else {
                    vec![pos2(cx - 3.5, cy - 2.0), pos2(cx + 3.5, cy - 2.0), pos2(cx, cy + 2.5)]
                };
                p.add(Shape::convex_polygon(pts, text(), Stroke::NONE));
            }
            if resp.clicked() {
                if active {
                    d.ascending = !d.ascending;
                } else {
                    d.sort = c.sort;
                    d.ascending = c.sort == DerivSort::Coin;
                }
            }
            x += c.w * scale;
        }
        for r in rows {
            let (rect, resp) = ui.allocate_exact_size(vec2(total_w.max(ui.available_width()), row_h), Sense::click());
            widgets::hover_wash(ui, rect, &resp, false);
            let mut x = rect.left();
            let mut cell = |k: usize, s: String, c: Color32, wash: Option<Color32>| {
                let w = cols[k].w * scale;
                let cr = Rect::from_min_size(pos2(x, rect.top()), vec2(w, row_h));
                if let Some(wash) = wash {
                    p.rect_filled(cr.shrink2(vec2(2.0, 2.0)), 3.0, wash);
                }
                if k == 0 {
                    p.text(pos2(cr.left() + 6.0, cr.center().y), Align2::LEFT_CENTER, s, mono(12.0), c);
                } else {
                    p.text(pos2(cr.right() - 6.0, cr.center().y), Align2::RIGHT_CENTER, s, mono(12.0), c);
                }
                x += w;
            };
            cell(0, r.coin.clone(), amber(), None);
            cell(1, fmt_price(r.price), text(), None);
            let ch = r.change_24h.unwrap_or(0.0);
            cell(2, r.change_24h.map_or("—".into(), |c| pct(c, 2)), signed_color(ch), None);
            cell(3, fmt_usd(r.oi_usd), text(), None);
            cell(
                4,
                r.oi_change_24h.map_or("—".into(), |c| pct(c, 1)),
                signed_color(r.oi_change_24h.unwrap_or(0.0)),
                None,
            );
            cell(5, fmt_usd(r.volume_usd), text(), None);
            match r.funding_8h {
                Some(f) => cell(6, fmt_funding(f, d.apr), text(), Some(funding_wash(f))),
                None => cell(6, "—".into(), muted(), None),
            }
            for (k, e) in Exchange::BOARD.iter().enumerate() {
                match r.venue(*e).and_then(|v| v.funding_8h()) {
                    Some(f) => cell(
                        7 + k,
                        fmt_funding(f, d.apr),
                        signed_color(f - 0.0001).gamma_multiply(0.9),
                        Some(funding_wash(f).gamma_multiply(0.6)),
                    ),
                    None => cell(7 + k, "·".into(), muted(), None),
                }
            }
            let k = 7 + Exchange::BOARD.len();
            cell(
                k,
                r.premium.map_or("—".into(), |x| format!("{:+.3}%", x * 100.0)),
                signed_color(r.premium.unwrap_or(0.0)),
                None,
            );
            cell(k + 1, r.long_short.map_or("—".into(), |x| format!("{x:.2}")), text(), None);
            if resp.clicked() {
                action = Some(Action::Command(format!("DERIV {}", r.coin)));
            }
            resp.on_hover_text(format!(
                "{}: {} venues · funding spread {} · right-click for more",
                r.coin,
                r.venues.len(),
                r.funding_spread().map_or("—".into(), |s| fmt_funding(s, d.apr))
            ))
            .context_menu(|ui| {
                for (label, cmd) in [
                    ("Detail", format!("DERIV {}", r.coin)),
                    ("Chart", format!("{}USD CRNCY", r.coin)),
                    ("Order book heatmap", format!("HEAT {}", r.coin)),
                    ("Footprint", format!("FOOT {}", r.coin)),
                    ("Liquidations", format!("LIQ {}", r.coin)),
                ] {
                    if ui.button(label).clicked() {
                        action = Some(Action::Command(cmd));
                        ui.close();
                    }
                }
            });
        }
    });
    action
}

// ---- DERIV detail --------------------------------------------------------------------

fn deriv_detail(ui: &mut Ui, d: &mut DerivPane, coin: &str, board: Option<&Board>) -> Option<Action> {
    let mut action = None;
    let row = board.and_then(|b| b.row(coin));
    ui.horizontal_wrapped(|ui| {
        if ui.button(RichText::new("‹ All").font(ui_font(11.5))).on_hover_text("Back to the board").clicked() {
            d.set_coin(None);
        }
        title(ui, &format!("{coin} PERPETUALS"));
        if let Some(r) = row {
            ui.label(RichText::new(fmt_price(r.price)).font(mono(13.0)).color(text()));
            if let Some(c) = r.change_24h {
                ui.label(RichText::new(pct(c, 2)).font(mono(12.0)).color(signed_color(c)));
            }
            ui.label(RichText::new(format!("OI {}", fmt_usd(r.oi_usd))).font(mono(12.0)).color(text()));
            if let Some(f) = r.funding_8h {
                ui.label(
                    RichText::new(format!("funding {}", fmt_funding(f, d.apr))).font(mono(12.0)).color(signed_color(f)),
                );
            }
        }
        if board.is_some_and(|b| b.simulated) || d.history.as_ref().is_some_and(|h| h.simulated) {
            simulated_badge(ui, "derivatives figures");
        }
    });
    ui.horizontal_wrapped(|ui| {
        if let Some(i) = widgets::segmented(ui, "deriv-apr-detail", &["8h", "APR"], Some(usize::from(d.apr)), 20.0) {
            d.apr = i == 1;
        }
        for (label, cmd) in [
            ("Chart", format!("{coin}USD CRNCY")),
            ("Heatmap", format!("HEAT {coin}")),
            ("Footprint", format!("FOOT {coin}")),
            ("Liquidations", format!("LIQ {coin}")),
        ] {
            if ui.button(RichText::new(label).font(ui_font(11.5))).clicked() {
                action = Some(Action::Command(cmd));
            }
        }
    });
    egui::ScrollArea::vertical().id_salt(("deriv-detail", coin)).auto_shrink([false, false]).show(ui, |ui| {
        if let Some(r) = row {
            venue_table(ui, r, d.apr);
        } else if board.is_some() {
            note(ui, format!("{coin} is not listed as a USDT perpetual on the tracked venues."));
        }
        ui.add_space(dp(6.0));
        let Some(h) = &d.history else {
            note(ui, "Loading history (Binance) and term structure (Deribit)…");
            return;
        };
        for e in &h.errors {
            note(ui, format!("history: {e}"));
        }
        let spark = |title: String, pts: &[(i64, f64)], bars, fmt: fn(f64) -> String, color| Spark {
            title,
            pts: pts.to_vec(),
            bars,
            fmt,
            color,
        };
        let mut charts = vec![
            spark("Open interest · Binance · 7d".into(), &h.oi_usd, false, fmt_usd, theme::cyan()),
            spark(
                "Funding per interval · Binance".into(),
                &h.funding,
                true,
                |x| format!("{:+.4}%", x * 100.0),
                theme::up(),
            ),
            spark("Long/short accounts · Binance".into(), &h.long_short, false, |x| format!("{x:.2}"), amber()),
            spark(
                "Taker buy/sell volume · Binance".into(),
                &h.taker_ratio,
                false,
                |x| format!("{x:.3}"),
                theme::series(2),
            ),
        ];
        let now = chrono::Utc::now().timestamp();
        if let Some(t) = &d.term {
            let curve: Vec<(i64, f64)> = t
                .points
                .iter()
                .filter_map(|p| Some(((p.days_to_expiry(now)? * 86_400.0) as i64, p.annualised(now)?)))
                .collect();
            let basis = format!("{} futures basis, annualised · Deribit", t.currency);
            charts.push(spark(basis, &curve, false, |x| format!("{:.2}%", x * 100.0), theme::series(3)));
            let dvol = format!("{} DVOL implied volatility · Deribit", t.currency);
            charts.push(spark(dvol, &t.dvol, false, |x| format!("{x:.1}"), theme::series(4)));
        }
        let h_cell = dp(130.0);
        let rows =
            charts.len().div_ceil(((ui.available_width() / dp(300.0)).floor() as usize).clamp(1, charts.len().max(1)));
        let (area, _) = ui.allocate_exact_size(vec2(ui.available_width(), h_cell * rows as f32), Sense::hover());
        for (cell, c) in grid(area, charts.len(), dp(300.0), h_cell).into_iter().zip(&charts) {
            mini_chart(ui, cell, &c.title, &c.pts, c.bars, &c.fmt, c.color);
        }
        if let Some(t) = &d.term {
            ui.add_space(dp(4.0));
            term_table(ui, t, now);
        }
    });
    action
}

fn venue_table(ui: &mut Ui, r: &CoinRow, apr: bool) {
    let heads = ["Venue", "Price", "Mark", "Index", "Premium", "Rate", "Every", "Funding", "Next", "OI", "Vol 24h"];
    let widths = [84.0, 92.0, 92.0, 92.0, 70.0, 74.0, 46.0, 78.0, 58.0, 76.0, 76.0];
    let s = dp(1.0);
    let row_h = dp(19.0);
    let total: f32 = widths.iter().map(|w| w * s).sum();
    let (hdr, _) = ui.allocate_exact_size(vec2(total.max(ui.available_width()), row_h), Sense::hover());
    let p = ui.painter().clone();
    p.rect_filled(hdr, 3.0, theme::header());
    let put = |p: &egui::Painter, top: f32, k: usize, t: &str, c: Color32, font: egui::FontId| {
        let left = hdr.left() + widths[..k].iter().map(|w| w * s).sum::<f32>();
        let y = top + row_h / 2.0;
        if k == 0 {
            p.text(pos2(left + 6.0, y), Align2::LEFT_CENTER, t, font, c);
        } else {
            p.text(pos2(left + widths[k] * s - 6.0, y), Align2::RIGHT_CENTER, t, font, c);
        }
    };
    for (k, h) in heads.iter().enumerate() {
        put(&p, hdr.top(), k, h, muted(), ui_font(11.0));
    }
    let now = chrono::Utc::now().timestamp_millis();
    for v in r.venues.values() {
        let (rect, _) = ui.allocate_exact_size(vec2(total.max(ui.available_width()), row_h), Sense::hover());
        let t = rect.top();
        let opt = |x: Option<f64>| x.map_or("—".to_string(), fmt_price);
        put(&p, t, 0, v.exchange.label(), amber(), mono(12.0));
        put(&p, t, 1, &fmt_price(v.price), text(), mono(12.0));
        put(&p, t, 2, &opt(v.mark), text(), mono(12.0));
        put(&p, t, 3, &opt(v.index), muted(), mono(12.0));
        let prem = v.premium();
        put(
            &p,
            t,
            4,
            &prem.map_or("—".into(), |x| format!("{:+.3}%", x * 100.0)),
            signed_color(prem.unwrap_or(0.0)),
            mono(12.0),
        );
        put(&p, t, 5, &v.funding.map_or("—".into(), |x| format!("{:+.4}%", x * 100.0)), text(), mono(12.0));
        put(&p, t, 6, &format!("{:.0}h", v.funding_hours), muted(), mono(12.0));
        let f8 = v.funding_8h();
        put(&p, t, 7, &f8.map_or("—".into(), |f| fmt_funding(f, apr)), signed_color(f8.unwrap_or(0.0)), mono(12.0));
        let next = v.next_funding.filter(|n| *n > now).map(|n| {
            let m = (n - now) / 60_000;
            format!("{}:{:02}", m / 60, m % 60)
        });
        put(&p, t, 8, next.as_deref().unwrap_or("—"), muted(), mono(12.0));
        put(&p, t, 9, &v.oi_usd.map_or("—".into(), fmt_usd), text(), mono(12.0));
        put(&p, t, 10, &v.volume_usd.map_or("—".into(), fmt_usd), text(), mono(12.0));
    }
}

fn term_table(ui: &mut Ui, t: &TermStructure, now: i64) {
    ui.label(RichText::new(format!("{} FUTURES CURVE (Deribit)", t.currency)).font(mono(11.5)).color(amber()));
    if let Some(e) = &t.error {
        note(ui, e.clone());
    }
    egui::Grid::new(("term", &t.currency)).striped(true).spacing(vec2(dp(14.0), dp(2.0))).show(ui, |ui| {
        for h in ["Contract", "Days", "Mark", "Basis", "Annualised", "OI"] {
            ui.label(RichText::new(h).font(ui_font(11.0)).color(muted()));
        }
        ui.end_row();
        for p in &t.points {
            ui.label(RichText::new(&p.instrument).font(mono(12.0)).color(text()));
            ui.label(
                RichText::new(p.days_to_expiry(now).map_or("perp".into(), |d| format!("{d:.0}")))
                    .font(mono(12.0))
                    .color(muted()),
            );
            ui.label(RichText::new(fmt_price(p.mark)).font(mono(12.0)).color(text()));
            ui.label(
                RichText::new(format!("{:+.3}%", p.basis() * 100.0)).font(mono(12.0)).color(signed_color(p.basis())),
            );
            ui.label(
                RichText::new(p.annualised(now).map_or("—".into(), |a| format!("{:.2}%", a * 100.0)))
                    .font(mono(12.0))
                    .color(text()),
            );
            ui.label(RichText::new(fmt_usd(p.oi_usd)).font(mono(12.0)).color(text()));
            ui.end_row();
        }
    });
}

// ---- LIQ -----------------------------------------------------------------------------

fn liquidations(ui: &mut Ui, l: &mut LiqPane, tape: Option<&SharedTape>) -> Option<Action> {
    let mut action = None;
    ui.horizontal_wrapped(|ui| {
        title(
            ui,
            &match &l.coin {
                Some(c) => format!("{c} LIQUIDATIONS"),
                None => "LIQUIDATIONS".into(),
            },
        );
        let windows = [5i64, 60, 240, 1440];
        if let Some(i) = widgets::segmented(
            ui,
            "liq-win",
            &["5m", "1h", "4h", "24h"],
            windows.iter().position(|w| *w == l.window_min),
            20.0,
        ) {
            l.window_min = windows[i];
        }
        let mins = [0.0, 10_000.0, 100_000.0, 1_000_000.0];
        if let Some(i) = widgets::segmented(
            ui,
            "liq-min",
            &["All", "$10K+", "$100K+", "$1M+"],
            mins.iter().position(|m| *m == l.min_usd),
            20.0,
        ) {
            l.min_usd = mins[i];
        }
        if l.coin.is_some() && ui.button(RichText::new("All coins").font(ui_font(11.5))).clicked() {
            l.coin = None;
        }
    });
    let Some(tape) = tape else {
        note(ui, "Connecting to Binance, Bybit and OKX liquidation streams…");
        ui.add(egui::Spinner::new());
        return None;
    };
    let Ok(t) = tape.lock() else { return None };
    ui.horizontal_wrapped(|ui| {
        for e in [Exchange::Binance, Exchange::Bybit, Exchange::Okx] {
            let s = t.status.get(&e).map_or("waiting", String::as_str);
            let c = if s == "live" || s == "simulated" { theme::up() } else { muted() };
            ui.label(RichText::new(format!("● {}", e.short())).font(mono(11.0)).color(c))
                .on_hover_text(format!("{}: {s}", e.label()));
        }
        if t.simulated {
            simulated_badge(ui, "liquidations");
        }
        note(ui, "Binance reports at most one liquidation per contract per second.");
    });
    let since = chrono::Utc::now().timestamp_millis() - l.window_min * 60_000;
    let tot = t.totals(since, l.coin.as_deref());
    // Long/short split bar.
    let (bar, _) = ui.allocate_exact_size(vec2(ui.available_width(), dp(34.0)), Sense::hover());
    let p = ui.painter_at(bar);
    let sum = (tot.long_usd + tot.short_usd).max(1e-9);
    let split = bar.left() + bar.width() * (tot.long_usd / sum) as f32;
    let strip = Rect::from_min_max(pos2(bar.left(), bar.bottom() - dp(8.0)), pos2(bar.right(), bar.bottom() - dp(2.0)));
    p.rect_filled(Rect::from_min_max(strip.min, pos2(split, strip.bottom())), 2.0, theme::down());
    p.rect_filled(Rect::from_min_max(pos2(split, strip.top()), strip.max), 2.0, theme::up());
    p.text(
        pos2(bar.left(), bar.top() + dp(9.0)),
        Align2::LEFT_CENTER,
        format!("Longs rekt {}", fmt_usd(tot.long_usd)),
        mono(12.5),
        theme::down(),
    );
    p.text(
        pos2(bar.right(), bar.top() + dp(9.0)),
        Align2::RIGHT_CENTER,
        format!("{} Shorts rekt", fmt_usd(tot.short_usd)),
        mono(12.5),
        theme::up(),
    );
    p.text(
        pos2(bar.center().x, bar.top() + dp(9.0)),
        Align2::CENTER_CENTER,
        format!("{} orders", tot.count),
        ui_font(11.0),
        muted(),
    );
    let rest = ui.available_rect_before_wrap();
    let wide = rest.width() > dp(620.0);
    let (left, right) = if wide && l.coin.is_none() {
        let split = rest.left() + dp(230.0);
        (
            Some(Rect::from_min_max(rest.min, pos2(split - dp(6.0), rest.bottom()))),
            Rect::from_min_max(pos2(split, rest.top()), rest.max),
        )
    } else {
        (None, rest)
    };
    if let Some(left) = left {
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(left));
        child.label(RichText::new("BY COIN").font(mono(11.0)).color(muted()));
        egui::ScrollArea::vertical().id_salt("liq-by-coin").auto_shrink([false, false]).show(&mut child, |ui| {
            for (coin, ct) in t.by_coin(since).into_iter().take(40) {
                let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), dp(18.0)), Sense::click());
                widgets::hover_wash(ui, r, &resp, false);
                let p = ui.painter();
                let total = (ct.long_usd + ct.short_usd).max(1e-9);
                p.text(pos2(r.left() + 4.0, r.center().y), Align2::LEFT_CENTER, &coin, mono(12.0), amber());
                p.text(pos2(r.right() - 4.0, r.center().y), Align2::RIGHT_CENTER, fmt_usd(total), mono(12.0), text());
                let w = dp(50.0);
                let bx = Rect::from_min_size(pos2(r.left() + dp(56.0), r.center().y - 3.0), vec2(w, 6.0));
                p.rect_filled(
                    Rect::from_min_size(bx.min, vec2(w * (ct.long_usd / total) as f32, 6.0)),
                    1.0,
                    theme::down(),
                );
                p.rect_filled(
                    Rect::from_min_max(pos2(bx.left() + w * (ct.long_usd / total) as f32, bx.top()), bx.max),
                    1.0,
                    theme::up(),
                );
                if resp.clicked() {
                    l.coin = Some(coin.clone());
                }
            }
        });
        ui.advance_cursor_after_rect(left);
    }
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(right));
    let ui = &mut child;
    let row_h = dp(18.0);
    let items: Vec<_> = t
        .items
        .iter()
        .take_while(|x| x.ts >= since)
        .filter(|x| x.usd >= l.min_usd && l.coin.as_deref().is_none_or(|c| x.coin.eq_ignore_ascii_case(c)))
        .take(1_000)
        .cloned()
        .collect();
    drop(t);
    if items.is_empty() {
        note(ui, "No liquidations in this window yet.");
    }
    let max_log = items.iter().map(|x| x.usd.max(1.0).ln()).fold(1.0, f64::max);
    egui::ScrollArea::vertical().id_salt("liq-tape").auto_shrink([false, false]).show_rows(
        ui,
        row_h,
        items.len(),
        |ui, range| {
            for x in &items[range] {
                let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), row_h), Sense::click());
                let p = ui.painter();
                let c = if x.side == Side::Long { theme::down() } else { theme::up() };
                let frac = (x.usd.max(1.0).ln() / max_log).clamp(0.05, 1.0) as f32;
                p.rect_filled(
                    Rect::from_min_max(
                        pos2(r.right() - r.width() * frac * 0.5, r.top() + 1.0),
                        pos2(r.right(), r.bottom() - 1.0),
                    ),
                    2.0,
                    c.gamma_multiply(0.12),
                );
                let big = x.usd >= 100_000.0;
                let font = if big { mono(12.5) } else { mono(12.0) };
                let cols = [0.0, 70.0, 108.0, 170.0];
                p.text(
                    pos2(r.left() + cols[0], r.center().y),
                    Align2::LEFT_CENTER,
                    local_hms(x.ts),
                    mono(11.5),
                    muted(),
                );
                p.text(
                    pos2(r.left() + dp(cols[1]), r.center().y),
                    Align2::LEFT_CENTER,
                    x.exchange.short(),
                    mono(11.5),
                    muted(),
                );
                p.text(pos2(r.left() + dp(cols[2]), r.center().y), Align2::LEFT_CENTER, &x.coin, font.clone(), amber());
                p.text(
                    pos2(r.left() + dp(cols[3]), r.center().y),
                    Align2::LEFT_CENTER,
                    if x.side == Side::Long { "LONG" } else { "SHORT" },
                    font.clone(),
                    c,
                );
                p.text(
                    pos2(r.right() - dp(90.0), r.center().y),
                    Align2::RIGHT_CENTER,
                    fmt_price(x.price),
                    mono(12.0),
                    text(),
                );
                p.text(
                    pos2(r.right() - 4.0, r.center().y),
                    Align2::RIGHT_CENTER,
                    fmt_usd(x.usd),
                    font,
                    if big { c } else { text() },
                );
                if resp.clicked() {
                    action = Some(Action::Command(format!("DERIV {}", x.coin)));
                }
            }
        },
    );
    ui.ctx().request_repaint_after(Duration::from_millis(500));
    action
}

// ---- HEAT / FOOT ---------------------------------------------------------------------

/// Bookmap-style ramp: deep blue → cyan → yellow → white.
fn heat_color(t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let stops: [(f32, [f32; 3]); 5] = [
        (0.0, [8.0, 10.0, 30.0]),
        (0.25, [20.0, 60.0, 140.0]),
        (0.5, [0.0, 190.0, 210.0]),
        (0.8, [250.0, 220.0, 40.0]),
        (1.0, [255.0, 255.0, 255.0]),
    ];
    let k = stops.iter().position(|s| s.0 >= t).unwrap_or(4).max(1);
    let (a, b) = (stops[k - 1], stops[k]);
    let u = (t - a.0) / (b.0 - a.0).max(1e-6);
    let c = |i: usize| (a.1[i] + (b.1[i] - a.1[i]) * u) as u8;
    Color32::from_rgb(c(0), c(1), c(2))
}

fn flow_header(ui: &mut Ui, f: &mut FlowPane) -> Option<Action> {
    let mut action = None;
    let mut retarget = None;
    ui.horizontal_wrapped(|ui| {
        let r = ui.label(RichText::new(&f.coin).font(mono(13.0)).color(amber())).interact(Sense::click());
        if r.on_hover_text("Open chart").clicked() {
            action = Some(Action::OpenChart(format!("{}USD Curncy", f.coin)));
        }
        let venues = [Exchange::Binance, Exchange::Hyperliquid];
        if let Some(i) = widgets::segmented(
            ui,
            ("flow-venue", &f.coin),
            &["Binance", "Hyperliquid"],
            venues.iter().position(|v| *v == f.venue),
            20.0,
        ) {
            retarget = Some(venues[i]);
        }
        let views = [FlowView::Heatmap, FlowView::Footprint];
        if let Some(i) = widgets::segmented(
            ui,
            ("flow-view", &f.coin),
            &["Heatmap", "Footprint"],
            views.iter().position(|v| *v == f.view),
            20.0,
        ) {
            f.view = views[i];
        }
        if f.view == FlowView::Footprint {
            let tfs = [60i64, 300, 900, 3600];
            if let Some(i) = widgets::segmented(
                ui,
                ("flow-tf", &f.coin),
                &["1m", "5m", "15m", "1h"],
                tfs.iter().position(|t| *t == f.tf_secs),
                20.0,
            ) {
                f.tf_secs = tfs[i];
                if let Some(fl) = &f.flow
                    && let Ok(mut g) = fl.lock()
                {
                    g.set_timeframe(f.tf_secs);
                }
            }
        }
        if ui.small_button("−").on_hover_text("Zoom out (or scroll)").clicked() {
            f.zoom = (f.zoom * 1.25).min(20.0);
        }
        if ui.small_button("+").on_hover_text("Zoom in (or scroll)").clicked() {
            f.zoom = (f.zoom / 1.25).max(0.05);
        }
        match f
            .flow
            .as_ref()
            .and_then(|fl| fl.lock().ok().map(|g| (g.status.clone(), g.last, g.simulated, g.symbol.clone(), g.since)))
        {
            Some((status, last, sim, sym, since)) => {
                if let Some(px) = last {
                    ui.label(RichText::new(fmt_price(px)).font(mono(13.0)).color(text()));
                }
                let c = if status == "live" || status == "simulated" { theme::up() } else { theme::down() };
                ui.label(RichText::new(format!("● {sym}")).font(mono(11.0)).color(c)).on_hover_text(status);
                if let Some(s) = since {
                    note(ui, format!("since {}", local_hms(s)));
                }
                if sim {
                    simulated_badge(ui, "trades and the order book");
                }
            }
            None => note(ui, "connecting…"),
        }
    });
    if let Some(v) = retarget {
        let coin = f.coin.clone();
        f.retarget(&coin, v);
    }
    action
}

fn flow_pane(ui: &mut Ui, f: &mut FlowPane) -> Option<Action> {
    let action = flow_header(ui, f);
    let rect = ui.available_rect_before_wrap();
    let resp = ui.allocate_rect(rect, Sense::click_and_drag());
    if resp.hovered() {
        let dy = ui.input(|i| i.smooth_scroll_delta.y);
        if dy != 0.0 {
            f.zoom = (f.zoom * (1.0 - dy * 0.002).clamp(0.5, 1.5)).clamp(0.05, 20.0);
        }
    }
    if f.view == FlowView::Footprint && resp.dragged() {
        let w = dp(84.0) / f.zoom.sqrt();
        let delta = (resp.drag_delta().x / w).round() as i64;
        f.offset = (f.offset as i64 + delta).max(0) as usize;
    }
    let Some(flow) = f.flow.clone() else {
        ui.painter().text(
            rect.center(),
            Align2::CENTER_CENTER,
            "Connecting to the trade and depth streams…",
            ui_font(12.0),
            muted(),
        );
        return action;
    };
    match f.view {
        FlowView::Heatmap => heatmap(ui, f, &flow, rect, resp.hover_pos()),
        FlowView::Footprint => footprint(ui, f, &flow, rect, resp.hover_pos()),
    }
    ui.ctx().request_repaint_after(Duration::from_millis(250));
    action
}

const AXIS_W: f32 = 74.0;

fn price_axis(p: &egui::Painter, plot: Rect, lo: f64, hi: f64, last: Option<f64>) {
    let y = |v: f64| plot.bottom() - ((v - lo) / (hi - lo)) as f32 * plot.height();
    let step = hd_flow::nice_step((hi - lo) / (plot.height() / dp(36.0)).max(1.0) as f64);
    let mut v = (lo / step).ceil() * step;
    let x = plot.right() + 4.0;
    while v <= hi {
        p.hline(plot.x_range(), y(v), Stroke::new(1.0, theme::grid().gamma_multiply(0.5)));
        p.text(pos2(x, y(v)), Align2::LEFT_CENTER, fmt_price(v), mono(11.0), muted());
        v += step;
    }
    if let Some(l) = last.filter(|l| (lo..=hi).contains(l)) {
        let r = Rect::from_min_size(pos2(plot.right(), y(l) - 8.0), vec2(AXIS_W, 16.0));
        p.rect_filled(r, 3.0, amber());
        p.text(pos2(x, y(l)), Align2::LEFT_CENTER, fmt_price(l), mono(11.0), Color32::BLACK);
    }
}

fn heatmap(ui: &mut Ui, f: &mut FlowPane, flow: &SharedFlow, rect: Rect, hover: Option<Pos2>) {
    let Ok(g) = flow.lock() else { return };
    let cvd_h = dp(54.0);
    let plot = Rect::from_min_max(rect.min, pos2(rect.right() - dp(AXIS_W), rect.bottom() - cvd_h - 4.0));
    let cvd_r = Rect::from_min_max(pos2(rect.left(), plot.bottom() + 4.0), pos2(plot.right(), rect.bottom()));
    let p = ui.painter_at(rect);
    p.rect_filled(plot, 0.0, heat_color(0.0));
    let heat = &g.heat;
    let n_vis = (plot.width() / dp(2.0)).floor().max(1.0) as usize;
    let cols: Vec<_> = heat.cols.iter().skip(heat.cols.len().saturating_sub(n_vis)).collect();
    let Some(last) = cols.last() else {
        p.text(
            plot.center(),
            Align2::CENTER_CENTER,
            format!("{} · waiting for the order book…", g.status),
            ui_font(12.0),
            muted(),
        );
        return;
    };
    let mid = (last.bid + last.ask) / 2.0;
    let half = mid * 0.004 * f.zoom as f64;
    let (lo, hi) = (mid - half, mid + half);
    let step = heat.step.max(1e-12);
    let (b_lo, b_hi) = (heat.bucket(lo), heat.bucket(hi));
    let rows = ((b_hi - b_lo + 1).clamp(1, 2_000)) as usize;
    // Bake the visible grid into a texture once per new column (or zoom change).
    let key = (last.ts, cols.len(), lo, hi);
    if f.baked.0 != key.0
        || f.baked.1 != key.1
        || (f.baked.2 - lo).abs() > step * 0.5
        || (f.baked.3 - hi).abs() > step * 0.5
        || f.texture.is_none()
    {
        let mut sizes: Vec<f32> =
            cols.iter().flat_map(|c| (b_lo..=b_hi).map(move |b| c.size_at(b))).filter(|s| *s > 0.0).collect();
        sizes.sort_by(f32::total_cmp);
        // Log scale, saturating at the 97th percentile, so ordinary liquidity stays visible
        // next to a few large walls.
        let cap = sizes.get(sizes.len() * 97 / 100).copied().unwrap_or(1.0).max(1e-9);
        let norm = (1.0 + cap / sizes.get(sizes.len() / 2).copied().unwrap_or(cap).max(1e-9) * 4.0).ln();
        let med = sizes.get(sizes.len() / 2).copied().unwrap_or(1.0).max(1e-9);
        let mut px = Vec::with_capacity(cols.len() * rows);
        for r in 0..rows {
            let b = b_hi - r as i64;
            for c in &cols {
                let s = c.size_at(b);
                px.push(heat_color(if s > 0.0 { (1.0 + s / med * 4.0).ln() / norm } else { 0.0 }));
            }
        }
        let img = ColorImage::new([cols.len(), rows], px);
        match &mut f.texture {
            Some(t) => t.set(img, TextureOptions::NEAREST),
            None => f.texture = Some(ui.ctx().load_texture(format!("heat-{}", f.coin), img, TextureOptions::NEAREST)),
        }
        f.baked = key;
    }
    let col_w = plot.width() / n_vis as f32;
    let x0 = plot.right() - cols.len() as f32 * col_w;
    let y = |v: f64| plot.bottom() - ((v - lo) / (hi - lo)) as f32 * plot.height();
    if let Some(t) = &f.texture {
        // Texture rows are whole buckets; align them to prices.
        let top = y((b_hi + 1) as f64 * step);
        let bottom = y(b_lo as f64 * step);
        p.with_clip_rect(plot).image(
            t.id(),
            Rect::from_min_max(pos2(x0, top), pos2(plot.right(), bottom)),
            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
            Color32::WHITE,
        );
    }
    let pc = p.with_clip_rect(plot);
    // Trades: bubbles sized by volume, coloured by aggressor; small prints are skipped so
    // the book stays readable.
    let mut vols: Vec<f64> = cols.iter().map(|c| c.buy + c.sell).collect();
    let vmax = vols.iter().copied().fold(0.0, f64::max).max(1e-12);
    vols.sort_by(f64::total_cmp);
    let floor = vols.get(vols.len() * 3 / 4).copied().unwrap_or(0.0);
    for (k, c) in cols.iter().enumerate() {
        let v = c.buy + c.sell;
        let Some(px) = c.vwap.filter(|_| v > floor && v > 0.0) else { continue };
        let r = (dp(10.0) * (v / vmax).sqrt() as f32).max(2.0);
        let col = if c.buy >= c.sell { theme::up() } else { theme::down() };
        pc.circle(pos2(x0 + (k as f32 + 0.5) * col_w, y(px)), r, col.gamma_multiply(0.45), Stroke::new(1.0, col));
    }
    // Best bid / ask on top.
    let line = |sel: &dyn Fn(&hd_flow::orderflow::HeatCol) -> f64, c: Color32| {
        let pts: Vec<Pos2> =
            cols.iter().enumerate().map(|(k, col)| pos2(x0 + (k as f32 + 0.5) * col_w, y(sel(col)))).collect();
        pc.add(Shape::line(pts, Stroke::new(1.5, c)));
    };
    line(&|c| c.bid, theme::up());
    line(&|c| c.ask, theme::down());
    // Current book profile along the right edge.
    let prof_w = dp(48.0);
    let cap = last.max_size().max(1e-9);
    for b in b_lo..=b_hi {
        let s = last.size_at(b);
        if s > 0.0 {
            let w = prof_w * (s / cap).sqrt();
            let yy = y((b as f64 + 0.5) * step);
            let c = if (b as f64 * step) < mid { theme::up() } else { theme::down() };
            pc.hline(
                (plot.right() - w)..=plot.right(),
                yy,
                Stroke::new((plot.height() / rows as f32).clamp(1.0, 6.0), c.gamma_multiply(0.6)),
            );
        }
    }
    price_axis(&p, plot, lo, hi, g.last);
    // CVD over the visible columns.
    let mut cum = 0.0;
    let cvd: Vec<(i64, f64)> = cols
        .iter()
        .map(|c| {
            cum += c.buy - c.sell;
            (c.ts / 1000, cum)
        })
        .collect();
    mini_chart(ui, cvd_r, "CVD (visible window)", &cvd, false, &|x| format!("{x:+.2}"), theme::cyan());
    if let Some(h) = hover.filter(|h| plot.contains(*h)) {
        pc.hline(plot.x_range(), h.y, Stroke::new(1.0, Color32::from_white_alpha(90)));
        pc.vline(h.x, plot.y_range(), Stroke::new(1.0, Color32::from_white_alpha(90)));
        let price = lo + (plot.bottom() - h.y) as f64 / plot.height() as f64 * (hi - lo);
        let k = ((h.x - x0) / col_w).floor();
        if k >= 0.0
            && let Some(c) = cols.get(k as usize)
        {
            let size = c.size_at(heat.bucket(price));
            let tip = format!("{}  {}  resting {:.3} {}", local_hms(c.ts), fmt_price(price), size, g.coin);
            p.text(pos2(plot.left() + 6.0, plot.top() + 10.0), Align2::LEFT_CENTER, tip, mono(11.5), Color32::WHITE);
        }
    }
}

fn footprint(ui: &mut Ui, f: &mut FlowPane, flow: &SharedFlow, rect: Rect, hover: Option<Pos2>) {
    let Ok(g) = flow.lock() else { return };
    let foot = &g.foot;
    let delta_h = dp(18.0);
    let cvd_h = dp(54.0);
    let plot = Rect::from_min_max(rect.min, pos2(rect.right() - dp(AXIS_W), rect.bottom() - cvd_h - delta_h - 6.0));
    let p = ui.painter_at(rect);
    let cw = (dp(84.0) / f.zoom.sqrt()).clamp(dp(6.0), dp(240.0));
    let n_vis = (plot.width() / cw).floor().max(1.0) as usize;
    let len = foot.candles.len();
    f.offset = f.offset.min(len.saturating_sub(1));
    let end = len - f.offset.min(len);
    let candles: Vec<_> = foot.candles.iter().take(end).skip(end.saturating_sub(n_vis)).collect();
    if candles.is_empty() {
        p.text(
            plot.center(),
            Align2::CENTER_CENTER,
            format!("{} · building footprints from live trades…", g.status),
            ui_font(12.0),
            muted(),
        );
        return;
    }
    let step = foot.step.max(1e-12);
    let lo = candles.iter().map(|c| c.low).fold(f64::INFINITY, f64::min) - step;
    let hi = candles.iter().map(|c| c.high).fold(f64::NEG_INFINITY, f64::max) + 2.0 * step;
    let y = |v: f64| plot.bottom() - ((v - lo) / (hi - lo)) as f32 * plot.height();
    let row_h = (y(0.0) - y(step)).abs();
    let x0 = plot.right() - candles.len() as f32 * cw;
    let pc = p.with_clip_rect(plot);
    let vmax = candles.iter().flat_map(|c| c.levels.values().map(|l| l.0.max(l.1))).fold(0.0, f64::max).max(1e-12);
    let show_text = row_h >= dp(11.0) && cw >= dp(60.0);
    let fmt_q = |q: f64| {
        if q >= 1000.0 {
            format!("{:.1}K", q / 1000.0)
        } else if q >= 10.0 {
            format!("{q:.0}")
        } else if q >= 1.0 {
            format!("{q:.1}")
        } else {
            format!("{q:.2}")
        }
    };
    for (k, c) in candles.iter().enumerate() {
        let left = x0 + k as f32 * cw;
        let body_x = left + dp(5.0);
        let cells = Rect::from_min_max(pos2(left + dp(10.0), plot.top()), pos2(left + cw - 2.0, plot.bottom()));
        let poc = c.poc();
        for (&b, &(buy, sell)) in &c.levels {
            let top = y((b + 1) as f64 * step);
            let r = Rect::from_min_max(pos2(cells.left(), top + 0.5), pos2(cells.right(), y(b as f64 * step) - 0.5));
            let tot = buy + sell;
            let imb = if tot > 0.0 { (buy - sell) / tot } else { 0.0 };
            let base = if imb >= 0.0 { theme::up() } else { theme::down() };
            let a = (0.12 + 0.6 * (tot / vmax).sqrt() as f32 * (0.4 + 0.6 * imb.abs() as f32)).min(0.85);
            pc.rect_filled(r, 1.0, base.gamma_multiply(a));
            if Some(b) == poc {
                pc.rect_stroke(r, 1.0, Stroke::new(1.0, amber()), egui::StrokeKind::Inside);
            }
            if show_text {
                let mid = r.center();
                pc.text(pos2(mid.x - 3.0, mid.y), Align2::RIGHT_CENTER, fmt_q(sell), mono(10.5), text());
                pc.text(pos2(mid.x + 3.0, mid.y), Align2::LEFT_CENTER, fmt_q(buy), mono(10.5), text());
                pc.vline(mid.x, (r.top() + 2.0)..=(r.bottom() - 2.0), Stroke::new(1.0, muted().gamma_multiply(0.5)));
            }
        }
        let up = c.close >= c.open;
        let col = if up { theme::candle_up() } else { theme::candle_down() };
        pc.vline(body_x, y(c.high)..=y(c.low), Stroke::new(1.0, col));
        let (bt, bb) = (y(c.open.max(c.close)), y(c.open.min(c.close)));
        pc.rect_filled(Rect::from_min_max(pos2(body_x - 2.5, bt), pos2(body_x + 2.5, bb.max(bt + 1.0))), 0.0, col);
        // Delta row.
        let dr = Rect::from_min_max(
            pos2(left + 1.0, plot.bottom() + 3.0),
            pos2(left + cw - 1.0, plot.bottom() + 3.0 + delta_h),
        );
        let dl = c.delta();
        p.rect_filled(dr, 2.0, signed_color(dl).gamma_multiply(0.15));
        if cw >= dp(34.0) {
            let label = format!("{}{}", if dl < 0.0 { "-" } else { "+" }, fmt_q(dl.abs()));
            p.text(dr.center(), Align2::CENTER_CENTER, label, mono(10.5), signed_color(dl));
        }
    }
    price_axis(&p, plot, lo, hi, g.last);
    let cvd: Vec<(i64, f64)> = candles.iter().map(|c| (c.ts, c.cvd)).collect();
    let cvd_r = Rect::from_min_max(pos2(rect.left(), plot.bottom() + delta_h + 6.0), pos2(plot.right(), rect.bottom()));
    mini_chart(
        ui,
        cvd_r,
        &format!("CVD · {} · bucket {}", g.symbol, fmt_price(step)),
        &cvd,
        false,
        &|x| format!("{x:+.2}"),
        theme::cyan(),
    );
    if let Some(h) = hover.filter(|h| plot.contains(*h)) {
        let k = ((h.x - x0) / cw).floor();
        if k >= 0.0
            && let Some(c) = candles.get(k as usize)
        {
            let t = chrono::DateTime::from_timestamp(c.ts, 0)
                .map(|d| d.with_timezone(&chrono::Local).format("%H:%M").to_string())
                .unwrap_or_default();
            let tip = format!(
                "{t}  O {} H {} L {} C {}  vol {}  Δ {:+.3}  CVD {:+.3}",
                fmt_price(c.open),
                fmt_price(c.high),
                fmt_price(c.low),
                fmt_price(c.close),
                fmt_q(c.volume()),
                c.delta(),
                c.cvd
            );
            p.text(pos2(plot.left() + 6.0, plot.top() + 10.0), Align2::LEFT_CENTER, tip, mono(11.5), text());
        }
    }
}

// ---- PRED ----------------------------------------------------------------------------

fn predict(ui: &mut Ui, pp: &mut PredPane, markets: Option<&Result<Vec<Market>, String>>) {
    ui.horizontal_wrapped(|ui| {
        title(ui, "PREDICTION MARKETS");
        note(ui, "Polymarket · by 24h volume");
        ui.add(
            egui::TextEdit::singleline(&mut pp.query).hint_text("search markets").font(mono(12.0)).desired_width(180.0),
        );
        if markets.is_some_and(|m| m.as_ref().is_ok_and(|v| v.first().is_some_and(|x| x.id.starts_with("sample-")))) {
            simulated_badge(ui, "markets and prices");
        }
    });
    let ms = match markets {
        None => {
            ui.add(egui::Spinner::new());
            return;
        }
        Some(Err(e)) => {
            ui.label(RichText::new(format!("Could not load markets: {e}")).font(ui_font(12.0)).color(theme::down()));
            return;
        }
        Some(Ok(ms)) => ms,
    };
    let list: Vec<&Market> = ms.iter().filter(|m| pp.query.trim().is_empty() || m.matches(&pp.query)).collect();
    let rest = ui.available_rect_before_wrap();
    let wide = rest.width() > dp(760.0);
    let selected = pp.selected.as_ref().and_then(|id| ms.iter().find(|m| &m.id == id));
    let (list_r, detail_r) = match (wide, selected.is_some()) {
        (true, _) => {
            let split = rest.left() + rest.width() * 0.48;
            (
                Some(Rect::from_min_max(rest.min, pos2(split - dp(6.0), rest.bottom()))),
                Some(Rect::from_min_max(pos2(split, rest.top()), rest.max)),
            )
        }
        (false, true) => (None, Some(rest)),
        (false, false) => (Some(rest), None),
    };
    if let Some(lr) = list_r {
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(lr));
        let row_h = dp(34.0);
        egui::ScrollArea::vertical().id_salt("pred-list").auto_shrink([false, false]).show_rows(
            &mut child,
            row_h,
            list.len(),
            |ui, range| {
                for m in &list[range] {
                    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), row_h), Sense::click());
                    widgets::hover_wash(ui, r, &resp, pp.selected.as_deref() == Some(m.id.as_str()));
                    let p = ui.painter_at(r);
                    let right = dp(150.0);
                    let q = p.layout(m.question.clone(), ui_font(12.0), text(), (r.width() - right - 8.0).max(60.0));
                    p.galley(pos2(r.left() + 4.0, r.top() + 2.0), q, text());
                    let sub = [m.event.as_deref(), m.category.as_deref(), m.end_date.as_deref()]
                        .into_iter()
                        .flatten()
                        .collect::<Vec<_>>()
                        .join(" · ");
                    p.text(pos2(r.left() + 4.0, r.bottom() - 7.0), Align2::LEFT_CENTER, sub, ui_font(10.5), muted());
                    if let Some((o, px)) = m.lead() {
                        p.text(
                            pos2(r.right() - dp(70.0), r.top() + dp(11.0)),
                            Align2::RIGHT_CENTER,
                            format!("{o} {:.0}¢", px * 100.0),
                            mono(13.0),
                            amber(),
                        );
                    }
                    if let Some(c) = m.change_1d {
                        p.text(
                            pos2(r.right() - 4.0, r.top() + dp(11.0)),
                            Align2::RIGHT_CENTER,
                            format!("{:+.1}", c * 100.0),
                            mono(11.5),
                            signed_color(c),
                        );
                    }
                    p.text(
                        pos2(r.right() - 4.0, r.bottom() - 7.0),
                        Align2::RIGHT_CENTER,
                        format!("24h {}", fmt_usd(m.volume_24h)),
                        mono(10.5),
                        muted(),
                    );
                    if resp.clicked() {
                        pp.selected = Some(m.id.clone());
                        pp.detail = None;
                        pp.loading = false;
                    }
                }
            },
        );
        ui.advance_cursor_after_rect(lr);
    }
    if let (Some(dr), Some(m)) = (detail_r, selected) {
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(dr));
        let ui = &mut child;
        if !wide && ui.button(RichText::new("‹ Markets").font(ui_font(11.5))).clicked() {
            pp.selected = None;
            return;
        }
        let q = ui.painter().layout(m.question.clone(), ui_font(15.0), text(), dr.width());
        let (qr, _) = ui.allocate_exact_size(q.size(), Sense::hover());
        ui.painter().galley(qr.min, q, text());
        ui.horizontal_wrapped(|ui| {
            note(
                ui,
                format!(
                    "Vol {} · 24h {} · liquidity {}",
                    fmt_usd(m.volume),
                    fmt_usd(m.volume_24h),
                    fmt_usd(m.liquidity)
                ),
            );
            if let Some(e) = &m.end_date {
                note(ui, format!("ends {e}"));
            }
            if !m.id.starts_with("sample-")
                && ui.small_button("Open on Polymarket ↗").clicked()
                && let Err(e) = crate::platform::open_external(m.url())
            {
                tracing::warn!("opening {}: {e}", m.url());
            }
        });
        for (k, (o, px)) in m.outcomes.iter().zip(&m.prices).enumerate() {
            let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), dp(20.0)), Sense::hover());
            let p = ui.painter_at(r);
            p.rect_filled(
                Rect::from_min_size(r.min, vec2(r.width() * *px as f32, r.height())),
                3.0,
                theme::series(k).gamma_multiply(0.25),
            );
            p.text(pos2(r.left() + 6.0, r.center().y), Align2::LEFT_CENTER, o, ui_font(12.0), text());
            p.text(
                pos2(r.right() - 6.0, r.center().y),
                Align2::RIGHT_CENTER,
                format!("{:.1}%", px * 100.0),
                mono(12.5),
                theme::series(k),
            );
        }
        ui.add_space(dp(4.0));
        match &pp.detail {
            None => {
                ui.add(egui::Spinner::new());
            }
            Some(d) => {
                if let Some(e) = &d.error {
                    note(ui, e.clone());
                }
                let (cr, _) = ui.allocate_exact_size(vec2(ui.available_width(), dp(170.0)), Sense::hover());
                prob_chart(ui, cr, m, d);
                if let Some(b) = &d.book {
                    let first = m.outcomes.first().map_or("", String::as_str);
                    ui.label(RichText::new(format!("ORDER BOOK · {first}")).font(mono(11.0)).color(muted()));
                    egui::Grid::new(("pm-book", &m.id)).spacing(vec2(dp(18.0), dp(1.0))).show(ui, |ui| {
                        for h in ["Bid size", "Bid", "Ask", "Ask size"] {
                            ui.label(RichText::new(h).font(ui_font(10.5)).color(muted()));
                        }
                        ui.end_row();
                        for k in 0..b.bids.len().max(b.asks.len()).min(10) {
                            let bid = b.bids.get(k);
                            let ask = b.asks.get(k);
                            ui.label(
                                RichText::new(bid.map_or(String::new(), |l| format!("{:.0}", l.1)))
                                    .font(mono(12.0))
                                    .color(text()),
                            );
                            ui.label(
                                RichText::new(bid.map_or(String::new(), |l| format!("{:.1}¢", l.0 * 100.0)))
                                    .font(mono(12.0))
                                    .color(theme::up()),
                            );
                            ui.label(
                                RichText::new(ask.map_or(String::new(), |l| format!("{:.1}¢", l.0 * 100.0)))
                                    .font(mono(12.0))
                                    .color(theme::down()),
                            );
                            ui.label(
                                RichText::new(ask.map_or(String::new(), |l| format!("{:.0}", l.1)))
                                    .font(mono(12.0))
                                    .color(text()),
                            );
                            ui.end_row();
                        }
                    });
                }
            }
        }
    }
}

/// Outcome probabilities over time, 0–100%.
fn prob_chart(ui: &Ui, rect: Rect, m: &Market, d: &Detail) {
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 6.0, theme::header().gamma_multiply(0.6));
    let plot = rect.shrink2(vec2(dp(36.0), dp(10.0)));
    let all: Vec<&(i64, f64)> = d.history.iter().flatten().collect();
    let (Some(t0), Some(t1)) = (all.iter().map(|x| x.0).min(), all.iter().map(|x| x.0).max()) else {
        p.text(rect.center(), Align2::CENTER_CENTER, "no price history", ui_font(11.0), muted());
        return;
    };
    let span = (t1 - t0).max(1) as f32;
    let y = |v: f64| plot.bottom() - v as f32 * plot.height();
    for lvl in [0.0, 0.25, 0.5, 0.75, 1.0] {
        p.hline(plot.x_range(), y(lvl), Stroke::new(1.0, theme::grid().gamma_multiply(0.6)));
        p.text(
            pos2(plot.left() - 4.0, y(lvl)),
            Align2::RIGHT_CENTER,
            format!("{:.0}%", lvl * 100.0),
            mono(10.0),
            muted(),
        );
    }
    for (k, h) in d.history.iter().enumerate() {
        if h.len() < 2 {
            continue;
        }
        let pts: Vec<Pos2> =
            h.iter().map(|&(t, v)| pos2(plot.left() + (t - t0) as f32 / span * plot.width(), y(v))).collect();
        p.add(Shape::line(pts, Stroke::new(1.6, theme::series(k))));
        if let Some(o) = m.outcomes.get(k) {
            p.text(
                pos2(plot.left() + 4.0 + k as f32 * dp(80.0), rect.top() + dp(8.0)),
                Align2::LEFT_CENTER,
                format!("■ {o}"),
                ui_font(10.5),
                theme::series(k),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn price_formatting() {
        assert_eq!(fmt_price(67_005.12), "67005.1");
        assert_eq!(fmt_price(2_565.4), "2565.40");
        assert_eq!(fmt_price(1.2345), "1.2345");
        assert_eq!(fmt_price(0.0000123), "0.00001230");
        assert_eq!(fmt_funding(0.0001, false), "+0.0100%");
        assert_eq!(fmt_funding(0.0002, true), "+21.9%");
        assert_eq!(heat_color(0.0), Color32::from_rgb(8, 10, 30));
        assert_eq!(heat_color(1.0), Color32::WHITE);
    }

    #[test]
    fn grid_cells_fill_rows() {
        let cells = grid(Rect::from_min_size(pos2(0.0, 0.0), vec2(620.0, 400.0)), 5, 300.0, 100.0);
        assert_eq!(cells.len(), 5);
        assert!(cells[2].top() > cells[1].top(), "two per row");
    }
}
