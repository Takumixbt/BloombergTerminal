//! Market-wide panes (data from `hd-flow`):
//!
//! - `WIRE`: the news terminal: a real-time tape of tweets, exchange listings and outlets with a
//!   breaking-news banner, story tags, toasts for important headlines and, per coin, the move
//!   since the headline arrived.
//! - `CALLS`: KOL calls from X: coin, direction and levels from each post, move since the call
//!   and a per-account scorecard.
//! - `HLECO`: Hyperliquid at a glance: perps, HIP-3 DEXs, spot tokens, HLP, fees and revenue.
//! - `MKT`: returns over 1h–30d, taker-flow CVD, OI and funding per coin, as a table or a
//!   returns heatmap, with market breadth.
//! - `SEASON`: average return by weekday × hour and by session.
//! - `CALRT`: crypto alerts on price, change, funding, OI and liquidations.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::{Align2, Color32, Rect, RichText, Sense, Stroke, Ui, pos2, vec2};
use hd_flow::alerts::Rule;
use hd_flow::fmt_usd;
use hd_flow::hl_eco::Eco;
use hd_flow::kol::{Call, KolSource, Side};
use hd_flow::market::{Dashboard, Seasonality, WINDOWS};
use hd_flow::news::{Headline, Kind, Tag};

use super::kit::*;
use super::{PaneKind, TerminalApp};
use crate::congress_view::Action;
use crate::theme::{self, amber, dp, mono, muted, text, ui_font};
use crate::widgets;
use crate::worker::flow::SharedWire;
use crate::worker::{FlowRequest, FlowResponse, Request, data_dir};

// ---- Pane state ----------------------------------------------------------------------

pub struct WirePane {
    pub query: String,
    pub source: Option<String>,
    pub tag: Option<Tag>,
    pub kind: Option<Kind>,
    pub important_only: bool,
    pub coin: Option<String>,
}

pub struct KolPane {
    pub sources: Vec<KolSource>,
    pub input: String,
    pub rss_input: String,
    pub calls: Vec<Call>,
    pub errors: Vec<String>,
    pub token: bool,
    pub fetched: Option<Instant>,
    pub loaded: bool,
}

