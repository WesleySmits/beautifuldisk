#!/bin/sh
# Builds "Beautiful Disk.app" so macOS (menu bar, Dock, app switcher) shows
# the real name and icon instead of the binary's.
#   --open  launch it
#   --dmg   also build "Beautiful Disk.dmg" with the drag-to-install window
set -e
cd "$(dirname "$0")"
cargo build --release
out=target/release
app="$out/Beautiful Disk.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp Info.plist "$app/Contents/"
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
/usr/libexec/PlistBuddy -c "Set :CFBundleShortVersionString $version" \
    -c "Set :CFBundleVersion $version" "$app/Contents/Info.plist"
cp "$out/beautifuldisk" "$app/Contents/MacOS/"

# App icon: every size macOS asks for, cut from the 1024px master.
set_dir="$out/AppIcon.iconset"
rm -rf "$set_dir" && mkdir "$set_dir"
for s in 16 32 128 256 512; do
    sips -z $s $s assets/icon/icon-1024.png --out "$set_dir/icon_${s}x${s}.png" >/dev/null
    d=$((s * 2))
    sips -z $d $d assets/icon/icon-1024.png --out "$set_dir/icon_${s}x${s}@2x.png" >/dev/null
done
iconutil -c icns "$set_dir" -o "$app/Contents/Resources/AppIcon.icns"
rm -rf "$set_dir"
# Sparkle, which installs updates. Downloaded once and checked against the
# hash of the pinned release.
sparkle_version=2.10.1
sparkle_sha256=1a856d2a9edb8e628de88121d7be04cf0e2167f8641650b78fca00b14244cf27
sparkle="$out/sparkle-$sparkle_version"
if [ ! -d "$sparkle/Sparkle.framework" ]; then
    mkdir -p "$sparkle"
    curl -sSfL -o "$sparkle.tar.xz" \
        "https://github.com/sparkle-project/Sparkle/releases/download/$sparkle_version/Sparkle-$sparkle_version.tar.xz"
    echo "$sparkle_sha256  $sparkle.tar.xz" | shasum -a 256 -c - >/dev/null
    tar xf "$sparkle.tar.xz" -C "$sparkle"
    rm "$sparkle.tar.xz"
fi
mkdir -p "$app/Contents/Frameworks"
ditto "$sparkle/Sparkle.framework" "$app/Contents/Frameworks/Sparkle.framework"

# Sign with the same certificate every time when it is in the keychain.
# macOS ties Full Disk Access to the signature, so a throwaway one (-) makes
# every update ask for access again.
identity="Beautiful Disk Signing"
security find-identity -p codesigning | grep -q "\"$identity\"" || identity=-
# Seal the whole bundle. The linker signs only the binary, and a downloaded
# app whose Info.plist and icon aren't covered by its signature opens as
# "damaged" with no way past it.
codesign --force --deep --sign "$identity" "$app"
codesign --verify --deep --strict "$app"
echo "$app"

if [ "$1" = "--dmg" ]; then
    name="Beautiful Disk"
    dmg="$out/$name.dmg"
    rw="$out/$name-rw.dmg"
    stage="$out/dmg-stage"
    rm -rf "$stage" "$rw" "$dmg"
    mkdir -p "$stage/.background"
    cp -R "$app" "$stage/"
    ln -s /Applications "$stage/Applications"
    # 1x and 2x backgrounds in one file, so Retina screens get the sharp one.
    tiffutil -cathidpicheck assets/icon/dmg-background-small.png assets/icon/dmg-background.png \
        -out "$stage/.background/background.tiff" >/dev/null
    hdiutil create -volname "$name" -srcfolder "$stage" -fs HFS+ -format UDRW -ov "$rw" >/dev/null
    rm -rf "$stage"
    mount=$(hdiutil attach -nobrowse -noautoopen "$rw" | awk -F'\t' '/Volumes/{print $NF}')
    # Finder lays out the window: size, background, icon spots. Matches the
    # 660x400 background, with the app at (165,200) and Applications at (495,200).
    osascript <<EOF
tell application "Finder"
    tell disk "$(basename "$mount")"
        open
        set current view of container window to icon view
        set toolbar visible of container window to false
        set statusbar visible of container window to false
        set bounds of container window to {200, 120, 860, 520}
        set opts to icon view options of container window
        set arrangement of opts to not arranged
        set icon size of opts to 128
        set text size of opts to 13
        set background picture of opts to file ".background:background.tiff"
        set position of item "$name.app" of container window to {165, 200}
        set position of item "Applications" of container window to {495, 200}
        update without registering applications
        delay 1
        close
    end tell
end tell
EOF
    sync
    hdiutil detach "$mount" -quiet
    hdiutil convert "$rw" -format UDZO -imagekey zlib-level=9 -o "$dmg" >/dev/null
    rm -f "$rw"
    echo "$dmg"
fi

if [ "$1" = "--open" ]; then
    open "$app"
fi
