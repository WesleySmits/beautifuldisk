#!/bin/sh
# Installs Beautiful Disk on Linux (x86_64) for the current user:
#   curl -fsSL https://raw.githubusercontent.com/vkpdeveloper/beautifuldisk/main/install.sh | sh
set -eu

case "$(uname -s)-$(uname -m)" in
  Linux-x86_64) ;;
  *) echo "This installer is for x86_64 Linux. On a Mac, use the DMG from the latest release." >&2; exit 1 ;;
esac

url="https://github.com/vkpdeveloper/beautifuldisk/releases/latest/download/beautiful-disk-linux-x86_64.tar.gz"
bin="$HOME/.local/bin"
share="${XDG_DATA_HOME:-$HOME/.local/share}"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

echo "Downloading Beautiful Disk..."
curl -fsSL "$url" | tar xz -C "$tmp"

mkdir -p "$bin" "$share/applications" "$share/beautifuldisk"
install -m 755 "$tmp/beautifuldisk" "$bin/beautifuldisk"
install -m 644 "$tmp/beautifuldisk.png" "$share/beautifuldisk/icon.png"
cat > "$share/applications/beautifuldisk.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=Beautiful Disk
Comment=See what fills your disk, then clear what your tools left behind
Exec=$bin/beautifuldisk
Icon=$share/beautifuldisk/icon.png
Categories=Utility;System;
EOF

echo "Installed to $bin/beautifuldisk. Open Beautiful Disk from your app menu, or run: beautifuldisk"
case ":$PATH:" in
  *":$bin:"*) ;;
  *) echo "Add $bin to your PATH to run it from a terminal." ;;
esac
