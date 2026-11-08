#!/usr/bin/env bash
set -euo pipefail

if [[ $(uname -s) != Linux ]]; then
  printf '%s\n' 'This installer is for Linux. On macOS, use scripts/bundle.sh.' >&2
  exit 1
fi
if (( $# )); then
  printf '%s\n' 'Usage: scripts/install-linux.sh' >&2
  exit 1
fi

cd "$(dirname "${BASH_SOURCE[0]}")/.."
root=$PWD
data_home=${XDG_DATA_HOME:-$HOME/.local/share}
bin_dir="$HOME/.local/bin"
target_dir=${CARGO_TARGET_DIR:-$root/target}
desktop_dir="$data_home/applications"
icon_dir="$data_home/icons/hicolor/512x512/apps"

# Build before touching the installed copy. A failed build leaves it usable.
"${CARGO:-cargo}" build --release --locked --target-dir "$target_dir"
install -d "$bin_dir" "$desktop_dir" "$icon_dir"
staging_dir=$(mktemp -d "$bin_dir/.sonora-install.XXXXXX")
trap 'rm -rf "$staging_dir"' EXIT
install -m 755 "$target_dir/release/sonora" "$staging_dir/sonora"
install -m 644 assets/sonora.png "$icon_dir/app.sonora.player.png"

# Desktop Exec has its own escaping rules, independent of shell quoting.
executable="$bin_dir/sonora"
executable=${executable//\\/\\\\}
executable=${executable//\"/\\\"}
executable=${executable//\$/\\\$}
executable=${executable//\`/\\\`}
executable=${executable//%/%%}
# Escape those backslashes again for the desktop entry's string-value layer.
executable=${executable//\\/\\\\}
cat > "$staging_dir/app.sonora.player.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=Sonora
Comment=Browse and play your Navidrome music library
Exec="$executable"
Icon=app.sonora.player
Terminal=false
Categories=AudioVideo;Audio;Player;
Keywords=music;Navidrome;Subsonic;
StartupWMClass=app.sonora.player
EOF
if command -v desktop-file-validate >/dev/null; then
  desktop-file-validate "$staging_dir/app.sonora.player.desktop"
fi
# Rename instead of overwriting an executable that may still be running.
mv -f "$staging_dir/sonora" "$bin_dir/sonora"
install -m 644 "$staging_dir/app.sonora.player.desktop" "$desktop_dir/app.sonora.player.desktop"
if command -v update-desktop-database >/dev/null; then
  update-desktop-database "$desktop_dir"
fi
if command -v gtk-update-icon-cache >/dev/null; then
  gtk-update-icon-cache --force --ignore-theme-index "$data_home/icons/hicolor" >/dev/null
fi
printf 'Installed %s\nFind Sonora in your application launcher.\n' "$bin_dir/sonora"
printf 'To update after changing this checkout, rerun: %s/scripts/install-linux.sh\n' "$root"
