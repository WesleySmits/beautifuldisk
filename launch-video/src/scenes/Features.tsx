import React from "react";
import {AbsoluteFill, useCurrentFrame} from "remotion";
import {DitherText, DotField, FPS, TONES, clamp} from "../dither";

const W = 1920;
const FG = "#ededef";
const MUTED = "#8b8b90";

export const ROW_AT = 0.7;
export const ROW_GAP = 0.42;

const rows = [
  {name: "Simulators", rest: " and emulators", tone: TONES[0]},
  {name: "Agent worktrees", rest: " from T3 Code, Codex, Cursor", tone: TONES[2]},
  {name: "node_modules", rest: " and build output", tone: TONES[1]},
  {name: "Package caches", rest: " from npm to Cargo", tone: TONES[5]},
  {name: "Docker", rest: " build cache and images", tone: TONES[4]},
  {name: "Test browsers", rest: " and AI models", tone: TONES[3]},
];

/** What it finds, one row at a time. */
export const Features: React.FC = () => {
  const t = useCurrentFrame() / FPS;
  const out = 1 - clamp((t - 5.6) / 0.3);
  return (
    <AbsoluteFill style={{background: "#000"}}>
      <DotField width={W} height={1080} />
      <DitherText
        lines={[[{text: "Knows where to look.", color: FG}]]}
        size={96}
        width={W}
        align="center"
        reveal={clamp((t - 0.05) / 0.45)}
        density={out}
        style={{position: "absolute", top: 120, left: 0}}
      />
      {rows.map((r, i) => {
        const at = ROW_AT + i * ROW_GAP;
        if (t < at) return null;
        return (
          <DitherText
            key={r.name}
            lines={[
              [
                {text: r.name, color: r.tone},
                {text: r.rest, color: MUTED},
              ],
            ]}
            size={64}
            width={1560}
            reveal={clamp((t - at) / 0.4)}
            density={out}
            style={{position: "absolute", top: 320 + i * 104, left: 260}}
          />
        );
      })}
    </AbsoluteFill>
  );
};
