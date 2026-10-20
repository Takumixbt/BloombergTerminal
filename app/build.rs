//! Embed the app icon and version info in the Windows executable (shown by Explorer, the
//! taskbar and Alt+Tab). A no-op for other targets.

fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut res = winresource::WindowsResource::new();
    res.set_icon("assets/icon.ico")
        .set("ProductName", "terminal-rs")
        .set("FileDescription", "terminal-rs market terminal")
        .set("InternalName", "terminal-rs");
    if let Err(e) = res.compile() {
        // A missing resource compiler only loses the icon; the app still builds.
        println!("cargo:warning=could not embed the Windows icon: {e}");
    }
}
