//! Hyperdesk: a keyboard-driven market terminal with GPU charts, crypto derivatives and order
//! flow, Hyperliquid trader intelligence, prediction markets and congressional trade tracking.

// Release builds on Windows are GUI apps (no console window); logs go to a file instead.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod anim;
mod app;
mod chart_view;
mod command;
mod congress_view;
mod dock;
mod layout;
mod layouts;
mod markets_view;
mod palette;
mod platform;
mod prefs;
mod theme;
mod themes;
mod widgets;
mod worker;

use std::time::Instant;

fn main() -> eframe::Result {
    let t0 = Instant::now();
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "info,wgpu_core=warn,wgpu_hal=warn,naga=warn".into());
    let log_file = platform::log_file().and_then(|path| {
        std::fs::create_dir_all(path.parent()?).ok()?;
        std::fs::File::create(&path).ok()
    });
    match log_file {
        Some(f) => tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_ansi(false)
            .with_writer(std::sync::Mutex::new(f))
            .init(),
        None => tracing_subscriber::fmt().with_env_filter(filter).init(),
    }

    // MSAA smooths chart lines and drawings; 4x by default on macOS and Windows, where a real
    // GPU is the norm (HD_MSAA overrides).
    let msaa: u16 = std::env::var("HD_MSAA")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|n| matches!(n, 1 | 2 | 4 | 8))
        .unwrap_or(if cfg!(any(target_os = "macos", windows)) { 4 } else { 1 });
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Hyperdesk")
        .with_app_id("hyperdesk")
        .with_icon(eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png")).expect("app icon"))
        .with_inner_size([1600.0, 960.0])
        .with_min_inner_size([900.0, 560.0]);
    if cfg!(target_os = "macos") {
        // Content extends under a transparent title bar; hd-macos adds the unified
        // toolbar (inset traffic lights) and sidebar vibrancy on the first frame.
        viewport = viewport
            .with_transparent(true)
            .with_fullsize_content_view(true)
            .with_title_shown(false)
            .with_titlebar_shown(false);
    }
    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        viewport,
        multisampling: if msaa > 1 { msaa } else { 0 },
        ..Default::default()
    };
    eframe::run_native(
        "hyperdesk",
        options,
        Box::new(move |cc| {
            let rs = cc.wgpu_render_state.as_ref().ok_or("Hyperdesk requires the wgpu renderer")?;
            rs.renderer.write().callback_resources.insert(hd_chart::gpu::ChartRenderer::new(
                &rs.device,
                rs.target_format,
                u32::from(msaa),
            ));
            let provider = hd_data::from_env();
            let name = provider.name().to_string();
            let worker = worker::Worker::spawn(cc.egui_ctx.clone(), provider)?;
            tracing::info!("window + GPU ready in {:?} (adapter: {})", t0.elapsed(), rs.adapter.get_info().name);
            Ok(Box::new(app::TerminalApp::new(&cc.egui_ctx, worker, name)))
        }),
    )
}
