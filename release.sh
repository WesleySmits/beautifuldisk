#!/bin/sh
# Publishes a GitHub release that installed copies update to. Bump the
# version in Cargo.toml first.
#   ./release.sh [beautiful-disk-linux-x86_64.tar.gz]
# The Mac app updates through Sparkle: appcast.xml lists the DMG, signed with
# the Sparkle key in the login keychain (account "beautifuldisk"). Linux
# copies download the tarball, which holds the beautifuldisk binary.
set -e
cd "$(dirname "$0")"
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
tag="v$version"

./bundle.sh --dmg
feed=target/release/appcast
rm -rf "$feed"
mkdir -p "$feed"
cp "target/release/Beautiful Disk.dmg" "$feed/Beautiful-Disk.dmg"
target/release/sparkle-*/bin/generate_appcast --account beautifuldisk \
    --download-url-prefix "https://github.com/vkpdeveloper/beautifuldisk/releases/download/$tag/" \
    "$feed"

gh release create "$tag" "$feed/Beautiful-Disk.dmg" "$feed/appcast.xml" ${1:+"$1"} \
    --title "Beautiful Disk $version" --generate-notes
