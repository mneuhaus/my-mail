#!/bin/sh
# Build "Just Mail.app" (release, ad-hoc signed) and the `jm` CLI into dist/.
# --install: also copy the app to /Applications and link `jm` into ~/bin.
set -eu
cd "$(dirname "$0")/.."
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
APP="dist/Just Mail.app"

# Two separate builds on purpose: built together, cargo would unify features and compile
# jm-core with `send` for the CLI as well. Alone, `jm` has no sending code at all.
cargo build --release -p jm-cli
cargo build --release -p jm-app
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp target/release/just-mail "$APP/Contents/MacOS/Just Mail"
cp target/release/jm dist/jm
cp crates/jm-app/assets/JustMail.icns "$APP/Contents/Resources/JustMail.icns"
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Just Mail</string>
  <key>CFBundleDisplayName</key><string>Just Mail</string>
  <key>CFBundleIdentifier</key><string>nrw.neuhaus.justmail</string>
  <key>CFBundleExecutable</key><string>Just Mail</string>
  <key>CFBundleIconFile</key><string>JustMail</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>LSMinimumSystemVersion</key><string>13.0</string>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST
xattr -cr "$APP" dist/jm
codesign --force --sign - dist/jm
codesign --force --sign - "$APP"
echo "built $APP and dist/jm ($VERSION)"

if [ "${1:-}" = "--install" ]; then
    rm -rf "/Applications/Just Mail.app"
    ditto "$APP" "/Applications/Just Mail.app"
    mkdir -p "$HOME/bin"
    ln -sf "$PWD/dist/jm" "$HOME/bin/jm"
    echo "installed /Applications/Just Mail.app and ~/bin/jm"
fi
