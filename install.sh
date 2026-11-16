#!/bin/sh
# Installs Beautiful Disk on Linux (x86_64) for the current user, with an
# app menu entry and icon:
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

mkdir -p "$bin" "$share/applications"
install -m 755 "$tmp/beautifuldisk" "$bin/beautifuldisk"
for size in 48 64 128 256 512; do
  mkdir -p "$share/icons/hicolor/${size}x${size}/apps"
  install -m 644 "$tmp/icons/$size.png" "$share/icons/hicolor/${size}x${size}/apps/beautifuldisk.png"
done

# StartupWMClass matches the window's app id, so docks show this icon.
cat > "$share/applications/beautifuldisk.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=Beautiful Disk
Comment=See what fills your disk, then clear what your tools left behind
Exec=$bin/beautifuldisk
Icon=beautifuldisk
Terminal=false
Categories=System;Filesystem;
Keywords=disk;storage;cache;cleaner;space;
StartupWMClass=beautifuldisk
EOF

# Desktops pick up new entries on their own; these just make it immediate.
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
  gtk-update-icon-cache -q -t "$share/icons/hicolor" 2>/dev/null || true
fi
if command -v update-desktop-database >/dev/null 2>&1; then
  update-desktop-database -q "$share/applications" 2>/dev/null || true
fi

echo "Installed. Open Beautiful Disk from your app menu, or run: beautifuldisk"
case ":$PATH:" in
  *":$bin:"*) ;;
  *) echo "Add $bin to your PATH to run it from a terminal." ;;
esac
if command -v ldconfig >/dev/null 2>&1 && ! ldconfig -p | grep -q libvulkan.so.1; then
  echo "Beautiful Disk draws with Vulkan. Install your distro's Vulkan driver package if it fails to open."
fi