impl KolPane {
    pub fn new(sources: Vec<KolSource>) -> Self {
        Self {
            sources,
            input: String::new(),
            rss_input: String::new(),
            calls: Vec::new(),
            errors: Vec::new(),
            token: false,
            fetched: None,
            loaded: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum MarketView {
    #[default]
    Table,
    Heatmap,
}

pub struct MarketPane {
    pub view: MarketView,
    /// Index into [`WINDOWS`] for the heatmap and sorting.
    pub window: usize,
    pub sort_desc: bool,
}

pub struct SeasonPane {
    pub coin: String,
    pub data: Option<Result<Box<Seasonality>, String>>,
    pub fetched: Option<Instant>,
}

pub struct AlertsPane {
    pub input: String,
    pub error: Option<String>,
}

/// App-wide market data shared by panes, plus the crypto alert rules.
#[derive(Default)]
pub struct MarketData {
    pub wire: Option<SharedWire>,
    wire_requested: Option<Instant>,
    /// When the wire's first batch arrived; headlines seen after that are "live".
    wire_since: Option<Instant>,
    /// Per headline link: when Hyperdesk first saw it and the coin prices at that moment.
    pub news_marks: HashMap<String, NewsMark>,
    pub eco: Option<Result<Arc<Eco>, String>>,
    eco_at: Option<Instant>,
    pub market: Option<Result<Arc<Dashboard>, String>>,
    market_at: Option<Instant>,
    pub rules: Vec<Rule>,
    pub fired: Vec<(i64, String)>,
    rules_loaded: bool,
    /// Newest liquidation already checked against the rules (Unix ms).
    liq_checked: i64,
    board_for_alerts: Option<Instant>,
}

/// What Hyperdesk knew when a headline first reached the wire.
pub struct NewsMark {
    pub seen: Instant,
    /// Arrived while the wire was open (not part of the opening batch).
    pub live: bool,
    /// Coin prices when the headline was seen, for "move since the headline". Only recorded for
    /// headlines under 15 minutes old, so the baseline is close to the news.
    pub px: Vec<(String, f64)>,
}

fn alerts_path() -> std::path::PathBuf {
    data_dir().join("crypto_alerts.json")
}

// ---- Polling, responses, alerts ------------------------------------------------------

impl TerminalApp {
    pub(super) fn market_periodic(&mut self, visible: &[usize]) {
        let shown = |f: &dyn Fn(&PaneKind) -> bool| visible.iter().any(|&i| f(&self.panes[i].kind));
        let any = |f: &dyn Fn(&PaneKind) -> bool| self.panes.iter().any(|p| f(&p.kind));
        let m = &mut self.market;
        let send = |r| self.worker.send(Request::Flow(r));
        if !any(&|k| matches!(k, PaneKind::Wire(_))) {
            m.wire = None;
            m.wire_since = None;
        } else if m.wire.is_none() && m.wire_requested.is_none_or(|t| t.elapsed() > Duration::from_secs(10)) {
            m.wire_requested = Some(Instant::now());
            send(FlowRequest::Wire);
        }
        if shown(&|k| matches!(k, PaneKind::Eco)) && m.eco_at.is_none_or(|t| t.elapsed() > Duration::from_secs(120)) {
            m.eco_at = Some(Instant::now());
            send(FlowRequest::Eco);
        }
        if shown(&|k| matches!(k, PaneKind::Market(_)))
            && m.market_at.is_none_or(|t| t.elapsed() > Duration::from_secs(300))
        {
            m.market_at = Some(Instant::now());
            send(FlowRequest::Market);
        }
        if !m.rules_loaded {
            m.rules_loaded = true;
            m.rules = std::fs::read_to_string(alerts_path())
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default();
        }
        // Alerts and KOL calls need fresh prices even when no derivatives pane is open.
        let needs_board = !m.rules.is_empty() || shown(&|k| matches!(k, PaneKind::Kol(_) | PaneKind::Wire(_)));
        if needs_board && m.board_for_alerts.is_none_or(|t| t.elapsed() > Duration::from_secs(30)) {
            m.board_for_alerts = Some(Instant::now());
            send(FlowRequest::Board);
        }
        for &i in visible {
            let id = self.panes[i].id;
            match &mut self.panes[i].kind {
                PaneKind::Kol(k) if k.fetched.is_none_or(|t| t.elapsed() > Duration::from_secs(120)) => {
                    k.fetched = Some(Instant::now());
                    self.worker.send(Request::Flow(FlowRequest::Kol { sources: k.sources.clone() }));
                }
                PaneKind::Season(s) if s.fetched.is_none() => {
                    s.fetched = Some(Instant::now());
                    self.worker.send(Request::Flow(FlowRequest::Season { pane: id, coin: s.coin.clone() }));
                }
                _ => {}
            }
        }
        self.check_alerts();
        self.mark_news();
    }

    /// Record first sight (and coin prices) for new headlines; toast the important live ones.
    fn mark_news(&mut self) {
        let Some(wire) = self.market.wire.clone() else { return };
        let since = *self.market.wire_since.get_or_insert_with(Instant::now);
        let Ok(w) = wire.lock() else { return };
        let board = self.flow.board.clone();
        let now = chrono::Utc::now().timestamp();
        let mut toasts = Vec::new();
        for h in w.items.iter().take(300) {
            if self.market.news_marks.contains_key(&h.link) {
                continue;
            }
            let live = since.elapsed() > Duration::from_secs(4);
            let px = match board.as_deref() {
                Some(b) if now - h.ts < 900 => h
                    .coins
                    .iter()
                    .filter_map(|c| b.row(c).filter(|r| r.price > 0.0).map(|r| (c.clone(), r.price)))
                    .collect(),
                _ => Vec::new(),
            };
            if live && h.important() && toasts.len() < 3 {
                toasts.push(super::chrome::ToastKind::News.msg(format!("{} · {}", who(h), h.title)));
            }
            self.market.news_marks.insert(h.link.clone(), NewsMark { seen: Instant::now(), live, px });
        }
        drop(w);
        if self.market.news_marks.len() > 5_000 {
            self.market.news_marks.retain(|_, m| m.seen.elapsed() < Duration::from_secs(6 * 3600));
        }
        for t in toasts {
            self.toasts.push((t, Instant::now()));
        }
    }

    /// Fire alert rules against the latest board and new liquidations.
    fn check_alerts(&mut self) {
        if self.market.rules.is_empty() {
            return;
        }
        let liqs: Vec<hd_flow::liq::Liquidation> = self
            .flow
            .tape
            .as_ref()
            .and_then(|t| {
                t.lock().ok().map(|t| t.items.iter().take_while(|l| l.ts > self.market.liq_checked).cloned().collect())
            })
            .unwrap_or_default();
        if let Some(newest) = liqs.first() {
            self.market.liq_checked = newest.ts;
        }
        let now = chrono::Utc::now().timestamp();
        let board = self.flow.board.clone();
        let fired = hd_flow::alerts::evaluate(&mut self.market.rules, board.as_deref(), &liqs, now);
        if !fired.is_empty() {
            for f in fired {
                self.toasts.push((super::chrome::ToastKind::Alert.msg(&f), Instant::now()));
                self.market.fired.insert(0, (now, f));
            }
            self.market.fired.truncate(200);
            self.save_rules();
        }
    }

    fn save_rules(&self) {
        if let Ok(json) = serde_json::to_vec_pretty(&self.market.rules)
            && let Err(e) = crate::layouts::write_atomic(&alerts_path(), &json)
        {
            tracing::warn!("saving crypto alerts: {e}");
        }
    }

    /// Returns `Some(r)` when the response is not a market-pane response.
    pub(super) fn market_response(&mut self, r: FlowResponse) -> Option<FlowResponse> {
        match r {
            FlowResponse::Wire(w) => {
                if self.panes.iter().any(|p| matches!(p.kind, PaneKind::Wire(_))) {
                    self.market.wire = Some(w);
                }
            }
            FlowResponse::Kol { calls, errors, token } => {
                for p in &mut self.panes {
                    if let PaneKind::Kol(k) = &mut p.kind {
                        k.calls = calls.clone();
                        k.errors = errors.clone();
                        k.token = token;
                        k.loaded = true;
                    }
                }
            }
            FlowResponse::Eco(e) => {
                if e.is_ok() || !matches!(self.market.eco, Some(Ok(_))) {
                    self.market.eco = Some(e);
                }
            }
            FlowResponse::Market(m) => {
                if m.is_ok() || !matches!(self.market.market, Some(Ok(_))) {
                    self.market.market = Some(m);
                }
            }
            FlowResponse::Season { pane, coin, season } => {
                if let Some(PaneKind::Season(s)) = self.panes.iter_mut().find(|p| p.id == pane).map(|p| &mut p.kind)
                    && s.coin == coin
                {
                    s.data = Some(season);
                }
            }
            other => return Some(other),
        }
        None
    }

    pub(super) fn market_content(&mut self, ui: &mut Ui, i: usize) -> Vec<Action> {
        let offline = self.provider.eq_ignore_ascii_case("sample") || hd_flow::offline();
        let board = self.flow.board.clone();
        match &mut self.panes[i].kind {
            PaneKind::Wire(w) => wire(ui, w, self.market.wire.as_ref(), &self.market.news_marks, board.as_deref()),
            PaneKind::Kol(k) => kol(ui, k, board.as_deref(), offline),
            PaneKind::Eco => eco(ui, self.market.eco.as_ref()),
            PaneKind::Market(m) => market(ui, m, self.market.market.as_ref()),
            PaneKind::Season(s) => season(ui, s),
            PaneKind::CryptoAlerts(a) => {
                let (acts, changed) = alerts(ui, a, &mut self.market.rules, &self.market.fired, board.as_deref());
                if changed {
                    self.save_rules();
                }
                acts
            }
            _ => Vec::new(),
        }
    }
}

// ---- Helpers -------------------------------------------------------------------------

fn tag_color(t: Tag) -> Color32 {
    match t {
        Tag::Breaking => Color32::from_rgb(255, 92, 92),
        Tag::Hack => theme::down(),
        Tag::Regulation => Color32::from_rgb(186, 160, 255),
        Tag::Etf => theme::up(),
        Tag::Listing => theme::cyan(),
        Tag::Macro => amber(),
        Tag::Funding => Color32::from_rgb(255, 140, 60),
        Tag::Partnership => muted(),
        Tag::Hyperliquid => theme::accent(),
    }
}

fn chip(ui: &mut Ui, label: &str, color: Color32) -> egui::Response {
    let font = mono(10.5);
    let g = ui.painter().layout_no_wrap(label.to_string(), font.clone(), color);
    let (r, resp) = ui.allocate_exact_size(g.size() + vec2(10.0, 4.0), Sense::click());
    ui.painter().rect_filled(r, 4.0, color.gamma_multiply(if resp.hovered() { 0.3 } else { 0.16 }));
    ui.painter().galley(r.min + vec2(5.0, 2.0), g, color);
    resp
}

/// Diverging red/green wash for a return, saturating at `scale`.
fn ret_color(r: f64, scale: f64) -> Color32 {
    let t = (r.abs() / scale).min(1.0) as f32;
    let base = if r >= 0.0 { theme::up() } else { theme::down() };
    widgets::lerp_color(theme::header(), base, 0.3 + 0.7 * t)
}

fn ago_secs(ts: i64) -> String {
    let s = (chrono::Utc::now().timestamp() - ts).max(0);
    if s < 3600 {
        format!("{}m", s / 60)
    } else if s < 86_400 {
        format!("{}h", s / 3600)
    } else {
        format!("{}d", s / 86_400)
    }
}

// ---- WIRE ----------------------------------------------------------------------------

fn kind_color(k: Kind) -> Color32 {
    match k {
        Kind::Social => Color32::from_rgb(90, 170, 255),
        Kind::Exchange => amber(),
        Kind::News => muted(),
        Kind::Official => Color32::from_rgb(186, 160, 255),
    }
}

/// `@handle` from an author like `Name (@handle)`, else the author or the source.
fn who(h: &Headline) -> String {
    match &h.author {
        Some(a) => a.find("(@").and_then(|i| a[i + 1..].strip_suffix(')')).unwrap_or(a).to_string(),
        None => h.source.clone(),
    }
}

/// Coin chip text and colour: the move since the headline when Hyperdesk saw it fresh,
/// otherwise just the coin.
fn coin_move(
    coin: &str,
    mark: Option<&NewsMark>,
    board: Option<&hd_flow::perps::Board>,
) -> (String, Color32, Option<f64>) {
    let now = board.and_then(|b| b.row(coin)).map(|r| r.price).filter(|p| *p > 0.0);
    let then = mark.and_then(|m| m.px.iter().find(|(c, _)| c == coin)).map(|(_, p)| *p);
    match (then, now) {
        (Some(a), Some(b)) => {
            let d = b / a - 1.0;
            (format!("{coin} {}", pct(d, 2)), signed(d), Some(d))
        }
        _ => (coin.to_string(), amber(), None),
    }
}

fn wire(
    ui: &mut Ui,
    w: &mut WirePane,
    data: Option<&SharedWire>,
    marks: &HashMap<String, NewsMark>,
    board: Option<&hd_flow::perps::Board>,
) -> Vec<Action> {
    let mut actions = Vec::new();
    let Some(data) = data else {
        title(ui, "NEWS");
        note(ui, "Connecting to the feeds…");
        return actions;
    };
    let Ok(wire) = data.lock() else { return actions };
    let mut sources: Vec<String> = wire.items.iter().map(|h| h.source.clone()).collect();
    sources.sort();
    sources.dedup();
    ui.horizontal(|ui| {
        title(ui, "NEWS");
        let pulse = 0.55 + 0.45 * ((ui.input(|i| i.time) * 2.5).sin() as f32).abs();
        let (r, _) = ui.allocate_exact_size(vec2(8.0, 8.0), Sense::hover());
        ui.painter().circle_filled(r.center(), 3.5, theme::up().gamma_multiply(pulse));
        ui.label(RichText::new("LIVE").font(mono(10.5)).color(theme::up()));
        let kinds = ["All", "X", "EXCH", "NEWS", "GOV"];
        let cur = match w.kind {
            None => 0,
            Some(k) => 1 + Kind::ALL.iter().position(|x| *x == k).unwrap_or(0),
        };
        if let Some(i) = widgets::segmented(ui, "wire-kind", &kinds, Some(cur), 20.0) {
            w.kind = if i == 0 { None } else { Some(Kind::ALL[i - 1]) };
        }
        ui.checkbox(&mut w.important_only, RichText::new("Important").font(ui_font(11.5)));
        ui.add(
            egui::TextEdit::singleline(&mut w.query)
                .hint_text("search")
                .font(mono(12.0))
                .desired_width((ui.available_width() - dp(90.0)).clamp(dp(60.0), dp(160.0))),
        );
    });
    // Story tags and sources on one scrollable row.
    egui::ScrollArea::horizontal()
        .id_salt("wire-tags")
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                if wire.simulated {
                    simulated(ui, "headlines");
                }
                let ok = wire.status.values().filter(|r| r.is_ok()).count();
                let bad: Vec<String> =
                    wire.status.iter().filter_map(|(k, r)| r.as_ref().err().map(|e| format!("{k}: {e}"))).collect();
                if !wire.status.is_empty() {
                    let l = ui.label(
                        RichText::new(format!("{ok}/{}", wire.status.len()))
                            .font(mono(11.0))
                            .color(if bad.is_empty() { muted() } else { amber() }),
                    );
                    l.on_hover_text(if bad.is_empty() {
                        "All sources answering".to_string()
                    } else {
                        format!("Sources not answering:\n{}", bad.join("\n"))
                    });
                }
                if let Some(c) = w.coin.clone()
                    && chip(ui, &format!("{c} ✕"), amber()).clicked()
                {
                    w.coin = None;
                }
                for t in [
                    Tag::Breaking,
                    Tag::Listing,
                    Tag::Hack,
                    Tag::Regulation,
                    Tag::Etf,
                    Tag::Macro,
                    Tag::Hyperliquid,
                    Tag::Funding,
                ] {
                    let on = w.tag == Some(t);
                    if ui.selectable_label(on, RichText::new(t.label()).font(mono(10.5)).color(tag_color(t))).clicked()
                    {
                        w.tag = if on { None } else { Some(t) };
                    }
                }
                ui.separator();
                for s in &sources {
                    let on = w.source.as_deref() == Some(s.as_str());
                    if ui.selectable_label(on, RichText::new(s).font(ui_font(10.5))).clicked() {
                        w.source = if on { None } else { Some(s.clone()) };
                    }
                }
            });
        });
    let q = w.query.trim().to_lowercase();
    let items: Vec<Headline> = wire
        .items
        .iter()
        .filter(|h| w.kind.is_none_or(|k| h.kind == k))
        .filter(|h| w.source.as_deref().is_none_or(|s| h.source == s))
        .filter(|h| w.tag.is_none_or(|t| h.tags.contains(&t)))
        .filter(|h| !w.important_only || h.important())
        .filter(|h| w.coin.as_deref().is_none_or(|c| h.coins.iter().any(|x| x == c)))
        .filter(|h| {
            q.is_empty()
                || h.title.to_lowercase().contains(&q)
                || h.author.as_ref().is_some_and(|a| a.to_lowercase().contains(&q))
        })
        .take(500)
        .cloned()
        .collect();
    drop(wire);
    if items.is_empty() {
        note(ui, "No headlines match.");
        return actions;
    }
    let now = chrono::Utc::now().timestamp();

    // Banner: the newest important story of the last 30 minutes, else the newest headline.
    let lead = items.iter().find(|h| h.important() && now - h.ts < 1_800).unwrap_or(&items[0]);
    {
        let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), dp(66.0)), Sense::click());
        let p = ui.painter_at(r);
        let mark = marks.get(&lead.link);
        let fresh = mark.filter(|m| m.live).map_or(1.0, |m| (m.seen.elapsed().as_secs_f32() / 10.0).min(1.0));
        let accent = if lead.tags.contains(&Tag::Breaking) { tag_color(Tag::Breaking) } else { kind_color(lead.kind) };
        p.rect_filled(r, 6.0, widgets::lerp_color(accent.gamma_multiply(0.35), theme::header(), fresh));
        p.rect_filled(Rect::from_min_size(r.min, vec2(4.0, r.height())), 2.0, accent);
        let meta = format!("{} · {} ago · {}", hms(lead.ts * 1000), ago_secs(lead.ts), who(lead));
        p.text(pos2(r.left() + 12.0, r.top() + 10.0), Align2::LEFT_CENTER, meta, mono(10.5), muted());
        let mut x = r.right() - 8.0;
        for c in lead.coins.iter().take(4).rev() {
            let (label, col, _) = coin_move(c, mark, board);
            let g = p.layout_no_wrap(label, mono(11.5), col);
            let cr = Rect::from_min_size(pos2(x - g.size().x - 10.0, r.top() + 3.0), g.size() + vec2(10.0, 4.0));
            p.rect_filled(cr, 4.0, col.gamma_multiply(0.16));
            p.galley(cr.min + vec2(5.0, 2.0), g, col);
            x = cr.left() - 4.0;
        }
        let g = one_line_rows(&p, &lead.title, ui_font(15.0), text(), r.width() - 24.0, 2);
        p.galley(pos2(r.left() + 12.0, r.top() + 20.0), g, text());
        if resp.clicked() {
            open_link(&lead.link);
        }
        resp.on_hover_text(format!("{}\n{}", lead.title, lead.link));
    }
    ui.add_space(dp(4.0));

    // The tape: one dense row per headline, newest first.
    let wide = ui.available_width() > dp(560.0);
    egui::ScrollArea::vertical().id_salt("wire").auto_shrink([false, false]).show(ui, |ui| {
        for h in &items {
            let mark = marks.get(&h.link);
            let width = ui.available_width();
            let left_w = if wide { dp(62.0 + 44.0 + 104.0) } else { dp(62.0 + 44.0) };
            let chips: Vec<(String, Color32, Option<f64>)> =
                h.coins.iter().take(3).map(|c| coin_move(c, mark, board)).collect();
            let chips_w: f32 = chips.iter().map(|(l, _, _)| l.len() as f32 * dp(7.2) + dp(14.0)).sum();
            let title_w = (width - left_w - chips_w - dp(8.0)).max(dp(120.0));
            let mut job = egui::text::LayoutJob::default();
            for t in &h.tags {
                job.append(
                    t.label(),
                    0.0,
                    egui::TextFormat { font_id: mono(10.5), color: tag_color(*t), ..Default::default() },
                );
                job.append(" ", 0.0, egui::TextFormat { font_id: mono(10.5), ..Default::default() });
            }
            let title_col = if h.important() { text() } else { text().gamma_multiply(0.85) };
            job.append(&h.title, 0.0, egui::TextFormat { font_id: ui_font(12.5), color: title_col, ..Default::default() });
            job.wrap = egui::text::TextWrapping { max_width: title_w, max_rows: 2, break_anywhere: false, overflow_character: Some('…') };
            let g = ui.painter().layout_job(job);
            let row_h = g.size().y.max(dp(16.0)) + dp(7.0);
            let (r, resp) = ui.allocate_exact_size(vec2(width, row_h), Sense::click());
            if !ui.is_rect_visible(r) {
                continue;
            }
            let p = ui.painter_at(r);
            let flash = mark.filter(|m| m.live).map(|m| 1.0 - (m.seen.elapsed().as_secs_f32() / 12.0).min(1.0));
            if let Some(f) = flash.filter(|f| *f > 0.0) {
                p.rect_filled(r, 2.0, kind_color(h.kind).gamma_multiply(0.22 * f));
                ui.ctx().request_repaint();
            }
            widgets::hover_wash(ui, r, &resp, false);
            if h.important() {
                let c = if h.tags.contains(&Tag::Breaking) { tag_color(Tag::Breaking) } else { amber() };
                p.rect_filled(Rect::from_min_size(r.min, vec2(3.0, r.height())), 1.5, c);
            }
            let y = r.top() + dp(11.0);
            let mut x = r.left() + dp(8.0);
            p.text(pos2(x, y), Align2::LEFT_CENTER, hms(h.ts * 1000), mono(11.0), muted());
            x += dp(62.0);
            let kc = kind_color(h.kind);
            let kr = Rect::from_min_size(pos2(x, y - dp(7.0)), vec2(dp(38.0), dp(14.0)));
            p.rect_filled(kr, 3.0, kc.gamma_multiply(0.18));
            p.text(kr.center(), Align2::CENTER_CENTER, h.kind.label(), mono(9.5), kc);
            x += dp(44.0);
            if wide {
                let g = one_line(&p, &who(h), ui_font(11.0), muted(), dp(98.0));
                p.galley(pos2(x, y - g.size().y / 2.0), g, muted());
                x += dp(104.0);
            }
            p.galley(pos2(x, r.top() + dp(3.0)), g, text());
            let mut cx = r.right() - dp(4.0);
            for (k, (label, col, _)) in chips.iter().enumerate().rev() {
                let g = p.layout_no_wrap(label.clone(), mono(10.5), *col);
                let cr = Rect::from_min_size(pos2(cx - g.size().x - dp(10.0), y - dp(8.0)), g.size() + vec2(dp(10.0), dp(4.0)));
                let cresp = ui.interact(cr, ui.id().with(("wire-coin", &h.link, k)), Sense::click());
                p.rect_filled(cr, 4.0, col.gamma_multiply(if cresp.hovered() { 0.32 } else { 0.16 }));
                p.galley(cr.min + vec2(dp(5.0), dp(2.0)), g, *col);
                let coin = h.coins[k].clone();
                if cresp.clicked() {
                    w.coin = Some(coin.clone());
                }
                cresp
                    .on_hover_text("Click: filter the wire to this coin · right-click: open a chart. The % is the move since Hyperdesk saw the headline (prices refresh about every 30 s).")
                    .context_menu(|ui| {
                        for (label, cmd) in [
                            ("Derivatives", format!("DERIV {coin}")),
                            ("Chart", format!("{coin}USD CRNCY")),
                            ("Heatmap", format!("HEAT {coin}")),
                        ] {
                            if ui.button(label).clicked() {
                                actions.push(Action::Command(cmd));
                                ui.close();
                            }
                        }
                    });
                cx = cr.left() - dp(4.0);
            }
            if resp.clicked() {
                open_link(&h.link);
            }
            let byline = match &h.author {
                Some(a) => format!("{a} · {}", h.source),
                None => h.source.clone(),
            };
            resp.on_hover_text(format!("{}\n\n{byline} · {} ago\n{}", h.title, ago_secs(h.ts), h.link));
        }
    });
    ui.ctx().request_repaint_after(Duration::from_secs(1));
    actions
}

