//! Small drawing and formatting helpers shared by the market panes: headings, notes, the
//! SIMULATED badge, number formats, painted table rows, mini charts and colour ramps.

use egui::{Align2, Color32, Pos2, Rect, RichText, Sense, Shape, Stroke, Ui, pos2, vec2};
use hd_flow::fmt_usd;

use crate::theme::{self, amber, dp, mono, muted, text, ui_font};
use crate::widgets;

pub(super) fn simulated(ui: &mut Ui, what: &str) {
    ui.label(RichText::new(" SIMULATED ").font(ui_font(10.5)).color(Color32::BLACK).background_color(amber()))
        .on_hover_text(format!("Offline sample data (HD_PROVIDER=sample); {what} are synthetic."));
}

pub(super) fn title(ui: &mut Ui, s: &str) {
    ui.label(RichText::new(s).font(mono(12.5)).color(amber()));
}

pub(super) fn note(ui: &mut Ui, s: impl Into<String>) {
    ui.label(RichText::new(s.into()).font(ui_font(11.0)).color(muted()));
}

/// Decimals that keep about five significant digits.
pub(super) fn price_dp(p: f64) -> usize {
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

pub(super) fn fmt_price(p: f64) -> String {
    format!("{:.*}", price_dp(p), p)
}

pub(super) fn pct(x: f64, dp: usize) -> String {
    format!("{:+.*}%", dp, x * 100.0)
}

pub(super) fn signed(x: f64) -> Color32 {
    if x > 0.0 {
        theme::up()
    } else if x < 0.0 {
        theme::down()
    } else {
        muted()
    }
}

pub(super) fn hms(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|d| d.with_timezone(&chrono::Local).format("%H:%M:%S").to_string())
        .unwrap_or_default()
}

/// A compact line or bar chart of `(unix_secs, value)` with a title, last value and range.
pub(super) fn mini_chart(
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

/// Lay out `n` chart cells in a grid filling `rect`.
pub(super) fn grid(rect: Rect, n: usize, min_w: f32, h: f32) -> Vec<Rect> {
    let cols = ((rect.width() / min_w).floor() as usize).clamp(1, n.max(1));
    let w = rect.width() / cols as f32;
    (0..n)
        .map(|k| {
            let (r, c) = (k / cols, k % cols);
            Rect::from_min_size(pos2(rect.left() + c as f32 * w, rect.top() + r as f32 * h), vec2(w, h)).shrink(3.0)
        })
        .collect()
}

/// Bookmap-style ramp: near-black navy → blue → cyan → amber → white-hot. Typical liquidity sits
/// in the dark first half so walls stand out.
pub(super) fn heat_color(t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let stops: [(f32, [f32; 3]); 6] = [
        (0.0, [8.0, 10.0, 30.0]),
        (0.2, [12.0, 28.0, 78.0]),
        (0.45, [16.0, 96.0, 170.0]),
        (0.65, [0.0, 200.0, 210.0]),
        (0.85, [250.0, 200.0, 40.0]),
        (1.0, [255.0, 255.0, 255.0]),
    ];
    let k = stops.iter().position(|s| s.0 >= t).unwrap_or(stops.len() - 1).max(1);
    let (a, b) = (stops[k - 1], stops[k]);
    let u = (t - a.0) / (b.0 - a.0).max(1e-6);
    let c = |i: usize| (a.1[i] + (b.1[i] - a.1[i]) * u) as u8;
    Color32::from_rgb(c(0), c(1), c(2))
}

/// `text` wrapped to at most `rows` lines of `max_w`, the last cut with an ellipsis.
pub(super) fn one_line_rows(
    p: &egui::Painter,
    text: &str,
    font: egui::FontId,
    color: Color32,
    max_w: f32,
    rows: usize,
) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple(text.to_string(), font, color, max_w.max(10.0));
    job.wrap.max_rows = rows;
    job.wrap.overflow_character = Some('…');
    p.layout_job(job)
}

/// `text` laid out on one line, cut with an ellipsis at `max_w`.
pub(super) fn one_line(
    p: &egui::Painter,
    text: &str,
    font: egui::FontId,
    color: Color32,
    max_w: f32,
) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_string(), font, color);
    job.wrap = egui::text::TextWrapping {
        max_width: max_w.max(10.0),
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    p.layout_job(job)
}

pub(super) fn signed_usd(x: f64) -> String {
    if x >= 0.0 { format!("+{}", fmt_usd(x)) } else { fmt_usd(x) }
}

/// `12.3M`, `649.6K`, `1.2345` for coin quantities.
pub(super) fn qty(x: f64) -> String {
    let a = x.abs();
    if a >= 1e6 {
        format!("{:.2}M", x / 1e6)
    } else if a >= 1e4 {
        format!("{:.1}K", x / 1e3)
    } else if a >= 100.0 {
        format!("{x:.1}")
    } else {
        format!("{x:.4}")
    }
}

pub(super) fn ago(ms: i64) -> String {
    let s = (chrono::Utc::now().timestamp_millis() - ms).max(0) / 1000;
    if s < 3600 {
        format!("{}m", s / 60)
    } else if s < 86_400 {
        format!("{}h", s / 3600)
    } else {
        format!("{}d", s / 86_400)
    }
}

/// A painted table row: `cells` are `(text, colour, right-aligned)` at fixed widths. When the pane
/// is narrower than the columns, they shrink a little, trailing columns that still do not fit are
/// left out, and each cell clips its text instead of spilling into its neighbour.
pub(super) fn row(ui: &mut Ui, widths: &[f32], cells: &[(String, Color32, bool)], header: bool) -> egui::Response {
    let s = dp(1.0);
    let total: f32 = widths.iter().map(|w| w * s).sum();
    let avail = ui.available_width();
    let s = if total > avail && total > 0.0 { s * (avail / total).max(0.85) } else { s };
    let h = dp(19.0);
    let (r, resp) = ui.allocate_exact_size(vec2(avail, h), if header { Sense::hover() } else { Sense::click() });
    let p = ui.painter_at(r);
    if header {
        p.rect_filled(r, 3.0, theme::header());
    } else {
        widgets::hover_wash(ui, r, &resp, false);
    }
    let mut x = r.left();
    for (k, (t, c, right)) in cells.iter().enumerate() {
        if x >= r.right() {
            break;
        }
        let w = widths.get(k).copied().unwrap_or(60.0) * s;
        if k > 0 && x + w > r.right() + 1.0 {
            // A column that does not fit is left out rather than shown half-cut.
            break;
        }
        let cell = Rect::from_min_max(pos2(x, r.top()), pos2((x + w).min(r.right()), r.bottom()));
        let cp = p.with_clip_rect(cell.shrink2(vec2(2.0, 0.0)));
        let font = if header { ui_font(11.0) } else { mono(12.0) };
        if *right {
            cp.text(pos2(cell.right() - 6.0, r.center().y), Align2::RIGHT_CENTER, t, font, *c);
        } else {
            cp.text(pos2(x + 6.0, r.center().y), Align2::LEFT_CENTER, t, font, *c);
        }
        x += w;
    }
    resp
}

pub(super) fn hdr(titles: &[&str], first_left: usize) -> Vec<(String, Color32, bool)> {
    titles.iter().enumerate().map(|(k, t)| (t.to_string(), muted(), k >= first_left)).collect()
}
