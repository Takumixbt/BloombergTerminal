//! Small OS differences: the home folder, `~/` expansion, opening files and URLs with the
//! system handler, and the Windows log file.

use std::path::PathBuf;

/// The user's home folder: `$HOME`, or `%USERPROFILE%` on Windows.
pub fn home_dir() -> Option<PathBuf> {
    let var = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    if cfg!(windows) { var("USERPROFILE").or_else(|| var("HOME")) } else { var("HOME") }
}

/// Expand a leading `~/` (or `~\` on Windows) to the home folder.
pub fn expand_home(p: &str) -> PathBuf {
    let rest = p.strip_prefix("~/").or_else(|| if cfg!(windows) { p.strip_prefix("~\\") } else { None });
    match rest {
        Some(rest) => home_dir().unwrap_or_default().join(rest),
        None => PathBuf::from(p),
    }
}

/// Open a URL, file or folder with the system handler (`open`, `explorer`, `xdg-open`).
pub fn open_external(target: impl AsRef<std::ffi::OsStr>) -> std::io::Result<()> {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(windows) {
        "explorer"
    } else {
        "xdg-open"
    };
    std::process::Command::new(opener).arg(target).spawn().map(|_| ())
}

/// Where logs go when there is no console to print to: release builds on Windows run as a
/// GUI app, so they log to `<data dir>/hyperdesk.log` (`HD_LOG_FILE` overrides; set it to
/// an empty value to log to stdout instead).
pub fn log_file() -> Option<PathBuf> {
    match std::env::var_os("HD_LOG_FILE") {
        Some(v) if v.is_empty() => None,
        Some(v) => Some(PathBuf::from(v)),
        None if cfg!(all(windows, not(debug_assertions))) => Some(crate::worker::data_dir().join("hyperdesk.log")),
        None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_home_prefix_only() {
        assert_eq!(expand_home("/abs/path"), PathBuf::from("/abs/path"));
        assert_eq!(expand_home("rel/~/x"), PathBuf::from("rel/~/x"));
        if let Some(h) = home_dir() {
            assert_eq!(expand_home("~/layouts"), h.join("layouts"));
        }
    }
}
