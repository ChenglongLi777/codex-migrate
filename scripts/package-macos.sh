#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
APP="$ROOT/dist/Codex Migrate.app"
MACOS="$APP/Contents/MacOS"
RESOURCES="$APP/Contents/Resources"
TARGET=${1:-}
if [ -n "$TARGET" ]; then
  BINARY="$ROOT/target/$TARGET/release/codex-migrate-gui"
else
  BINARY="$ROOT/target/release/codex-migrate-gui"
fi
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT/Cargo.toml" | head -n 1)
BUILD_VERSION=$(printf '%s' "$VERSION" | tr -d '.')

if [ ! -x "$BINARY" ]; then
  echo "GUI binary not found. Run: cargo build --release --features gui --bins" >&2
  exit 1
fi

rm -rf "$APP"
mkdir -p "$MACOS" "$RESOURCES"
cp "$BINARY" "$MACOS/codex-migrate-gui"
cp "$ROOT/LICENSE" "$RESOURCES/LICENSE"
cp "$ROOT/assets/icons/CodexMigrate.icns" "$RESOURCES/CodexMigrate.icns"

cat > "$APP/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDisplayName</key>
  <string>Codex Migrate</string>
  <key>CFBundleExecutable</key>
  <string>codex-migrate-gui</string>
  <key>CFBundleIdentifier</key>
  <string>dev.codex-migrate.desktop</string>
  <key>CFBundleIconFile</key>
  <string>CodexMigrate</string>
  <key>CFBundleInfoDictionaryVersion</key>
  <string>6.0</string>
  <key>CFBundleName</key>
  <string>Codex Migrate</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleShortVersionString</key>
  <string>__VERSION__</string>
  <key>CFBundleVersion</key>
  <string>__BUILD_VERSION__</string>
  <key>LSMinimumSystemVersion</key>
  <string>11.0</string>
  <key>NSHighResolutionCapable</key>
  <true/>
</dict>
</plist>
PLIST

sed -i.bak \
  -e "s/__VERSION__/$VERSION/g" \
  -e "s/__BUILD_VERSION__/$BUILD_VERSION/g" \
  "$APP/Contents/Info.plist"
rm -f "$APP/Contents/Info.plist.bak"
chmod +x "$MACOS/codex-migrate-gui"
echo "Created $APP"
