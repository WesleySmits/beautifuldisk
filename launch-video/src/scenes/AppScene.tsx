import React from "react";
import {AbsoluteFill, Easing, OffthreadVideo, interpolate, staticFile, useCurrentFrame} from "remotion";
import {DitherText, DotField, FPS, Run, TONES, clamp, hash} from "../dither";

const FG = "#ededef";

// Seconds into the recording (this scene's own time) when things happen.
const ITEMS_IN = 3.05;
const CLEAR = 6.47;

const captions: {from: number; to: number; lines: Run[][]}[] = [
  {
    from: 0.3,
    to: 2.95,
    lines: [[{text: "Scans your Mac", color: FG}], [{text: "in ", color: FG}, {text: "seconds.", color: TONES[0]}]],
  },
  {
    from: 3.1,
    to: 6.35,
    lines: [
      [{text: "Finds what your", color: FG}],
      [{text: "agents", color: TONES[2]}, {text: " left behind.", color: FG}],
    ],
  },
  {
    from: 6.45,
    to: 8.2,
    lines: [[{text: "Clears it all", color: FG}], [{text: "at once.", color: TONES[1]}]],
  },
];

/** 2 to 10s: the real app, recorded in autoplay, with captions beside it. */
export const AppScene: React.FC = () => {
  const frame = useCurrentFrame();
  const t = frame / FPS;

  // Zoom toward the usage bar while scanning, then glide down to the
  // treemap once the items arrive.
  const push = interpolate(t, [0, 1.2, 8], [1, 1.07, 1.09], {
    extrapolateRight: "clamp",
    easing: Easing.inOut(Easing.cubic),
  });
  const focus = interpolate(t, [ITEMS_IN - 0.2, ITEMS_IN + 0.9], [6, 80], {
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
    easing: Easing.inOut(Easing.cubic),
  });
  // A punch with a short shake when everything clears.
  const since = t - CLEAR;
  const punch = since > 0 ? 0.035 * Math.exp(-since * 3) * Math.min(1, since * 12) : 0;
  const shake = since > 0 && since < 0.25 ? (1 - since / 0.25) * 4 : 0;
  const sx = (hash(frame, 1) - 0.5) * 2 * shake;
  const sy = (hash(frame, 2) - 0.5) * 2 * shake;

  return (
    <AbsoluteFill style={{background: "#000"}}>
      <DotField width={1920} height={1080} />
      <div
        style={{
          position: "absolute",
          left: 800,
          top: 70,
          width: 1016,
          height: 960,
          transform: `translate(${sx}px, ${sy}px) scale(${push + punch})`,
          transformOrigin: `30% ${focus}%`,
        }}
      >
        <OffthreadVideo src={staticFile("app.mp4")} muted style={{width: "100%", height: "100%"}} />
      </div>
      {captions.map((c) => {
        if (t < c.from || t > c.to) return null;
        const reveal = clamp((t - c.from) / 0.45);
        const density = 1 - clamp((t - (c.to - 0.18)) / 0.18);
        return (
          <DitherText
            key={c.from}
            lines={c.lines}
            size={76}
            width={720}
            reveal={reveal}
            density={density}
            style={{position: "absolute", left: 96, top: 420}}
          />
        );
      })}
    </AbsoluteFill>
  );
};
