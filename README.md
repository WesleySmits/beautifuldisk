<p align="center">
  <img src="assets/readme/banner.png" alt="Beautiful Disk" width="800">
</p>

<p align="center">See what fills your disk, then clear what your tools left behind. For macOS and Linux.</p>

<p align="center">
  <a href="https://github.com/vkpdeveloper/beautifuldisk/releases/latest/download/beautiful-disk-launch.mp4">
    <img src="assets/readme/launch.gif" alt="Beautiful Disk launch video" width="800">
  </a>
  <br>
  <sub>Click for the video with sound.</sub>
</p>

## What it does

- Scans your disk in seconds and shows it as one dithered bar: apps, developer files, photos, movies, documents, music, caches, trash and system data.
- Finds clutter that is safe to clear and lays it out as a treemap, sized by bytes:
  - `node_modules`, Rust build output, Xcode build data, archives and device support
  - iOS simulators and Android emulators (running ones are kept)
  - Agent worktrees from T3 Code, Codex, Cursor and Conductor (only ones without changes)
  - Docker build cache and unused images (volumes and containers are never touched)
  - Package caches: Gradle, Maven, Yarn, pnpm, bun, npm, Go, Cargo, uv, pip, Deno, Composer, Dart, Homebrew, CocoaPods
  - Build caches: `.next`, `.turbo`, Bazel, ccache, sccache, JetBrains, VS Code, Python `.venv` folders
  - Test browsers (Playwright, Cypress, Puppeteer), browser caches, downloaded models (Hugging Face, Ollama, LM Studio, PyTorch), old installers and the Trash
- Clears everything you picked at the same time, with quiet ticks and a whoosh when it is gone.
- Finds files you have more than one copy of, and screenshots older than three months, in Pictures, Documents, Movies, Desktop and Downloads. Open one to see its copies side by side and pick which to keep. They go to the Trash, with Put back.

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/vkpdeveloper/beautifuldisk/main/install.sh | sh
```

Works on Macs with Apple silicon and on x86_64 Linux, then opens the app. Run it again to update.

- **macOS:** installs `Beautiful Disk.app` in `/Applications`. You can also download `Beautiful-Disk.dmg` from the [latest release](https://github.com/vkpdeveloper/beautifuldisk/releases/latest) and drag the app to Applications. The app is not notarized yet, so a copy from the DMG opens the first time with right-click, Open.
- **Linux:** installs to `~/.local/bin` with an app menu entry. Needs X11 or Wayland and Vulkan drivers.

## Build

```sh
cargo run --release            # run it
cargo run --release -- --demo  # sample data, deletes nothing
cargo run --release -- --no-sound  # silent: never opens an audio output
./bundle.sh --dmg              # Beautiful Disk.app and the DMG in target/release
./bundle-linux.sh              # on Linux: the release tarball in target/release
```

The sound effects open the audio output only while they play and release it two seconds later. To keep Beautiful Disk off the audio device entirely, start it with `--no-sound`; for the installed app: `open -a "Beautiful Disk" --args --no-sound`.

The launch video is made with [Remotion](https://www.remotion.dev) in [`launch-video/`](launch-video), from a recording of the app in `--autoplay` mode.