fn open_link(link: &str) {
    if link.starts_with("http")
        && !link.contains("example.invalid")
        && let Err(e) = crate::platform::open_external(link)
    {
        tracing::warn!("opening {link}: {e}");
    }
}

// ---- CALLS ---------------------------------------------------------------------------

fn kol(ui: &mut Ui, k: &mut KolPane, board: Option<&hd_flow::perps::Board>, offline: bool) -> Vec<Action> {
    let actions = Vec::new();
    ui.horizontal_wrapped(|ui| {
        title(ui, "KOL CALLS");
        let r =
            ui.add(egui::TextEdit::singleline(&mut k.input).hint_text("@handle").font(mono(12.0)).desired_width(110.0));
        ui.add(
            egui::TextEdit::singleline(&mut k.rss_input)
                .hint_text("RSS url (optional)")
                .font(mono(11.0))
                .desired_width(150.0),
        );
        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if (enter || ui.button(RichText::new("Add").font(ui_font(11.5))).clicked()) && !k.input.trim().is_empty() {
            let handle = k.input.trim().trim_start_matches('@').to_string();
            k.sources.retain(|s| !s.handle.eq_ignore_ascii_case(&handle));
            k.sources.push(KolSource { handle, rss: k.rss_input.trim().to_string() });
            k.input.clear();
            k.rss_input.clear();
            k.fetched = None;
        }
        if offline {
            simulated(ui, "posts");
        }
    });
    let mut remove = None;
    ui.horizontal_wrapped(|ui| {
        for (i, s) in k.sources.iter().enumerate() {
            let label =
                if s.rss.is_empty() { format!("@{} ✕", s.handle) } else { format!("@{} (rss) ✕", s.handle) };
            if chip(ui, &label, theme::accent()).on_hover_text("Remove").clicked() {
                remove = Some(i);
            }
        }
    });
    if let Some(i) = remove {
        k.sources.remove(i);
        k.fetched = None;
    }
    if !offline && !k.token && k.sources.iter().all(|s| s.rss.is_empty()) {
        ui.label(
            RichText::new(
                "X has no free read API. Set X_BEARER_TOKEN (a paid X API tier) before starting Hyperdesk, \
                 or give each account an RSS bridge URL (RSSHub, Nitter, rss.app).",
            )
            .font(ui_font(11.5))
            .color(amber()),
        );
    }
    for e in k.errors.iter().take(3) {
        ui.label(RichText::new(e).font(ui_font(11.0)).color(theme::down()));
    }
    let price = |c: &str| board.and_then(|b| b.row(c)).map(|r| r.price);
    let scores = hd_flow::kol::scorecard(&k.calls, price);
    if !scores.is_empty() {
        ui.label(
            RichText::new("SCORECARD · move since the call, in the call's direction").font(mono(11.0)).color(muted()),
        );
        let widths = [150.0, 60.0, 70.0, 80.0];
        row(ui, &widths, &hdr(&["Account", "Calls", "Hit rate", "Avg"], 1), true);
        for s in scores.iter().take(8) {
            row(
                ui,
                &widths,
                &[
                    (format!("@{}", s.handle), text(), false),
                    (s.calls.to_string(), text(), true),
                    (format!("{:.0}%", s.hit_rate() * 100.0), signed(s.hit_rate() - 0.5), true),
                    (format!("{:+.2}%", s.avg * 100.0), signed(s.avg), true),
                ],
                false,
            );
        }
    }
    if k.calls.is_empty() {
        note(
            ui,
            if k.loaded {
                "No calls yet: posts need a coin ($BTC or a ticker) and a clear long/short."
            } else {
                "Loading…"
            },
        );
        return actions;
    }
    ui.add_space(dp(4.0));
    let widths = [44.0, 120.0, 56.0, 56.0, 90.0, 90.0, 72.0];
    row(ui, &widths, &hdr(&["Age", "Account", "Coin", "Side", "At call", "Now", "Since"], 4), true);
    egui::ScrollArea::vertical().id_salt("kol").auto_shrink([false, false]).show(ui, |ui| {
        for c in &k.calls {
            let now = price(&c.coin);
            let perf = now.and_then(|p| c.performance(p));
            let side = if c.side == Side::Long { ("LONG", theme::up()) } else { ("SHORT", theme::down()) };
            let r = row(
                ui,
                &widths,
                &[
                    (ago_secs(c.post.ts), muted(), false),
                    (format!("@{}", c.post.handle), text(), false),
                    (c.coin.clone(), amber(), false),
                    (side.0.into(), side.1, false),
                    (c.price_at_call.map_or("—".into(), fmt_price), muted(), true),
                    (now.map_or("—".into(), fmt_price), text(), true),
                    (perf.map_or("—".into(), |x| format!("{:+.2}%", x * 100.0)), perf.map_or(muted(), signed), true),
                ],
                false,
            );
            if r.clicked()
                && let Err(e) = crate::platform::open_external(&c.post.url)
            {
                tracing::warn!("opening {}: {e}", c.post.url);
            }
            let levels = if c.levels.is_empty() {
                String::new()
            } else {
                format!("\nlevels: {}", c.levels.iter().map(|l| fmt_price(*l)).collect::<Vec<_>>().join(" · "))
            };
            r.on_hover_text(format!("{}{levels}\nclick to open the post", c.post.text));
        }
    });
    actions
}

