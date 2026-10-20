//! Hyperliquid trader intelligence and options flow panes (data from `hd-flow`):
//!
//! - `HLW <address>`: any wallet's account value, positions (size, entry, PnL, ROE,
//!   liquidation price and distance, leverage, funding) and recent fills; saved wallets, and
//!   a toast when a displayed wallet opens, closes or flips a position.
//! - `HLTOP`: the leaderboard (PnL, ROI and volume by day, week, month, all time) and vaults.
//! - `HLPOS BTC`: positioning of the top wallets: long/short exposure, size cohorts,
//!   liquidation levels from their real positions, and the largest positions.
//! - `PRINTS`: large Hyperliquid trades with the taker's and maker's wallets.
//! - `OFLOW`: Deribit BTC/ETH options flow with premium, IV, block trades and summaries.

use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::{Align2, Rect, RichText, Sense, Stroke, Ui, pos2, vec2};
use hd_flow::fmt_usd;
use hd_flow::hyperliquid::{self, Account, COHORTS, Fill, Leader, Perf, Positioning, Vault, short_address};
use hd_flow::labels::Labels;
use hd_flow::options_flow::OptionTrade;
use serde::{Deserialize, Serialize};

use super::kit::*;
use super::{PaneKind, TerminalApp};
use crate::congress_view::Action;
use crate::theme::{self, amber, dp, mono, muted, text, ui_font};
use crate::widgets;
use crate::worker::flow::{SharedOptionFlow, SharedPrints};
use crate::worker::{FlowRequest, FlowResponse, Request};

// ---- Pane state ----------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SavedWallet {
    pub address: String,
    #[serde(default)]
    pub label: String,
}

pub struct WalletPane {
    pub address: Option<String>,
    pub input: String,
    pub saved: Vec<SavedWallet>,
    pub account: Option<Result<Box<Account>, String>>,
    pub fills: Vec<Fill>,
    pub fetched: Option<Instant>,
    /// Label being edited for the shown wallet.
    pub label_edit: Option<String>,
}

impl WalletPane {
    pub fn new(address: Option<String>, saved: Vec<SavedWallet>) -> Self {
        Self {
            address: address.and_then(|a| hyperliquid::parse_address(&a)),
            input: String::new(),
            saved,
            account: None,
            fills: Vec::new(),
            fetched: None,
            label_edit: None,
        }
    }

