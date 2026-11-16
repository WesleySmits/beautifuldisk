import React from "react";
import {AbsoluteFill, Html5Audio, Sequence, interpolate, staticFile, useCurrentFrame} from "remotion";
import {DitherWipe, FPS} from "./dither";
import {AppScene} from "./scenes/AppScene";
import {Intro} from "./scenes/Intro";
import {Outro} from "./scenes/Outro";

/** Dither cover in and out around each cut, peaking on the cut itself. */
const wipe = (frame: number, cut: number) =>
  interpolate(frame, [cut - 8, cut, cut + 14], [0, 1, 0], {
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
  });

export const Launch: React.FC = () => {
  const frame = useCurrentFrame();
  return (
    <AbsoluteFill style={{background: "#000"}}>
      <Sequence durationInFrames={2 * FPS} name="Intro">
        <Intro />
      </Sequence>
      <Sequence from={2 * FPS} durationInFrames={8 * FPS} name="App">
        <AppScene />
      </Sequence>
      <Sequence from={10 * FPS} durationInFrames={5 * FPS} name="Outro">
        <Outro />
      </Sequence>
      <DitherWipe width={1920} height={1080} amount={Math.max(wipe(frame, 2 * FPS), wipe(frame, 10 * FPS))} />
      <Html5Audio src={staticFile("soundtrack.wav")} />
    </AbsoluteFill>
  );
};
