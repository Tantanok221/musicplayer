#!/bin/zsh
set -euo pipefail
cd "${0:A:h:h}"
profile="${1:-release}"
case "$profile" in
  release) "${CARGO:-$HOME/.cargo/bin/cargo}" build --release ;;
  debug) "${CARGO:-$HOME/.cargo/bin/cargo}" build ;;
  *) print -u2 'Usage: scripts/bundle.sh [release|debug]'; exit 1 ;;
esac
mkdir -p "$PWD/dist"
staging_dir="$(mktemp -d "$PWD/dist/.sonora-bundle.XXXXXX")"
trap 'rm -rf "$staging_dir"' EXIT
app_path="$staging_dir/Sonora.app"
mkdir -p "$app_path/Contents/MacOS" "$app_path/Contents/Resources"
cp "target/$profile/sonora" "$app_path/Contents/MacOS/sonora"
if [[ ! -f assets/Sonora.icns ]]; then
  mkdir -p target/Sonora.iconset
  /usr/bin/swift scripts/icon.swift target/Sonora.iconset
  /usr/bin/iconutil -c icns target/Sonora.iconset -o assets/Sonora.icns
fi
cp assets/Sonora.icns "$app_path/Contents/Resources/"
cat > "$app_path/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleName</key><string>Sonora</string>
<key>CFBundleDisplayName</key><string>Sonora</string>
<key>CFBundleIdentifier</key><string>app.sonora.player</string>
<key>CFBundleExecutable</key><string>sonora</string>
<key>CFBundleIconFile</key><string>Sonora.icns</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>0.1.0</string>
<key>CFBundleVersion</key><string>1</string>
<key>LSMinimumSystemVersion</key><string>14.0</string>
<key>NSHighResolutionCapable</key><true/>
<key>NSLocalNetworkUsageDescription</key><string>Sonora connects to the Navidrome server you enter to browse and play your music.</string>
<key>NSPrincipalClass</key><string>NSApplication</string>
</dict></plist>
PLIST
/usr/bin/codesign --force --sign - "$app_path"
destination="$PWD/dist/Sonora.app"
# Replace the bundle without overwriting the running executable's inode.
if [[ -d "$destination" ]]; then
  mv "$destination" "$staging_dir/previous.app"
fi
if ! mv "$app_path" "$destination"; then
  if [[ -d "$staging_dir/previous.app" ]]; then
    mv "$staging_dir/previous.app" "$destination"
  fi
  exit 1
fi
print "Built $destination"
