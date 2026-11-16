import React from "react";
import {AbsoluteFill, Easing, interpolate, useCurrentFrame} from "remotion";
import {DitherBar, DitherText, DotField, FPS, clamp} from "../dither";

const W = 1920;
const FG = "#ededef";
const PINK = "#ff4f8b";

/** 0 to 2.2s: the hook. A usage bar fills up while the line lands. */
export const Intro: React.FC = () => {
  const frame = useCurrentFrame();
  const t = frame / FPS;
  const fill = interpolate(t, [0.25, 1.95], [0.02, 0.97], {
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
    easing: Easing.in(Easing.quad),
  });
  const first = clamp((t - 0.1) / 0.45);
  const firstOut = 1 - clamp((t - 1.0) / 0.12);
  const second = clamp((t - 1.08) / 0.45);
  return (
    <AbsoluteFill style={{background: "#000"}}>
      <DotField width={W} height={1080} />
      <div style={{position: "absolute", top: 330, left: 0, width: W}}>
        {firstOut > 0 && (
          <DitherText
            lines={[[{text: "Agents write code all day.", color: FG}]]}
            size={118}
            width={W}
            align="center"
            reveal={first}
            density={firstOut}
            style={{position: "absolute", top: 0}}
          />
        )}
        {second > 0 && (
          <DitherText
            lines={[[{text: "Your disk keeps the ", color: FG}, {text: "mess.", color: PINK}]]}
            size={118}
            width={W}
            align="center"
            reveal={second}
            style={{position: "absolute", top: 0}}
          />
        )}
      </div>
      <div style={{position: "absolute", top: 600, left: (W - 1260) / 2}}>
        <DitherBar width={1260} height={63} fill={fill} time={t} hot={clamp((fill - 0.85) / 0.12)} />
      </div>
    </AbsoluteFill>
  );
};