// ---- HLECO ---------------------------------------------------------------------------

fn stat(ui: &mut Ui, label: &str, value: String, sub: Option<(String, Color32)>) {
    let (r, _) = ui.allocate_exact_size(vec2(dp(150.0), dp(50.0)), Sense::hover());
    let p = ui.painter_at(r);
    p.rect_filled(r.shrink(2.0), 6.0, theme::header().gamma_multiply(0.7));
    p.text(pos2(r.left() + 10.0, r.top() + 13.0), Align2::LEFT_CENTER, label, ui_font(10.5), muted());
    p.text(pos2(r.left() + 10.0, r.top() + 31.0), Align2::LEFT_CENTER, value, mono(15.0), text());
    if let Some((s, c)) = sub {
        p.text(pos2(r.right() - 8.0, r.top() + 13.0), Align2::RIGHT_CENTER, s, mono(10.5), c);
    }
}

fn eco(ui: &mut Ui, data: Option<&Result<Arc<Eco>, String>>) -> Vec<Action> {
    let mut actions = Vec::new();
    let e = match data {
        None => {
            title(ui, "HYPERLIQUID ECOSYSTEM");
            ui.add(egui::Spinner::new());
            return actions;
        }
        Some(Err(err)) => {
            ui.label(RichText::new(format!("Unavailable: {err}")).color(theme::down()));
            return actions;
        }
        Some(Ok(e)) => e.clone(),
    };
    ui.horizontal_wrapped(|ui| {
        title(ui, "HYPERLIQUID ECOSYSTEM");
        if e.simulated {
            simulated(ui, "ecosystem figures");
        }
        for err in &e.errors {
            ui.label(RichText::new("partial").font(ui_font(11.0)).color(amber())).on_hover_text(err);
        }
    });
    egui::ScrollArea::vertical().id_salt("hleco").auto_shrink([false, false]).show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            stat(ui, "Perp open interest", fmt_usd(e.core.oi()), None);
            stat(ui, "Perp volume 24h", fmt_usd(e.core.volume()), None);
            stat(
                ui,
                "Perp markets",
                e.core.markets.len().to_string(),
                Some((format!("+{} HIP-3", e.hip3.iter().map(|d| d.markets.len()).sum::<usize>()), muted())),
            );
            if let Some(h) = e.hype() {
                let ch = h.change_24h.unwrap_or(0.0);
                stat(ui, "HYPE", fmt_price(h.price), Some((format!("{:+.2}%", ch * 100.0), signed(ch))));
            }
            if let Some(v) = &e.hlp {
                stat(ui, "HLP vault TVL", fmt_usd(v.tvl), Some((format!("APR {:+.1}%", v.apr * 100.0), signed(v.apr))));
            }
            if let Some(f) = &e.fees {
                stat(
                    ui,
                    "Fees 24h / 30d",
                    f.total_24h.map_or("—".into(), fmt_usd),
                    Some((f.total_30d.map_or("—".into(), fmt_usd), muted())),
                );
            }
            if let Some(r) = &e.revenue {
                stat(
                    ui,
                    "Revenue 24h / 30d",
                    r.total_24h.map_or("—".into(), fmt_usd),
                    Some((r.total_30d.map_or("—".into(), fmt_usd), muted())),
                );
            }
        });
        if let Some(r) = &e.revenue {
            let (cr, _) = ui.allocate_exact_size(vec2(ui.available_width(), dp(120.0)), Sense::hover());
            let daily: Vec<(i64, f64)> = r.daily.iter().rev().take(120).rev().copied().collect();
            mini_chart(ui, cr, "Protocol revenue per day (DefiLlama)", &daily, true, &|x| fmt_usd(x), theme::accent());
        }
        ui.label(RichText::new("HIP-3 BUILDER PERP DEXS").font(mono(11.0)).color(muted()));
        let widths = [70.0, 170.0, 64.0, 100.0, 100.0, 220.0];
        row(ui, &widths, &hdr(&["DEX", "Name", "Markets", "OI", "Vol 24h", "Top markets"], 2), true);
        for d in &e.hip3 {
            let mut top: Vec<&hd_flow::hl_eco::Market> = d.markets.iter().collect();
            top.sort_by(|a, b| b.oi_usd.total_cmp(&a.oi_usd));
            let names = top
                .iter()
                .take(4)
                .map(|m| m.name.split(':').next_back().unwrap_or(&m.name))
                .collect::<Vec<_>>()
                .join(" ");
            row(
                ui,
                &widths,
                &[
                    (d.name.clone(), amber(), false),
                    (d.full_name.clone(), text(), false),
                    (d.markets.len().to_string(), text(), true),
                    (fmt_usd(d.oi()), text(), true),
                    (fmt_usd(d.volume()), text(), true),
                    (names, muted(), true),
                ],
                false,
            )
            .on_hover_text(format!("deployer {}", d.deployer));
        }
        ui.add_space(dp(4.0));
        ui.label(RichText::new("SPOT TOKENS BY MARKET CAP").font(mono(11.0)).color(muted()));
        let widths = [80.0, 100.0, 70.0, 110.0, 100.0];
        row(ui, &widths, &hdr(&["Token", "Price", "24h", "Market cap", "Vol 24h"], 1), true);
        for t in e.spot.iter().take(25) {
            let ch = t.change_24h.unwrap_or(0.0);
            let r = row(
                ui,
                &widths,
                &[
                    (t.name.clone(), amber(), false),
                    (fmt_price(t.price), text(), true),
                    (format!("{:+.2}%", ch * 100.0), signed(ch), true),
                    (fmt_usd(t.market_cap), text(), true),
                    (fmt_usd(t.volume_usd), muted(), true),
                ],
                false,
            );
            if t.name == "HYPE" && r.clicked() {
                actions.push(Action::Command("DERIV HYPE".into()));
            }
        }
    });
    actions
}

