# Launch video

The 39 second launch video, built with Remotion from a recording of the app.

```sh
bun install
bun scripts/soundtrack.ts                       # public/soundtrack.wav
npx remotion studio                             # preview
npx remotion render Launch out/launch.mp4       # the video
npx remotion still Banner ../assets/readme/banner.png
```

`public/app.mp4` is the app window recorded with `beautifuldisk --autoplay`;
`scripts/app-sounds.bin` is the sound log from the same run
(`BEAUTIFULDISK_SOUND_LOG`).
