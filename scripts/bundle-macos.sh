#!/usr/bin/env bash
# Wrap a release binary in a minimal macOS .app bundle (retina-capable, dark-mode aware), written
# next to the binary as Hyperdesk.app: target/release/hyperdesk unless another path is given.
# The executable inside is named Hyperdesk too, since the app menu's About, Hide and Quit items
# take the process name.
set -euo pipefail
cd "$(dirname "$0")/.."
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
bin=${1:-target/release/hyperdesk}
app=$(dirname "$bin")/Hyperdesk.app
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$bin" "$app/Contents/MacOS/Hyperdesk"
cp app/assets/AppIcon.icns "$app/Contents/Resources/AppIcon.icns"
cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Hyperdesk</string>
  <key>CFBundleDisplayName</key><string>Hyperdesk</string>
  <key>CFBundleIdentifier</key><string>app.hyperdesk</string>
  <key>CFBundleExecutable</key><string>Hyperdesk</string>
  <key>CFBundleIconFile</key><string>AppIcon</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>${version}</string>
  <key>CFBundleVersion</key><string>${version}</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSSupportsAutomaticGraphicsSwitching</key><true/>
  <key>LSApplicationCategoryType</key><string>public.app-category.finance</string>
</dict>
</plist>
PLIST
echo "bundled $app"