// ---- MKT -----------------------------------------------------------------------------

fn market(ui: &mut Ui, m: &mut MarketPane, data: Option<&Result<Arc<Dashboard>, String>>) -> Vec<Action> {
    let mut actions = Vec::new();
    let d = match data {
        None => {
            title(ui, "MARKET");
            note(ui, "Loading 30 days of hourly bars for the top coins…");
            ui.add(egui::Spinner::new());
            return actions;
        }
        Some(Err(e)) => {
            ui.label(RichText::new(format!("Market data unavailable: {e}")).color(theme::down()));
            return actions;
        }
        Some(Ok(d)) => d.clone(),
    };
    let labels: Vec<&str> = WINDOWS.iter().map(|w| w.0).collect();
    ui.horizontal_wrapped(|ui| {
        title(ui, "MARKET");
        if let Some(i) = widgets::segmented(
            ui,
            "mkt-view",
            &["Table", "Heatmap"],
            Some(usize::from(m.view == MarketView::Heatmap)),
            20.0,
        ) {
            m.view = if i == 1 { MarketView::Heatmap } else { MarketView::Table };
        }
        if let Some(i) = widgets::segmented(ui, "mkt-win", &labels, Some(m.window), 20.0) {
            m.window = i;
        }
        let (up, down, med) = d.breadth(m.window);
        ui.label(RichText::new(format!("▲ {up}")).font(mono(12.0)).color(theme::up()));
        ui.label(RichText::new(format!("▼ {down}")).font(mono(12.0)).color(theme::down()));
        if let Some(md) = med {
            ui.label(RichText::new(format!("median {:+.2}%", md * 100.0)).font(mono(12.0)).color(signed(md)));
        }
        if d.simulated {
            simulated(ui, "market data");
        }
    });
    let (up, down, _) = d.breadth(m.window);
    let (bar, _) = ui.allocate_exact_size(vec2(ui.available_width(), dp(6.0)), Sense::hover());
    let split = bar.left() + bar.width() * up as f32 / (up + down).max(1) as f32;
    ui.painter().rect_filled(Rect::from_min_max(bar.min, pos2(split, bar.bottom())), 2.0, theme::up());
    ui.painter().rect_filled(Rect::from_min_max(pos2(split, bar.top()), bar.max), 2.0, theme::down());
    let mut rows: Vec<&hd_flow::market::Row> = d.rows.iter().collect();
    let w = m.window;
    rows.sort_by(|a, b| {
        b.returns[w].unwrap_or(f64::NEG_INFINITY).total_cmp(&a.returns[w].unwrap_or(f64::NEG_INFINITY))
    });
    if !m.sort_desc {
        rows.sort_by(|a, b| b.oi_usd.total_cmp(&a.oi_usd));
    }
    match m.view {
        MarketView::Heatmap => {
            let area = ui.available_rect_before_wrap();
            let mut items: Vec<(&hd_flow::market::Row, f64)> =
                d.rows.iter().map(|r| (r, r.oi_usd.max(r.volume_24h * 0.2).max(1.0))).collect();
            items.sort_by(|a, b| b.1.total_cmp(&a.1));
            let rects = treemap(area.shrink(2.0), &items.iter().map(|x| x.1).collect::<Vec<_>>());
            let scale = [0.01, 0.02, 0.05, 0.12, 0.25][w];
            for ((r, _), rect) in items.iter().zip(rects) {
                let rect = rect.shrink(1.5);
                let ret = r.returns[w].unwrap_or(0.0);
                let resp = ui.interact(rect, ui.id().with(("mkt-tile", &r.coin)), Sense::click());
                let p = ui.painter_at(rect);
                p.rect_filled(rect, 4.0, ret_color(ret, scale));
                if resp.hovered() {
                    p.rect_stroke(rect, 4.0, Stroke::new(1.5, text()), egui::StrokeKind::Inside);
                }
                let big = rect.height() > dp(44.0) && rect.width() > dp(60.0);
                if rect.width() > dp(34.0) && rect.height() > dp(18.0) {
                    let (f1, f2) = if big { (mono(14.0), mono(12.0)) } else { (mono(11.0), mono(10.0)) };
                    let c = rect.center();
                    p.text(
                        pos2(c.x, c.y - if big { 9.0 } else { 6.0 }),
                        Align2::CENTER_CENTER,
                        &r.coin,
                        f1,
                        Color32::WHITE,
                    );
                    p.text(
                        pos2(c.x, c.y + if big { 9.0 } else { 6.0 }),
                        Align2::CENTER_CENTER,
                        format!("{:+.2}%", ret * 100.0),
                        f2,
                        Color32::WHITE.gamma_multiply(0.9),
                    );
                }
                if resp.clicked() {
                    actions.push(Action::Command(format!("DERIV {}", r.coin)));
                }
                resp.on_hover_text(format!(
                    "{} {} · OI {} · CVD 24h {}",
                    r.coin,
                    fmt_price(r.price),
                    fmt_usd(r.oi_usd),
                    fmt_usd(r.cvd_24h)
                ));
            }
            ui.advance_cursor_after_rect(area);
        }
        MarketView::Table => {
            ui.horizontal(|ui| {
                ui.checkbox(
                    &mut m.sort_desc,
                    RichText::new(format!("Sort by {} return", WINDOWS[w].0)).font(ui_font(11.0)),
                );
                note(ui, "click a row for derivatives detail");
            });
            let widths = [62.0, 92.0, 62.0, 62.0, 62.0, 62.0, 62.0, 78.0, 90.0, 66.0, 78.0, 78.0];
            row(
                ui,
                &widths,
                &hdr(&["Coin", "Price", "1h", "4h", "24h", "7d", "30d", "Vol 24h", "CVD 24h", "", "OI", "Funding"], 1),
                true,
            );
            egui::ScrollArea::vertical().id_salt("mkt").auto_shrink([false, false]).show(ui, |ui| {
                for r in rows {
                    let mut cells = vec![(r.coin.clone(), amber(), false), (fmt_price(r.price), text(), true)];
                    for x in r.returns {
                        cells.push((
                            x.map_or("—".into(), |v| format!("{:+.2}%", v * 100.0)),
                            x.map_or(muted(), signed),
                            true,
                        ));
                    }
                    cells.push((fmt_usd(r.volume_24h), text(), true));
                    cells.push((fmt_usd(r.cvd_24h), signed(r.cvd_24h), true));
                    cells.push((String::new(), muted(), true));
                    cells.push((fmt_usd(r.oi_usd), text(), true));
                    cells.push((
                        r.funding_8h.map_or("—".into(), |f| format!("{:+.4}%", f * 100.0)),
                        r.funding_8h.map_or(muted(), signed),
                        true,
                    ));
                    let resp = row(ui, &widths, &cells, false);
                    // Return cells get a heat wash; the blank column holds the CVD sparkline.
                    let s = dp(1.0);
                    let x0 = resp.rect.left() + (widths[0] + widths[1]) * s;
                    for (k, x) in r.returns.iter().enumerate() {
                        if let Some(v) = x {
                            let cell = Rect::from_min_size(
                                pos2(x0 + k as f32 * widths[2] * s, resp.rect.top()),
                                vec2(widths[2] * s, resp.rect.height()),
                            )
                            .shrink2(vec2(2.0, 2.0));
                            ui.painter().rect_filled(
                                cell,
                                3.0,
                                ret_color(*v, [0.01, 0.02, 0.05, 0.12, 0.25][k]).gamma_multiply(0.35),
                            );
                        }
                    }
                    let sx = resp.rect.left() + widths[..9].iter().sum::<f32>() * s;
                    let spark = Rect::from_min_size(
                        pos2(sx + 4.0, resp.rect.top() + 3.0),
                        vec2(widths[9] * s - 8.0, resp.rect.height() - 6.0),
                    );
                    sparkline(ui, spark, &r.cvd_spark);
                    if resp.clicked() {
                        actions.push(Action::Command(format!("DERIV {}", r.coin)));
                    }
                }
            });
        }
    }
    actions
}

