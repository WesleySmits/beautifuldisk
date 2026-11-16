#!/bin/sh
# Builds "Beautiful Disk.app" so macOS (menu bar, Dock, app switcher) shows
# the real name instead of the binary's. Pass --open to launch it.
set -e
cd "$(dirname "$0")"
cargo build --release
app="target/release/Beautiful Disk.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS"
cp Info.plist "$app/Contents/"
cp target/release/beautifuldisk "$app/Contents/MacOS/"
echo "$app"
if [ "$1" = "--open" ]; then
    open "$app"
fi
