import React from "react";
import {AbsoluteFill, Easing, Img, interpolate, spring, staticFile, useCurrentFrame} from "remotion";
import {DitherText, DotField, Dust, FPS, MONO, TONES, clamp} from "../dither";

const W = 1920;
const FG = "#ededef";
const MUTED = "#8b8b90";
const FREED = 107.1;

/** 10 to 15s: the payoff count, then the name. */
export const Outro: React.FC = () => {
  const frame = useCurrentFrame();
  const t = frame / FPS;

  const count = interpolate(t, [0.1, 0.95], [0, FREED], {
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
    easing: Easing.out(Easing.cubic),
  });
  const countOut = 1 - clamp((t - 2.0) / 0.2);
  const logo = spring({frame: frame - Math.round(2.15 * FPS), fps: FPS, config: {damping: 13, stiffness: 140}});
  const name = clamp((t - 2.3) / 0.45);
  const tagline = clamp((t - 2.75) / 0.3);

  return (
    <AbsoluteFill style={{background: "#000"}}>
      <DotField width={W} height={1080} />
      <Dust width={W} height={1080} box={[560, 470, 1360, 640]} t={t} />
      {countOut > 0 && (
        <div style={{position: "absolute", top: 330, left: 0, width: W}}>
          <DitherText
            lines={[[{text: `${count.toFixed(1)} GB`, color: FG}]]}
            size={230}
            weight={700}
            font={MONO}
            width={W}
            align="center"
            cell={5}
            density={countOut}
          />
          <DitherText
            lines={[[{text: "freed", color: TONES[1]}]]}
            size={72}
            width={W}
            align="center"
            reveal={clamp((t - 0.35) / 0.35)}
            density={countOut}
            style={{marginTop: 6}}
          />
        </div>
      )}
      {t > 2.1 && (
        <AbsoluteFill style={{alignItems: "center", justifyContent: "center", flexDirection: "column"}}>
          <Img
            src={staticFile("icon.png")}
            style={{
              width: 250,
              height: 250,
              imageRendering: "pixelated",
              transform: `scale(${logo}) translateY(${(1 - logo) * 40}px)`,
              opacity: clamp(logo * 1.5),
            }}
          />
          <DitherText
            lines={[[{text: "Beautiful Disk", color: FG}]]}
            size={112}
            width={W}
            align="center"
            reveal={name}
            style={{marginTop: 34}}
          />
          <div
            style={{
              marginTop: 8,
              color: MUTED,
              fontFamily: "-apple-system, sans-serif",
              fontSize: 44,
              opacity: tagline,
            }}
          >
            For macOS
          </div>
        </AbsoluteFill>
      )}
    </AbsoluteFill>
  );
};