fn sparkline(ui: &Ui, r: Rect, v: &[f64]) {
    if v.len() < 2 {
        return;
    }
    let lo = v.iter().copied().fold(f64::INFINITY, f64::min);
    let hi = v.iter().copied().fold(f64::NEG_INFINITY, f64::max).max(lo + 1e-9);
    let pts: Vec<egui::Pos2> = v
        .iter()
        .enumerate()
        .map(|(i, x)| {
            pos2(
                r.left() + r.width() * i as f32 / (v.len() - 1) as f32,
                r.bottom() - ((x - lo) / (hi - lo)) as f32 * r.height(),
            )
        })
        .collect();
    let c = if v[v.len() - 1] >= v[0] { theme::up() } else { theme::down() };
    ui.painter().add(egui::Shape::line(pts, Stroke::new(1.2, c)));
}

/// Slice-and-dice treemap: rectangles proportional to `weights` (sorted descending), split
/// along the longer side so tiles stay roughly square.
pub fn treemap(area: Rect, weights: &[f64]) -> Vec<Rect> {
    fn go(area: Rect, w: &[f64], out: &mut Vec<Rect>) {
        match w.len() {
            0 => {}
            1 => out.push(area),
            _ => {
                let total: f64 = w.iter().sum();
                // Split where the first group holds about half the weight.
                let mut acc = 0.0;
                let mut k = 1;
                for (i, x) in w.iter().enumerate() {
                    acc += x;
                    if acc >= total / 2.0 {
                        k = (i + 1).min(w.len() - 1).max(1);
                        break;
                    }
                }
                let first: f64 = w[..k].iter().sum();
                let f = (first / total.max(1e-12)) as f32;
                let (a, b) = if area.width() >= area.height() {
                    let x = area.left() + area.width() * f;
                    (
                        Rect::from_min_max(area.min, pos2(x, area.bottom())),
                        Rect::from_min_max(pos2(x, area.top()), area.max),
                    )
                } else {
                    let y = area.top() + area.height() * f;
                    (
                        Rect::from_min_max(area.min, pos2(area.right(), y)),
                        Rect::from_min_max(pos2(area.left(), y), area.max),
                    )
                };
                go(a, &w[..k], out);
                go(b, &w[k..], out);
            }
        }
    }
    let mut out = Vec::with_capacity(weights.len());
    go(area, weights, &mut out);
    out
}

