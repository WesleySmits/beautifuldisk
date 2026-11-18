import React from "react";
import {AbsoluteFill, Html5Audio, Sequence, interpolate, staticFile, useCurrentFrame} from "remotion";
import {DitherWipe, FPS, TONES} from "./dither";
import {AppScene, Caption} from "./scenes/AppScene";
import {AppsDocker} from "./scenes/AppsDocker";
import {Features} from "./scenes/Features";
import {Intro} from "./scenes/Intro";
import {Outro} from "./scenes/Outro";

const FG = "#ededef";

// Scene starts, in seconds. scripts/soundtrack.ts uses the same cuts.
const CUTS = {app: 2, features: 10, files: 16, tools: 26.5, outro: 32, end: 39};
export const LENGTH = CUTS.end;

// The recording: scanning and clearing from 1.2s, the "Your files" tour from 9.3s.
const SCAN_FROM = 1.2;
const FILES_FROM = 9.3;

const scanCaptions: Caption[] = [
  {from: 0.3, to: 2.65, lines: [[{text: "Scans your disk", color: FG}], [{text: "in ", color: FG}, {text: "seconds.", color: TONES[0]}]]},
  {
    from: 2.8,
    to: 6.45,
    lines: [[{text: "Finds what your", color: FG}], [{text: "agents", color: TONES[2]}, {text: " left behind.", color: FG}]],
  },
  {from: 6.6, to: 7.95, lines: [[{text: "Clears it all", color: FG}], [{text: "at once.", color: TONES[1]}]]},
];

const filesCaptions: Caption[] = [
  {from: 0.2, to: 2.1, lines: [[{text: "Finds ", color: FG}, {text: "copies", color: TONES[3]}], [{text: "of your files.", color: FG}]]},
  {from: 2.25, to: 3.95, lines: [[{text: "See them", color: FG}], [{text: "side by side.", color: TONES[4]}]]},
  {from: 4.05, to: 6.3, lines: [[{text: "Keep the one", color: FG}], [{text: "you want.", color: TONES[1]}]]},
  {from: 6.45, to: 7.9, lines: [[{text: "Old ", color: FG}, {text: "screenshots", color: TONES[5]}], [{text: "go too.", color: FG}]]},
  {from: 8.05, to: 10.4, lines: [[{text: "Into the Trash.", color: FG}], [{text: "Put back", color: TONES[0]}, {text: " anytime.", color: FG}]]},
];

/** Dither cover in and out around each cut, peaking on the cut itself. */
const wipe = (frame: number, cut: number) =>
  interpolate(frame, [cut - 8, cut, cut + 14], [0, 1, 0], {
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
  });

const span = (from: number, to: number) => ({from: from * FPS, durationInFrames: (to - from) * FPS});

export const Launch: React.FC = () => {
  const frame = useCurrentFrame();
  const cuts = [CUTS.app, CUTS.features, CUTS.files, CUTS.tools, CUTS.outro];
  return (
    <AbsoluteFill style={{background: "#000"}}>
      <Sequence {...span(0, CUTS.app)} name="Intro">
        <Intro />
      </Sequence>
      <Sequence {...span(CUTS.app, CUTS.features)} name="Scan">
        <AppScene from={SCAN_FROM} captions={scanCaptions} focus={{at: [2.6, 3.7], y: [6, 80]}} punch={6.6} />
      </Sequence>
      <Sequence {...span(CUTS.features, CUTS.files)} name="Features">
        <Features />
      </Sequence>
      <Sequence {...span(CUTS.files, CUTS.tools)} name="Files">
        <AppScene from={FILES_FROM} captions={filesCaptions} focus={{at: [0, 1], y: [40, 85]}} punch={8.05} />
      </Sequence>
      <Sequence {...span(CUTS.tools, CUTS.outro)} name="Apps and Docker">
        <AppsDocker />
      </Sequence>
      <Sequence {...span(CUTS.outro, CUTS.end)} name="Outro">
        <Outro />
      </Sequence>
      <DitherWipe width={1920} height={1080} amount={Math.max(...cuts.map((c) => wipe(frame, c * FPS)))} />
      <Html5Audio src={staticFile("soundtrack.wav")} />
    </AbsoluteFill>
  );
};
