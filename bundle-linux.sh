#!/bin/sh
# Builds the Linux release tarball that install.sh and the in-app updater
# download: the binary at the top (the updater looks for it there) and the
# app menu icons.
#   ./bundle-linux.sh   ->  target/release/beautiful-disk-linux-x86_64.tar.gz
set -e
cd "$(dirname "$0")"
cargo build --release
out=target/release
stage="$out/linux-stage"
rm -rf "$stage"
mkdir -p "$stage"
cp "$out/beautifuldisk" "$stage/"
cp -R assets/icon/linux "$stage/icons"
tar czf "$out/beautiful-disk-linux-x86_64.tar.gz" -C "$stage" beautifuldisk icons
rm -rf "$stage"
echo "$out/beautiful-disk-linux-x86_64.tar.gz"
