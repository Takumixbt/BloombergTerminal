#!/usr/bin/env bash
# Type-check and lint the whole workspace for macOS (aarch64-apple-darwin) from Linux,
# using zig as the C cross-compiler for the bundled C libraries (SQLite, aws-lc).
# Everything compiles; the final link needs the macOS SDK, so run that on a Mac or in CI.
#
#   pip install ziglang   # or put `zig` on PATH
#   rustup target add aarch64-apple-darwin
#   scripts/check-macos-cross.sh
set -euo pipefail
ZIG="${ZIG:-$(command -v zig || python3 -c 'import ziglang, os; print(os.path.join(os.path.dirname(ziglang.__file__), "zig"))')}"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
cat > "$tmp/cc" <<CC
#!/usr/bin/env bash
args=()
for a in "\$@"; do case "\$a" in --target=*|-target) ;; *) args+=("\$a");; esac; done
exec "$ZIG" cc -target aarch64-macos "\${args[@]}"
CC
printf '#!/usr/bin/env bash\nexec "%s" ar "$@"\n' "$ZIG" > "$tmp/ar"
chmod +x "$tmp/cc" "$tmp/ar"
export CC_aarch64_apple_darwin="$tmp/cc" AR_aarch64_apple_darwin="$tmp/ar"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-target/macos-cross}"
cargo clippy --target aarch64-apple-darwin --workspace --all-targets -- -D warnings
echo "aarch64-apple-darwin: workspace compiles and lints cleanly (link step needs the macOS SDK)"