// ---- SEASON --------------------------------------------------------------------------

fn season(ui: &mut Ui, s: &mut SeasonPane) -> Vec<Action> {
    let actions = Vec::new();
    ui.horizontal_wrapped(|ui| {
        title(ui, &format!("{} SEASONALITY", s.coin));
        for c in ["BTC", "ETH", "SOL", "HYPE", "XRP", "DOGE"] {
            if ui.selectable_label(s.coin == c, RichText::new(c).font(mono(11.5))).clicked() && s.coin != c {
                s.coin = c.into();
                s.data = None;
                s.fetched = None;
            }
        }
    });
    let d = match &s.data {
        None => {
            note(ui, "Loading about 125 days of hourly bars (Binance futures)…");
            ui.add(egui::Spinner::new());
            return actions;
        }
        Some(Err(e)) => {
            ui.label(RichText::new(e).color(theme::down()));
            return actions;
        }
        Some(Ok(d)) => d,
    };
    ui.horizontal_wrapped(|ui| {
        note(ui, format!("Average hourly return by weekday × hour (UTC) over {} hours.", d.hours));
        if d.simulated {
            simulated(ui, "bars");
        }
    });
    let area = ui.available_rect_before_wrap();
    let grid_h = (area.height() - dp(90.0)).max(dp(140.0));
    let left = dp(40.0);
    let cw = (area.width() - left - dp(56.0)) / 24.0;
    let ch = (grid_h - dp(18.0)) / 7.0;
    let p = ui.painter_at(area);
    let scale = d.grid.iter().flatten().map(|x| x.abs()).fold(0.0, f64::max).max(1e-9);
    let days = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
    for h in 0..24 {
        if h % 3 == 0 {
            p.text(
                pos2(area.left() + left + (h as f32 + 0.5) * cw, area.top() + 8.0),
                Align2::CENTER_CENTER,
                format!("{h:02}"),
                mono(10.0),
                muted(),
            );
        }
    }
    for (dd, rowv) in d.grid.iter().enumerate() {
        let y = area.top() + dp(18.0) + dd as f32 * ch;
        p.text(pos2(area.left() + 4.0, y + ch / 2.0), Align2::LEFT_CENTER, days[dd], mono(10.5), muted());
        for (h, v) in rowv.iter().enumerate() {
            let r = Rect::from_min_size(pos2(area.left() + left + h as f32 * cw, y), vec2(cw, ch)).shrink(1.0);
            p.rect_filled(r, 2.0, ret_color(*v, scale));
            if cw > dp(30.0) && ch > dp(16.0) {
                p.text(
                    r.center(),
                    Align2::CENTER_CENTER,
                    format!("{:+.2}", v * 100.0),
                    mono(9.5),
                    Color32::WHITE.gamma_multiply(0.85),
                );
            }
        }
        let wm = d.weekday_mean.get(dd).copied().unwrap_or(0.0);
        p.text(
            pos2(area.right() - 4.0, y + ch / 2.0),
            Align2::RIGHT_CENTER,
            format!("{:+.3}%", wm * 100.0),
            mono(10.5),
            signed(wm),
        );
    }
    let y0 = area.top() + grid_h + dp(6.0);
    let mut x = area.left() + 4.0;
    for (name, mean, up) in &d.sessions {
        let r = Rect::from_min_size(pos2(x, y0), vec2(dp(150.0), dp(44.0)));
        p.rect_filled(r, 6.0, theme::header().gamma_multiply(0.7));
        p.text(pos2(r.left() + 8.0, r.top() + 12.0), Align2::LEFT_CENTER, name, ui_font(11.0), muted());
        p.text(
            pos2(r.left() + 8.0, r.top() + 30.0),
            Align2::LEFT_CENTER,
            format!("{:+.3}%/h", mean * 100.0),
            mono(13.0),
            signed(*mean),
        );
        p.text(
            pos2(r.right() - 8.0, r.top() + 30.0),
            Align2::RIGHT_CENTER,
            format!("{:.0}% up", up * 100.0),
            mono(10.5),
            muted(),
        );
        x += dp(158.0);
    }
    ui.advance_cursor_after_rect(area);
    actions
}

