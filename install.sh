#!/bin/sh
# Installs Beautiful Disk and opens it:
#   curl -fsSL https://raw.githubusercontent.com/vkpdeveloper/beautifuldisk/main/install.sh | sh
# macOS (Apple silicon): the app from the latest DMG, in /Applications.
# Linux (x86_64): the binary in ~/.local/bin, with an app menu entry and icon.
# Running it again updates in place and reopens the app.
set -eu

release="https://github.com/vkpdeveloper/beautifuldisk/releases/latest/download"
tmp="$(mktemp -d)"
mount=""
cleanup() {
  [ -n "$mount" ] && hdiutil detach -quiet "$mount" 2>/dev/null || true
  rm -rf "$tmp"
}
trap cleanup EXIT

# Stops a running copy so the new one replaces it, waiting up to 5 seconds
# for it to quit on its own first.
stop_running() {
  pgrep -x beautifuldisk >/dev/null 2>&1 || return 0
  [ "$(uname -s)" = Darwin ] &&
    osascript -e 'quit app id "com.vkpdeveloper.beautifuldisk"' >/dev/null 2>&1 || true
  for _ in 1 2 3 4 5 6 7 8 9 10; do
    pgrep -x beautifuldisk >/dev/null 2>&1 || return 0
    sleep 0.5
  done
  pkill -x beautifuldisk 2>/dev/null || true
  sleep 0.5
}

install_mac() {
  echo "Downloading Beautiful Disk..."
  curl -fsSL -o "$tmp/Beautiful-Disk.dmg" "$release/Beautiful-Disk.dmg"
  mount="$(hdiutil attach -nobrowse -noautoopen -readonly "$tmp/Beautiful-Disk.dmg" |
    awk -F'\t' '/\/Volumes\//{print $NF}')"
  [ -d "$mount/Beautiful Disk.app" ] || { echo "The download didn't hold the app." >&2; exit 1; }

  dest=/Applications
  [ -w "$dest" ] || { dest="$HOME/Applications"; mkdir -p "$dest"; }
  app="$dest/Beautiful Disk.app"
  stop_running
  rm -rf "$app"
  ditto "$mount/Beautiful Disk.app" "$app"
  hdiutil detach -quiet "$mount" && mount=""
  # curl doesn't mark downloads, but a copy dragged in before may be.
  xattr -dr com.apple.quarantine "$app" 2>/dev/null || true

  echo "Installed in $dest. Opening Beautiful Disk..."
  open "$app"
}

install_linux() {
  bin="$HOME/.local/bin"
  share="${XDG_DATA_HOME:-$HOME/.local/share}"

  echo "Downloading Beautiful Disk..."
  curl -fsSL "$release/beautiful-disk-linux-x86_64.tar.gz" | tar xz -C "$tmp"

  mkdir -p "$bin" "$share/applications"
  stop_running
  # A new file renamed over the old one, so a copy still running can't block it.
  install -m 755 "$tmp/beautifuldisk" "$bin/.beautifuldisk.new"
  mv -f "$bin/.beautifuldisk.new" "$bin/beautifuldisk"
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

  if command -v ldconfig >/dev/null 2>&1 && ! ldconfig -p | grep -q libvulkan.so.1; then
    echo "Beautiful Disk draws with Vulkan. Install your distro's Vulkan driver package if it fails to open."
  fi
  case ":$PATH:" in
    *":$bin:"*) ;;
    *) echo "Add $bin to your PATH to run it from a terminal." ;;
  esac

  if [ -n "${DISPLAY:-}${WAYLAND_DISPLAY:-}" ]; then
    echo "Installed. Opening Beautiful Disk..."
    # Its own session, so it keeps running after this script and terminal end.
    if command -v setsid >/dev/null 2>&1; then
      setsid "$bin/beautifuldisk" </dev/null >/dev/null 2>&1 &
    else
      nohup "$bin/beautifuldisk" </dev/null >/dev/null 2>&1 &
    fi
  else
    echo "Installed. Open Beautiful Disk from your app menu, or run: beautifuldisk"
  fi
}

case "$(uname -s)-$(uname -m)" in
  Darwin-arm64) install_mac ;;
  Linux-x86_64) install_linux ;;
  Darwin-*) echo "Beautiful Disk needs a Mac with Apple silicon." >&2; exit 1 ;;
  *) echo "Beautiful Disk runs on Apple silicon Macs and x86_64 Linux." >&2; exit 1 ;;
esac