    fn open(&mut self, address: &str) {
        if self.address.as_deref() != Some(address) {
            self.address = Some(address.to_string());
            self.account = None;
            self.fills.clear();
            self.fetched = None;
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Window {
    Day,
    Week,
    #[default]
    Month,
    AllTime,
}

impl Window {
    const ALL: [Window; 4] = [Window::Day, Window::Week, Window::Month, Window::AllTime];
    const LABELS: [&'static str; 4] = ["24h", "7d", "30d", "All"];

    fn of(self, l: &Leader) -> Perf {
        match self {
            Window::Day => l.day,
            Window::Week => l.week,
            Window::Month => l.month,
            Window::AllTime => l.all_time,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum LeaderSort {
    #[default]
    Pnl,
    Roi,
    Value,
    Volume,
}

pub struct LeadersPane {
    pub vaults: bool,
    pub window: Window,
    pub sort: LeaderSort,
    pub filter: String,
}

pub struct PositioningPane {
    pub coin: String,
}

pub struct PrintsPane {
    pub coin: Option<String>,
    pub min_usd: f64,
}

pub struct OptionFlowPane {
    pub currency: Option<String>,
    pub min_premium: f64,
    pub blocks_only: bool,
    pub window_min: i64,
}

/// App-wide Hyperliquid and options data shared by panes of a kind.
#[derive(Default)]
pub struct HlData {
    pub leaders: Option<Result<Arc<Vec<Leader>>, String>>,
    pub vaults: Vec<Vault>,
    leaders_at: Option<Instant>,
    pub positioning: Option<Result<Arc<Positioning>, String>>,
    positioning_at: Option<Instant>,
    pub prints: Option<SharedPrints>,
    prints_requested: Option<Instant>,
    pub oflow: Option<SharedOptionFlow>,
    oflow_requested: Option<Instant>,
}

// ---- Polling and responses -----------------------------------------------------------

impl TerminalApp {
    pub(super) fn hl_periodic(&mut self, visible: &[usize]) {
        let shown = |f: &dyn Fn(&PaneKind) -> bool| visible.iter().any(|&i| f(&self.panes[i].kind));
        let any = |f: &dyn Fn(&PaneKind) -> bool| self.panes.iter().any(|p| f(&p.kind));
        let d = &mut self.hl;
        let send = |r| self.worker.send(Request::Flow(r));
        if shown(&|k| matches!(k, PaneKind::Leaders(_)))
            && d.leaders_at.is_none_or(|t| t.elapsed() > Duration::from_secs(300))
        {
            d.leaders_at = Some(Instant::now());
            send(FlowRequest::Leaders);
        }
        if shown(&|k| matches!(k, PaneKind::HlPositioning(_)))
            && d.positioning_at.is_none_or(|t| t.elapsed() > Duration::from_secs(120))
        {
            d.positioning_at = Some(Instant::now());
            send(FlowRequest::Positioning);
        }
        // Streams run only while some pane (visible or not) uses them.
        if !any(&|k| matches!(k, PaneKind::Prints(_))) {
            d.prints = None;
        } else if d.prints.is_none() && d.prints_requested.is_none_or(|t| t.elapsed() > Duration::from_secs(10)) {
            d.prints_requested = Some(Instant::now());
            send(FlowRequest::Prints);
        }
        if !any(&|k| matches!(k, PaneKind::OptionFlow(_))) {
            d.oflow = None;
        } else if d.oflow.is_none() && d.oflow_requested.is_none_or(|t| t.elapsed() > Duration::from_secs(10)) {
            d.oflow_requested = Some(Instant::now());
            send(FlowRequest::OptionFlow);
        }
        for &i in visible {
            let id = self.panes[i].id;
            if let PaneKind::Wallet(w) = &mut self.panes[i].kind
                && let Some(a) = &w.address
                && w.fetched.is_none_or(|t| t.elapsed() > Duration::from_secs(30))
            {
                w.fetched = Some(Instant::now());
                self.worker.send(Request::Flow(FlowRequest::Wallet { pane: id, address: a.clone() }));
            }
        }
    }

    pub(super) fn hl_response(&mut self, r: FlowResponse) {
        match r {
            FlowResponse::Wallet { pane, address, account, fills } => {
                let mut toasts = Vec::new();
                if let Some(PaneKind::Wallet(w)) = self.panes.iter_mut().find(|p| p.id == pane).map(|p| &mut p.kind)
                    && w.address.as_deref() == Some(address.as_str())
                {
                    if let (Some(Ok(old)), Ok(new)) = (&w.account, &account) {
                        toasts = position_changes(old, new);
                    }
                    w.account = Some(account);
                    w.fills = fills;
                }
                for t in toasts {
                    self.toasts.push((t, Instant::now()));
                }
            }
            FlowResponse::Leaders { leaders, vaults } => {
                if let Ok(l) = &leaders {
                    self.labels.absorb_leaders(l);
                }
                self.labels.absorb_vaults(&vaults);
                if leaders.is_ok() || !matches!(self.hl.leaders, Some(Ok(_))) {
                    self.hl.leaders = Some(leaders);
                }
                if !vaults.is_empty() {
                    self.hl.vaults = vaults;
                }
            }
            FlowResponse::Positioning(p) => {
                if p.is_ok() || !matches!(self.hl.positioning, Some(Ok(_))) {
                    self.hl.positioning = Some(p);
                }
            }
            FlowResponse::Prints(t) => {
                if self.panes.iter().any(|p| matches!(p.kind, PaneKind::Prints(_))) {
                    self.hl.prints = Some(t);
                }
            }
            FlowResponse::OptionFlow(t) => {
                if self.panes.iter().any(|p| matches!(p.kind, PaneKind::OptionFlow(_))) {
                    self.hl.oflow = Some(t);
                }
            }
            other => self.flow_response(other),
        }
    }

    pub(super) fn hl_content(&mut self, ui: &mut Ui, i: usize) -> Vec<Action> {
        let hl = &self.hl;
        let offline = self.provider.eq_ignore_ascii_case("sample") || hd_flow::offline();
        let labels = &mut self.labels;
        match &mut self.panes[i].kind {
            PaneKind::Wallet(w) => {
                let (actions, changed) = wallet(ui, w, labels);
                if changed && let Err(e) = labels.save(&crate::worker::data_dir().join("wallet_labels.json")) {
                    tracing::warn!("saving wallet labels: {e:#}");
                }
                actions
            }
            PaneKind::Leaders(l) => leaders(ui, l, hl.leaders.as_ref(), &hl.vaults, offline, labels),
            PaneKind::HlPositioning(p) => positioning(ui, p, hl.positioning.as_ref(), labels),
            PaneKind::Prints(p) => prints(ui, p, hl.prints.as_ref(), labels),
            PaneKind::OptionFlow(o) => option_flow(ui, o, hl.oflow.as_ref()),
            _ => Vec::new(),
        }
    }
}

/// Toasts for positions opened, closed, flipped or resized by more than a quarter.
pub fn position_changes(old: &Account, new: &Account) -> Vec<String> {
    let who = short_address(&new.address);
    let mut out = Vec::new();
    for p in &new.positions {
        let side = if p.is_long() { "long" } else { "short" };
        match old.positions.iter().find(|o| o.coin == p.coin) {
            None => out.push(format!("{who} opened {} {side} {}", p.coin, fmt_usd(p.value))),
            Some(o) if o.is_long() != p.is_long() => {
                out.push(format!("{who} flipped {} to {side} {}", p.coin, fmt_usd(p.value)))
            }
            Some(o) if (p.size.abs() / o.size.abs().max(1e-12) - 1.0).abs() > 0.25 => {
                let verb = if p.size.abs() > o.size.abs() { "added to" } else { "reduced" };
                out.push(format!("{who} {verb} {} {side}: now {}", p.coin, fmt_usd(p.value)));
            }
            _ => {}
        }
    }
    for o in &old.positions {
        if !new.positions.iter().any(|p| p.coin == o.coin) {
            out.push(format!("{who} closed {} {}", o.coin, if o.is_long() { "long" } else { "short" }));
        }
    }
    out
}

// ---- Helpers -------------------------------------------------------------------------

// ---- HLW: wallet ---------------------------------------------------------------------

fn wallet(ui: &mut Ui, w: &mut WalletPane, labels: &mut Labels) -> (Vec<Action>, bool) {
    let mut actions = Vec::new();
    let mut changed = false;
    ui.horizontal_wrapped(|ui| {
        title(ui, "HYPERLIQUID WALLET");
        let r = ui.add(
            egui::TextEdit::singleline(&mut w.input).hint_text("0x… address").font(mono(12.0)).desired_width(200.0),
        );
        let go = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if (go || ui.button(RichText::new("Open").font(ui_font(11.5))).clicked()) && !w.input.trim().is_empty() {
            match hyperliquid::parse_address(&w.input) {
                Some(a) => {
                    w.open(&a);
                    w.input.clear();
                }
                None => w.input = format!("{} (not an address)", w.input.trim()),
            }
        }
        if let Some(a) = w.address.clone() {
            let saved = w.saved.iter().any(|s| s.address == a);
            if ui.button(RichText::new(if saved { "★ Saved" } else { "☆ Save" }).font(ui_font(11.5))).clicked() {
                if saved {
                    w.saved.retain(|s| s.address != a);
                } else {
                    w.saved.push(SavedWallet { address: a.clone(), label: String::new() });
                }
            }
            if ui.small_button("Copy").on_hover_text(&a).clicked() {
                ui.ctx().copy_text(a.clone());
            }
        }
    });
    if !w.saved.is_empty() {
        ui.horizontal_wrapped(|ui| {
            note(ui, "Saved:");
            let mut open = None;
            for s in &w.saved {
                let label = if s.label.is_empty() { labels.display(&s.address) } else { s.label.clone() };
                let on = w.address.as_deref() == Some(s.address.as_str());
                if ui.selectable_label(on, RichText::new(label).font(mono(11.5))).clicked() {
                    open = Some(s.address.clone());
                }
            }
            if let Some(a) = open {
                w.open(&a);
            }
        });
    }
    let Some(addr) = w.address.clone() else {
        note(
            ui,
            "Paste any Hyperliquid address (or pick one in HLTOP, HLPOS or PRINTS) to see its positions, liquidation prices, PnL and fills. Positions are public on-chain data.",
        );
        return (actions, changed);
    };
    let acct = match &w.account {
        None => {
            ui.add(egui::Spinner::new());
            return (actions, changed);
        }
        Some(Err(e)) => {
            ui.label(RichText::new(format!("{}: {e}", short_address(&addr))).font(ui_font(12.0)).color(theme::down()));
            return (actions, changed);
        }
        Some(Ok(a)) => a.clone(),
    };
    // Name, tags and your label.
    ui.horizontal_wrapped(|ui| {
        let name = labels.display(&addr);
        ui.label(RichText::new(&name).font(mono(14.0)).color(amber())).on_hover_text(&addr);
        if labels.label(&addr).is_some() {
            ui.label(RichText::new(short_address(&addr)).font(mono(11.0)).color(muted())).on_hover_text(&addr);
        }
        for t in labels.account_tags(&acct) {
            let c = match t.as_str() {
                "High leverage" | "Underwater" | "Near liquidation" | "Rekt 30d" => theme::down(),
                "Top 30d PnL" | "Top 10" => theme::up(),
                _ => theme::accent(),
            };
            ui.label(
                RichText::new(format!(" {t} ")).font(mono(10.5)).color(c).background_color(c.gamma_multiply(0.15)),
            );
        }
        match &mut w.label_edit {
            None => {
                if ui.small_button("✎ Label").on_hover_text("Name this wallet (saved on this computer)").clicked() {
                    w.label_edit = Some(labels.user.get(&addr).cloned().unwrap_or_default());
                }
            }
            Some(buf) => {
                let r =
                    ui.add(egui::TextEdit::singleline(buf).hint_text("label").font(mono(12.0)).desired_width(140.0));
                let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if enter || ui.small_button("Save").clicked() {
                    labels.set_user(&addr, buf);
                    changed = true;
                    w.label_edit = None;
                } else if ui.small_button("Cancel").clicked() {
                    w.label_edit = None;
                }
            }
        }
    });
    ui.horizontal_wrapped(|ui| {
        for (k, v, c) in [
            ("Value", fmt_usd(acct.account_value), text()),
            ("uPnL", signed_usd(acct.upnl()), signed(acct.upnl())),
            ("Notional", fmt_usd(acct.notional), text()),
            ("Net", signed_usd(acct.net()), signed(acct.net())),
            ("Leverage", format!("{:.2}x", acct.leverage()), text()),
            ("Margin", fmt_usd(acct.margin_used), text()),
            ("Withdrawable", fmt_usd(acct.withdrawable), muted()),
        ] {
            ui.label(RichText::new(k).font(ui_font(11.0)).color(muted()));
            ui.label(RichText::new(v).font(mono(12.5)).color(c));
        }
    });
    egui::ScrollArea::vertical().id_salt(("hlw", &addr)).auto_shrink([false, false]).show(ui, |ui| {
        ui.label(RichText::new(format!("POSITIONS ({})", acct.positions.len())).font(mono(11.0)).color(muted()));
        let widths = [70.0, 62.0, 88.0, 90.0, 90.0, 90.0, 90.0, 70.0, 90.0, 64.0, 80.0, 74.0];
        row(
            ui,
            &widths,
            &hdr(
                &[
                    "Coin",
                    "Side",
                    "Size",
                    "Value",
                    "Entry",
                    "Mark",
                    "uPnL",
                    "ROE",
                    "Liq. price",
                    "To liq",
                    "Leverage",
                    "Funding",
                ],
                2,
            ),
            true,
        );
        let mut positions = acct.positions.clone();
        positions.sort_by(|a, b| b.value.total_cmp(&a.value));
        for p in &positions {
            let side_c = if p.is_long() { theme::up() } else { theme::down() };
            let dist = p.liq_distance();
            let risk = dist.map_or(muted(), |d| {
                if d < 0.05 {
                    theme::down()
                } else if d < 0.15 {
                    amber()
                } else {
                    text()
                }
            });
            let r = row(
                ui,
                &widths,
                &[
                    (p.coin.clone(), amber(), false),
                    ((if p.is_long() { "LONG" } else { "SHORT" }).into(), side_c, false),
                    (qty(p.size.abs()), text(), true),
                    (fmt_usd(p.value), text(), true),
                    (fmt_price(p.entry), text(), true),
                    (fmt_price(p.mark()), text(), true),
                    (signed_usd(p.upnl), signed(p.upnl), true),
                    (format!("{:+.1}%", p.roe * 100.0), signed(p.roe), true),
                    (p.liq_px.map_or("—".into(), fmt_price), risk, true),
                    (dist.map_or("—".into(), |d| format!("{:.1}%", d * 100.0)), risk, true),
                    (format!("{:.0}x {}", p.leverage, if p.cross { "cross" } else { "iso" }), muted(), true),
                    (signed_usd(-p.funding), signed(-p.funding), true),
                ],
                false,
            );
            if r.clicked() {
                actions.push(Action::Command(format!("HLPOS {}", p.coin)));
            }
            r.on_hover_text("Click: how the top wallets are positioned in this coin");
        }
        ui.add_space(dp(6.0));
        ui.label(RichText::new("RECENT FILLS").font(mono(11.0)).color(muted()));
        let widths = [56.0, 64.0, 150.0, 90.0, 88.0, 90.0, 90.0, 70.0];
        row(
            ui,
            &widths,
            &hdr(&["Time", "Coin", "Direction", "Price", "Size", "Notional", "Closed PnL", "Fee"], 3),
            true,
        );
        for f in w.fills.iter().take(200) {
            let c = if f.dir.contains("Liquidat") {
                theme::down()
            } else if f.buy {
                theme::up()
            } else {
                theme::down()
            };
            row(
                ui,
                &widths,
                &[
                    (ago(f.time), muted(), false),
                    (f.coin.clone(), amber(), false),
                    (f.dir.clone(), c, false),
                    (fmt_price(f.price), text(), true),
                    (qty(f.size), text(), true),
                    (fmt_usd(f.notional()), text(), true),
                    (
                        if f.closed_pnl == 0.0 { "—".into() } else { signed_usd(f.closed_pnl) },
                        signed(f.closed_pnl),
                        true,
                    ),
                    (format!("{:.2}", f.fee), muted(), true),
                ],
                false,
            )
            .on_hover_text(hms(f.time));
        }
    });
    ui.ctx().request_repaint_after(Duration::from_secs(5));
    (actions, changed)
}

// ---- HLTOP: leaderboard and vaults ---------------------------------------------------

fn leaders(
    ui: &mut Ui,
    l: &mut LeadersPane,
    data: Option<&Result<Arc<Vec<Leader>>, String>>,
    vaults: &[Vault],
    sim: bool,
    labels: &Labels,
) -> Vec<Action> {
    let mut actions = Vec::new();
    ui.horizontal_wrapped(|ui| {
        title(ui, "HYPERLIQUID");
        if let Some(i) = widgets::segmented(ui, "hl-tab", &["Traders", "Vaults"], Some(usize::from(l.vaults)), 20.0) {
            l.vaults = i == 1;
        }
        if !l.vaults {
            if let Some(i) =
                widgets::segmented(ui, "hl-win", &Window::LABELS, Window::ALL.iter().position(|w| *w == l.window), 20.0)
            {
                l.window = Window::ALL[i];
            }
            let sorts = [LeaderSort::Pnl, LeaderSort::Roi, LeaderSort::Value, LeaderSort::Volume];
            if let Some(i) = widgets::segmented(
                ui,
                "hl-sort",
                &["PnL", "ROI", "Value", "Volume"],
                sorts.iter().position(|s| *s == l.sort),
                20.0,
            ) {
                l.sort = sorts[i];
            }
        }
        ui.add(egui::TextEdit::singleline(&mut l.filter).hint_text("filter").font(mono(12.0)).desired_width(110.0));
        if sim {
            simulated(ui, "data");
        }
    });
    if l.vaults {
        let widths = [260.0, 110.0, 80.0, 120.0];
        row(ui, &widths, &hdr(&["Vault", "TVL", "APR", "Leader"], 1), true);
        egui::ScrollArea::vertical().id_salt("hl-vaults").auto_shrink([false, false]).show(ui, |ui| {
            for v in vaults
                .iter()
                .filter(|v| l.filter.is_empty() || v.name.to_lowercase().contains(&l.filter.to_lowercase()))
                .take(300)
            {
                let r = row(
                    ui,
                    &widths,
                    &[
                        (v.name.clone(), text(), false),
                        (fmt_usd(v.tvl), text(), true),
                        (format!("{:+.1}%", v.apr * 100.0), signed(v.apr), true),
                        (short_address(&v.leader), muted(), true),
                    ],
                    false,
                );
                if r.clicked() {
                    actions.push(Action::Command(format!("HLW {}", v.address)));
                }
                r.on_hover_text("Click: open the vault's positions");
            }
        });
        if vaults.is_empty() {
            note(ui, "Loading vaults…");
        }
        return actions;
    }
    let ls = match data {
        None => {
            note(ui, "Loading the Hyperliquid leaderboard…");
            ui.add(egui::Spinner::new());
            return actions;
        }
        Some(Err(e)) => {
            ui.label(RichText::new(format!("Leaderboard unavailable: {e}")).font(ui_font(12.0)).color(theme::down()));
            return actions;
        }
        Some(Ok(ls)) => ls,
    };
    let f = l.filter.trim().to_lowercase();
    let mut rows: Vec<&Leader> = ls
        .iter()
        // Tiny accounts make ROI meaningless; keep the top of the book.
        .filter(|x| x.account_value >= 10_000.0)
        .filter(|x| {
            f.is_empty() || x.address.contains(&f) || x.name.as_deref().is_some_and(|n| n.to_lowercase().contains(&f))
        })
        .collect();
    let key = |x: &Leader| {
        let p = l.window.of(x);
        match l.sort {
            LeaderSort::Pnl => p.pnl,
            LeaderSort::Roi => p.roi,
            LeaderSort::Value => x.account_value,
            LeaderSort::Volume => p.volume,
        }
    };
    rows.sort_by(|a, b| key(b).total_cmp(&key(a)));
    note(ui, format!("{} wallets with $10K+ · click a row to open the wallet", rows.len()));
    let widths = [44.0, 170.0, 110.0, 110.0, 80.0, 110.0];
    row(ui, &widths, &hdr(&["#", "Trader", "Account", "PnL", "ROI", "Volume"], 2), true);
    let h = dp(19.0);
    egui::ScrollArea::vertical().id_salt("hl-leaders").auto_shrink([false, false]).show_rows(
        ui,
        h,
        rows.len().min(2_000),
        |ui, range| {
            for (k, x) in rows.iter().enumerate().skip(range.start).take(range.len()) {
                let p = l.window.of(x);
                let r = row(
                    ui,
                    &widths,
                    &[
                        ((k + 1).to_string(), muted(), false),
                        (
                            labels.display(&x.address),
                            if labels.label(&x.address).is_some() { amber() } else { text() },
                            false,
                        ),
                        (fmt_usd(x.account_value), text(), true),
                        (signed_usd(p.pnl), signed(p.pnl), true),
                        (format!("{:+.1}%", p.roi * 100.0), signed(p.roi), true),
                        (fmt_usd(p.volume), muted(), true),
                    ],
                    false,
                );
                if r.clicked() {
                    actions.push(Action::Command(format!("HLW {}", x.address)));
                }
            }
        },
    );
    actions
}

// ---- HLPOS: positioning of the top wallets -------------------------------------------

fn positioning(
    ui: &mut Ui,
    pp: &mut PositioningPane,
    data: Option<&Result<Arc<Positioning>, String>>,
    labels: &Labels,
) -> Vec<Action> {
    let mut actions = Vec::new();
    let pos = match data {
        None => {
            title(ui, &format!("{} POSITIONING", pp.coin));
            note(ui, format!("Scanning the top {} Hyperliquid wallets…", crate::worker::flow::SCAN_WALLETS));
            ui.add(egui::Spinner::new());
            return actions;
        }
        Some(Err(e)) => {
            ui.label(RichText::new(format!("Positioning unavailable: {e}")).font(ui_font(12.0)).color(theme::down()));
            return actions;
        }
        Some(Ok(p)) => p.clone(),
    };
    ui.horizontal_wrapped(|ui| {
        title(ui, "TOP-WALLET POSITIONING");
        for c in pos.coins.iter().take(14) {
            if ui
                .selectable_label(c.coin.eq_ignore_ascii_case(&pp.coin), RichText::new(&c.coin).font(mono(11.5)))
                .clicked()
            {
                pp.coin = c.coin.clone();
            }
        }
        if pos.simulated {
            simulated(ui, "data");
        }
    });
    let when = chrono::DateTime::from_timestamp(pos.updated, 0)
        .map(|d| d.with_timezone(&chrono::Local).format("%H:%M").to_string())
        .unwrap_or_default();
    note(
        ui,
        format!(
            "{} wallets scanned (top of the leaderboard by account value){} · {when} · their positions, not the whole market",
            pos.wallets,
            if pos.errors > 0 { format!(", {} failed", pos.errors) } else { String::new() }
        ),
    );
    let Some(c) = pos.coin(&pp.coin) else {
        note(ui, format!("None of the scanned wallets hold {}.", pp.coin));
        return actions;
    };
    // Long/short summary and split bar.
    let t = c.total;
    ui.horizontal_wrapped(|ui| {
        ui.label(
            RichText::new(format!("Long {} · {} wallets", fmt_usd(t.long_usd), t.longs))
                .font(mono(12.0))
                .color(theme::up()),
        );
        ui.label(
            RichText::new(format!("Short {} · {} wallets", fmt_usd(t.short_usd), t.shorts))
                .font(mono(12.0))
                .color(theme::down()),
        );
        ui.label(RichText::new(format!("net {}", signed_usd(t.net()))).font(mono(11.5)).color(signed(t.net())));
        ui.label(RichText::new(format!("uPnL {}", signed_usd(t.upnl))).font(mono(11.5)).color(signed(t.upnl)));
    });
    let (bar, _) = ui.allocate_exact_size(vec2(ui.available_width(), dp(8.0)), Sense::hover());
    let p = ui.painter_at(bar);
    let split = bar.left() + bar.width() * t.long_share() as f32;
    let strip = bar.shrink2(vec2(0.0, 1.0));
    p.rect_filled(Rect::from_min_max(strip.min, pos2(split, strip.bottom())), 2.0, theme::up());
    p.rect_filled(Rect::from_min_max(pos2(split, strip.top()), strip.max), 2.0, theme::down());
    let rest = ui.available_rect_before_wrap();
    if rest.width() > dp(640.0) {
        // Wide: cohorts and the liquidation map on the left, largest positions on the right.
        let x = rest.left() + rest.width() * 0.42;
        let left = Rect::from_min_max(rest.min, pos2(x - dp(6.0), rest.bottom()));
        let right = Rect::from_min_max(pos2(x, rest.top()), rest.max);
        let mut lu = ui.new_child(egui::UiBuilder::new().max_rect(left));
        cohort_table(&mut lu, c);
        lu.add_space(dp(4.0));
        let map = lu.available_rect_before_wrap();
        liq_map(&lu, map, c);
        let mut ru = ui.new_child(egui::UiBuilder::new().max_rect(right));
        egui::ScrollArea::vertical()
            .id_salt(("hlpos-top", &c.coin))
            .auto_shrink([false, false])
            .show(&mut ru, |ui| top_positions(ui, c, &mut actions, labels));
        ui.advance_cursor_after_rect(rest);
    } else {
        // Narrow: everything stacked in one scroll area.
        egui::ScrollArea::vertical().id_salt(("hlpos", &c.coin)).auto_shrink([false, false]).show(ui, |ui| {
            cohort_table(ui, c);
            ui.add_space(dp(4.0));
            let (map, _) = ui.allocate_exact_size(vec2(ui.available_width(), dp(240.0)), Sense::hover());
            liq_map(ui, map, c);
            ui.add_space(dp(4.0));
            top_positions(ui, c, &mut actions, labels);
        });
    }
    actions
}

fn cohort_table(ui: &mut Ui, c: &hyperliquid::CoinPositioning) {
    let widths = [150.0, 90.0, 90.0, 70.0];
    row(ui, &widths, &hdr(&["Cohort", "Long", "Short", "Long %"], 1), true);
    for (k, e) in c.cohorts.iter().enumerate() {
        if e.longs + e.shorts == 0 {
            continue;
        }
        row(
            ui,
            &widths,
            &[
                (COHORTS[k].0.to_string(), text(), false),
                (fmt_usd(e.long_usd), theme::up(), true),
                (fmt_usd(e.short_usd), theme::down(), true),
                (format!("{:.0}%", e.long_share() * 100.0), signed(e.long_share() - 0.5), true),
            ],
            false,
        );
    }
}

fn top_positions(ui: &mut Ui, c: &hyperliquid::CoinPositioning, actions: &mut Vec<Action>, labels: &Labels) {
    ui.label(RichText::new("LARGEST POSITIONS · click to open the wallet").font(mono(11.0)).color(muted()));
    let widths = [110.0, 56.0, 90.0, 90.0, 90.0, 80.0, 60.0];
    row(ui, &widths, &hdr(&["Wallet", "Side", "Value", "Entry", "Liq.", "uPnL", "Lev"], 2), true);
    for (addr, _av, p) in &c.top {
        let side = if p.is_long() { ("LONG", theme::up()) } else { ("SHORT", theme::down()) };
        let r = row(
            ui,
            &widths,
            &[
                (labels.display(addr), if labels.label(addr).is_some() { amber() } else { text() }, false),
                (side.0.into(), side.1, false),
                (fmt_usd(p.value), text(), true),
                (fmt_price(p.entry), text(), true),
                (p.liq_px.map_or("—".into(), fmt_price), muted(), true),
                (signed_usd(p.upnl), signed(p.upnl), true),
                (format!("{:.0}x", p.leverage), muted(), true),
            ],
            false,
        );
        if r.clicked() {
            actions.push(Action::Command(format!("HLW {addr}")));
        }
    }
}

/// Liquidation levels of the scanned positions around the price: long liquidations (below the
/// price) extend left in red, short liquidations (above) extend right in green.
fn liq_map(ui: &Ui, rect: Rect, c: &hyperliquid::CoinPositioning) {
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 6.0, theme::header().gamma_multiply(0.6));
    p.text(
        pos2(rect.left() + 6.0, rect.top() + 9.0),
        Align2::LEFT_CENTER,
        "Liquidation levels",
        ui_font(11.0),
        muted(),
    );
    if rect.height() < dp(60.0) {
        p.text(
            rect.center(),
            Align2::CENTER_CENTER,
            "enlarge the pane to see liquidation levels",
            ui_font(11.0),
            muted(),
        );
        return;
    }
    if c.liq_levels.is_empty() || c.mark <= 0.0 {
        p.text(rect.center(), Align2::CENTER_CENTER, "no liquidation levels within ±50%", ui_font(11.0), muted());
        return;
    }
    let plot = Rect::from_min_max(
        pos2(rect.left() + dp(70.0), rect.top() + dp(20.0)),
        pos2(rect.right() - 8.0, rect.bottom() - 6.0),
    );
    // Show ±15% around the price, wider if nothing is there.
    let mut span = 0.15;
    if !c.liq_levels.iter().any(|l| (l.0 / c.mark - 1.0).abs() < span) {
        span = 0.5;
    }
    let (lo, hi) = (c.mark * (1.0 - span), c.mark * (1.0 + span));
    let y = |v: f64| plot.bottom() - ((v - lo) / (hi - lo)) as f32 * plot.height();
    let vmax =
        c.liq_levels.iter().filter(|l| (lo..hi).contains(&l.0)).map(|l| l.1.max(l.2)).fold(0.0, f64::max).max(1.0);
    let mid_x = plot.center().x;
    let bh = ((y(0.0) - y(c.bucket)).abs()).clamp(1.0, dp(14.0));
    for &(px, long, short) in c.liq_levels.iter().filter(|l| (lo..hi).contains(&l.0)) {
        let yy = y(px + c.bucket / 2.0);
        if long > 0.0 {
            let w = (plot.width() / 2.0) * (long / vmax) as f32;
            p.rect_filled(
                Rect::from_min_max(pos2(mid_x - w, yy - bh / 2.0), pos2(mid_x, yy + bh / 2.0)),
                1.0,
                theme::down().gamma_multiply(0.8),
            );
        }
        if short > 0.0 {
            let w = (plot.width() / 2.0) * (short / vmax) as f32;
            p.rect_filled(
                Rect::from_min_max(pos2(mid_x, yy - bh / 2.0), pos2(mid_x + w, yy + bh / 2.0)),
                1.0,
                theme::up().gamma_multiply(0.8),
            );
        }
    }
    let step = hd_flow::nice_step((hi - lo) / 6.0);
    let mut v = (lo / step).ceil() * step;
    while v <= hi {
        p.text(pos2(rect.left() + 6.0, y(v)), Align2::LEFT_CENTER, fmt_price(v), mono(10.5), muted());
        p.hline(plot.x_range(), y(v), Stroke::new(1.0, theme::grid().gamma_multiply(0.4)));
        v += step;
    }
    p.hline(plot.x_range(), y(c.mark), Stroke::new(1.5, amber()));
    p.text(
        pos2(plot.right(), y(c.mark) - 7.0),
        Align2::RIGHT_CENTER,
        format!("mark {}", fmt_price(c.mark)),
        mono(10.5),
        amber(),
    );
    p.text(
        pos2(plot.left() + 4.0, plot.bottom() - 6.0),
        Align2::LEFT_CENTER,
        "longs liquidated",
        ui_font(10.0),
        theme::down(),
    );
    p.text(
        pos2(plot.right() - 4.0, plot.bottom() - 6.0),
        Align2::RIGHT_CENTER,
        "shorts liquidated",
        ui_font(10.0),
        theme::up(),
    );
}

// ---- PRINTS --------------------------------------------------------------------------

fn prints(ui: &mut Ui, pp: &mut PrintsPane, tape: Option<&SharedPrints>, labels: &Labels) -> Vec<Action> {
    let mut actions = Vec::new();
    ui.horizontal_wrapped(|ui| {
        title(
            ui,
            &match &pp.coin {
                Some(c) => format!("{c} WHALE PRINTS"),
                None => "WHALE PRINTS".into(),
            },
        );
        let mins = [25_000.0, 100_000.0, 500_000.0, 1_000_000.0];
        if let Some(i) = widgets::segmented(
            ui,
            "prints-min",
            &["$25K+", "$100K+", "$500K+", "$1M+"],
            mins.iter().position(|m| *m == pp.min_usd),
            20.0,
        ) {
            pp.min_usd = mins[i];
        }
        if pp.coin.is_some() && ui.button(RichText::new("All coins").font(ui_font(11.5))).clicked() {
            pp.coin = None;
        }
    });
    let Some(tape) = tape else {
        note(ui, "Connecting to Hyperliquid trades…");
        return actions;
    };
    let Ok(t) = tape.lock() else { return actions };
    ui.horizontal_wrapped(|ui| {
        let live = t.status == "live" || t.status == "simulated";
        ui.label(RichText::new(format!("● Hyperliquid {}", t.status)).font(mono(11.0)).color(if live {
            theme::up()
        } else {
            muted()
        }));
        if t.simulated {
            simulated(ui, "data");
        }
        note(ui, "On-chain trades: click a row to open the aggressor's wallet.");
    });
    let items: Vec<_> = t
        .prints
        .iter()
        .filter(|x| x.usd >= pp.min_usd && pp.coin.as_deref().is_none_or(|c| x.coin.eq_ignore_ascii_case(c)))
        .take(1_000)
        .cloned()
        .collect();
    drop(t);
    let widths = [72.0, 64.0, 56.0, 96.0, 90.0, 120.0, 120.0];
    row(ui, &widths, &hdr(&["Time", "Coin", "Side", "Price", "Value", "Taker", "Maker"], 3), true);
    egui::ScrollArea::vertical().id_salt("prints").auto_shrink([false, false]).show_rows(
        ui,
        dp(19.0),
        items.len(),
        |ui, range| {
            for x in &items[range] {
                let c = if x.buy { theme::up() } else { theme::down() };
                let r = row(
                    ui,
                    &widths,
                    &[
                        (hms(x.ts), muted(), false),
                        (x.coin.clone(), amber(), false),
                        ((if x.buy { "BUY" } else { "SELL" }).into(), c, false),
                        (fmt_price(x.price), text(), true),
                        (fmt_usd(x.usd), if x.usd >= 1e6 { c } else { text() }, true),
                        (
                            labels.display(&x.taker),
                            if labels.label(&x.taker).is_some() { amber() } else { text() },
                            true,
                        ),
                        (labels.display(&x.maker), muted(), true),
                    ],
                    false,
                );
                if r.clicked() {
                    actions.push(Action::Command(format!("HLW {}", x.taker)));
                }
                r.context_menu(|ui| {
                    if ui.button("Open taker wallet").clicked() {
                        actions.push(Action::Command(format!("HLW {}", x.taker)));
                        ui.close();
                    }
                    if ui.button("Open maker wallet").clicked() {
                        actions.push(Action::Command(format!("HLW {}", x.maker)));
                        ui.close();
                    }
                });
            }
        },
    );
    ui.ctx().request_repaint_after(Duration::from_millis(500));
    actions
}

// ---- OFLOW ---------------------------------------------------------------------------

fn option_flow(ui: &mut Ui, o: &mut OptionFlowPane, tape: Option<&SharedOptionFlow>) -> Vec<Action> {
    let mut actions = Vec::new();
    ui.horizontal_wrapped(|ui| {
        title(ui, "OPTIONS FLOW");
        note(ui, "Deribit");
        let ccys: [Option<&str>; 3] = [None, Some("BTC"), Some("ETH")];
        if let Some(i) = widgets::segmented(
            ui,
            "oflow-ccy",
            &["All", "BTC", "ETH"],
            ccys.iter().position(|c| *c == o.currency.as_deref()),
            20.0,
        ) {
            o.currency = ccys[i].map(str::to_string);
        }
        let wins = [60i64, 240, 1440];
        if let Some(i) = widgets::segmented(
            ui,
            "oflow-win",
            &["1h", "4h", "24h"],
            wins.iter().position(|w| *w == o.window_min),
            20.0,
        ) {
            o.window_min = wins[i];
        }
        let mins = [0.0, 10_000.0, 100_000.0, 1_000_000.0];
        if let Some(i) = widgets::segmented(
            ui,
            "oflow-min",
            &["All", "$10K+", "$100K+", "$1M+"],
            mins.iter().position(|m| *m == o.min_premium),
            20.0,
        ) {
            o.min_premium = mins[i];
        }
        ui.checkbox(&mut o.blocks_only, RichText::new("Blocks only").font(ui_font(11.5)));
    });
    let Some(tape) = tape else {
        note(ui, "Loading Deribit option trades…");
        return actions;
    };
    let Ok(t) = tape.lock() else { return actions };
    let since = chrono::Utc::now().timestamp_millis() - o.window_min * 60_000;
    let ccy = o.currency.as_deref();
    let prem = t.premiums(ccy, since);
    let strikes: Vec<_> = t.by_strike(ccy, since).into_iter().take(8).collect();
    let expiries = t.by_expiry(ccy, since);
    let items: Vec<OptionTrade> = t
        .filtered(ccy, since)
        .filter(|x| x.premium_usd() >= o.min_premium && (!o.blocks_only || x.block))
        .take(1_500)
        .cloned()
        .collect();
    let (sim, err) = (t.simulated, t.error.clone());
    drop(t);
    ui.horizontal_wrapped(|ui| {
        if sim {
            simulated(ui, "data");
        }
        if let Some(e) = err {
            ui.label(RichText::new("Deribit offline").font(ui_font(11.0)).color(theme::down())).on_hover_text(e);
        }
        for (k, v, c) in [
            ("Calls bought", prem.calls_bought, theme::up()),
            ("Calls sold", prem.calls_sold, theme::up().gamma_multiply(0.6)),
            ("Puts bought", prem.puts_bought, theme::down()),
            ("Puts sold", prem.puts_sold, theme::down().gamma_multiply(0.6)),
        ] {
            ui.label(RichText::new(k).font(ui_font(11.0)).color(muted()));
            ui.label(RichText::new(fmt_usd(v)).font(mono(12.5)).color(c));
        }
        ui.label(
            RichText::new(format!("P/C {}", prem.put_call().map_or("—".into(), |r| format!("{r:.2}"))))
                .font(mono(12.5))
                .color(text()),
        );
        note(ui, format!("{} trades", prem.count));
    });
    ui.horizontal_wrapped(|ui| {
        note(ui, "Top strikes:");
        for (k, c, p) in &strikes {
            ui.label(RichText::new(format!("{k:.0}")).font(mono(11.5)).color(amber()));
            ui.label(RichText::new(format!("C {} P {}", fmt_usd(*c), fmt_usd(*p))).font(mono(11.0)).color(muted()));
        }
    });
    ui.horizontal_wrapped(|ui| {
        note(ui, "By expiry:");
        for (exp, p) in &expiries {
            let d = chrono::DateTime::from_timestamp(*exp, 0)
                .map(|d| d.format("%d%b").to_string().to_uppercase())
                .unwrap_or_default();
            ui.label(RichText::new(d).font(mono(11.5)).color(amber()));
            ui.label(
                RichText::new(format!("C {} P {}", fmt_usd(p.calls()), fmt_usd(p.puts())))
                    .font(mono(11.0))
                    .color(muted()),
            );
        }
    });
    let widths = [72.0, 46.0, 150.0, 56.0, 76.0, 90.0, 60.0, 60.0, 56.0];
    row(ui, &widths, &hdr(&["Time", "Ccy", "Contract", "Side", "Size", "Premium", "Price", "IV", "Block"], 4), true);
    egui::ScrollArea::vertical().id_salt("oflow").auto_shrink([false, false]).show_rows(
        ui,
        dp(19.0),
        items.len(),
        |ui, range| {
            for x in &items[range] {
                let cp = if x.call { theme::up() } else { theme::down() };
                let r = row(
                    ui,
                    &widths,
                    &[
                        (hms(x.ts), muted(), false),
                        (x.currency.clone(), amber(), false),
                        (x.label(), cp, false),
                        ((if x.buy { "BUY" } else { "SELL" }).into(), if x.buy { text() } else { muted() }, false),
                        (format!("{:.1}", x.amount), text(), true),
                        (fmt_usd(x.premium_usd()), if x.premium_usd() >= 100_000.0 { amber() } else { text() }, true),
                        (format!("{:.4}", x.price), muted(), true),
                        (x.iv.map_or("—".into(), |v| format!("{:.1}%", v * 100.0)), text(), true),
                        ((if x.block { "BLOCK" } else { "" }).into(), amber(), true),
                    ],
                    false,
                );
                if r.clicked() {
                    actions.push(Action::Command(format!("DERIV {}", x.currency)));
                }
                r.on_hover_text(format!(
                    "{} · notional {} · index {}",
                    x.instrument,
                    fmt_usd(x.notional_usd()),
                    fmt_price(x.index)
                ));
            }
        },
    );
    ui.ctx().request_repaint_after(Duration::from_secs(1));
    actions
}

#[cfg(test)]
mod tests {
    use super::*;
    use hd_flow::hyperliquid::Position;

    #[test]
    fn wallet_change_toasts() {
        let pos = |coin: &str, size: f64| Position {
            coin: coin.into(),
            size,
            value: size.abs() * 100.0,
            ..Default::default()
        };
        let acct = |ps| Account {
            address: "0x31ca8395cf837de08b24da3f660e77761dfb974b".into(),
            positions: ps,
            ..Default::default()
        };
        let old = acct(vec![pos("BTC", 1.0), pos("ETH", -2.0), pos("SOL", 10.0)]);
        let new = acct(vec![pos("BTC", 1.1), pos("ETH", 2.0), pos("SOL", 20.0), pos("HYPE", 5.0)]);
        let t = position_changes(&old, &new);
        assert_eq!(t.len(), 3, "{t:?}");
        assert!(t.iter().any(|m| m.contains("flipped ETH to long")));
        assert!(t.iter().any(|m| m.contains("added to SOL long")));
        assert!(t.iter().any(|m| m.contains("opened HYPE long")));
        let closed = position_changes(&new, &acct(vec![]));
        assert_eq!(closed.len(), 4);
        assert!(closed[0].starts_with("0x31ca…974b closed"));
    }
}
