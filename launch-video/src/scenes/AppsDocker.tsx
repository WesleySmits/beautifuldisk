import React from "react";
import {AbsoluteFill, Easing, interpolate, useCurrentFrame} from "remotion";
import {DitherBar, DitherText, DotField, FPS, TONES, clamp} from "../dither";

const W = 1920;
const MUTED = "#8b8b90";

export const SIDES_AT = [0.2, 1.7];
export const DRAIN_AT = [0.9, 2.4];

const sides = [
  {title: "Removes apps", rest: "and what they leave behind.", tone: TONES[0]},
  {title: "Cleans Docker", rest: "and never touches volumes.", tone: TONES[4]},
];

/** The two tools beside the scan: apps and Docker. */
export const AppsDocker: React.FC = () => {
  const t = useCurrentFrame() / FPS;
  const out = 1 - clamp((t - 5.2) / 0.3);
  return (
    <AbsoluteFill style={{background: "#000"}}>
      <DotField width={W} height={1080} />
      {sides.map((s, i) => {
        const at = SIDES_AT[i];
        if (t < at) return null;
        const fill = interpolate(t, [DRAIN_AT[i], DRAIN_AT[i] + 1.1], [0.82, 0.22], {
          extrapolateLeft: "clamp",
          extrapolateRight: "clamp",
          easing: Easing.inOut(Easing.cubic),
        });
        const left = 140 + i * 860;
        return (
          <div key={s.title} style={{position: "absolute", left, top: 330, opacity: out}}>
            <DitherText lines={[[{text: s.title, color: s.tone}]]} size={92} width={780} reveal={clamp((t - at) / 0.45)} />
            <DitherText
              lines={[[{text: s.rest, color: MUTED}]]}
              size={52}
              width={780}
              reveal={clamp((t - at - 0.25) / 0.45)}
              style={{marginTop: 4}}
            />
            <div style={{marginTop: 56}}>
              <DitherBar width={760} height={56} fill={fill} time={t} />
            </div>
          </div>
        );
      })}
    </AbsoluteFill>
  );
};