// ---- CALRT ---------------------------------------------------------------------------

fn alerts(
    ui: &mut Ui,
    a: &mut AlertsPane,
    rules: &mut Vec<Rule>,
    fired: &[(i64, String)],
    board: Option<&hd_flow::perps::Board>,
) -> (Vec<Action>, bool) {
    let mut changed = false;
    title(ui, "CRYPTO ALERTS");
    ui.horizontal(|ui| {
        let r = ui.add(
            egui::TextEdit::singleline(&mut a.input)
                .hint_text("BTC > 70k · SOL CHG > 8% · HYPE FUNDING > 0.05% · LIQ > 1M")
                .font(mono(12.0))
                .desired_width((ui.available_width() - dp(60.0)).clamp(dp(140.0), dp(420.0))),
        );
        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if enter || ui.button(RichText::new("Add").font(ui_font(11.5))).clicked() {
            match Rule::parse(&a.input) {
                Some(rule) => {
                    rules.push(rule);
                    a.input.clear();
                    a.error = None;
                    changed = true;
                }
                None if a.input.trim().is_empty() => {}
                None => a.error = Some(format!("Could not read {:?}", a.input.trim())),
            }
        }
    });
    if let Some(e) = &a.error {
        ui.label(RichText::new(e).font(ui_font(11.5)).color(theme::down()));
    }
    note(
        ui,
        "Rules fire once when their condition becomes true (toast + log) and re-arm when it is false again. Liquidation rules need a LIQ pane open.",
    );
    let widths = [300.0, 110.0, 80.0, 70.0];
    row(ui, &widths, &hdr(&["Rule", "Now", "State", ""], 1), true);
    let mut remove = None;
    for (i, r) in rules.iter().enumerate() {
        let now = board.and_then(|b| r.current(b));
        let state = if r.active { ("TRIGGERED", amber()) } else { ("armed", muted()) };
        let resp = row(
            ui,
            &widths,
            &[
                (r.describe(), text(), false),
                (
                    now.map_or("—".into(), |v| {
                        if matches!(r.metric, hd_flow::alerts::Metric::Price) {
                            fmt_price(v)
                        } else {
                            format!("{:+.3}%", v * 100.0)
                        }
                    }),
                    text(),
                    true,
                ),
                (state.0.into(), state.1, true),
                ("remove".into(), theme::down().gamma_multiply(0.8), true),
            ],
            false,
        );
        let s = dp(1.0);
        let x = resp.rect.left() + widths[..3].iter().sum::<f32>() * s;
        let cell = Rect::from_min_size(pos2(x, resp.rect.top()), vec2(widths[3] * s, resp.rect.height()));
        if ui
            .interact(cell, ui.id().with(("calrt-remove", i)), Sense::click())
            .on_hover_text("Remove this rule")
            .clicked()
        {
            remove = Some(i);
        }
    }
    if let Some(i) = remove {
        rules.remove(i);
        changed = true;
    }
    ui.add_space(dp(6.0));
    ui.label(RichText::new("FIRED").font(mono(11.0)).color(muted()));
    egui::ScrollArea::vertical().id_salt("calrt").auto_shrink([false, false]).show(ui, |ui| {
        for (t, m) in fired {
            ui.horizontal(|ui| {
                ui.label(RichText::new(hms(t * 1000)).font(mono(11.0)).color(muted()));
                ui.label(RichText::new(m).font(mono(12.0)).color(text()));
            });
        }
        if fired.is_empty() {
            note(ui, "Nothing has fired yet.");
        }
    });
    (Vec::new(), changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn treemap_covers_the_area() {
        let area = Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0, 200.0));
        let w = [50.0, 20.0, 10.0, 10.0, 5.0, 5.0];
        let rects = treemap(area, &w);
        assert_eq!(rects.len(), w.len());
        let total: f32 = rects.iter().map(|r| r.area()).sum();
        assert!((total - area.area()).abs() < 1.0);
        assert!((rects[0].area() / area.area() - 0.5).abs() < 0.01, "largest tile gets its share");
        assert!(rects.iter().all(|r| area.expand(0.01).contains_rect(*r)));
    }
}
