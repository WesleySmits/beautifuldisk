import React from "react";
import {AbsoluteFill, Easing, OffthreadVideo, interpolate, staticFile, useCurrentFrame} from "remotion";
import {DitherText, DotField, FPS, Run, clamp, hash} from "../dither";

export type Caption = {from: number; to: number; lines: Run[][]};

/**
 * The real app, recorded in autoplay, with captions beside it. `from` is
 * where this scene starts in the recording; every other time is scene time.
 * `focus` keys the zoom origin's height (percent) over time, and `punch`
 * is when the camera hits and shakes.
 */
export const AppScene: React.FC<{
  from: number;
  captions: Caption[];
  focus: {at: number[]; y: number[]};
  punch?: number;
}> = ({from, captions, focus, punch}) => {
  const frame = useCurrentFrame();
  const t = frame / FPS;

  const push = interpolate(t, [0, 1.2, 10], [1, 1.07, 1.1], {
    extrapolateRight: "clamp",
    easing: Easing.inOut(Easing.cubic),
  });
  const originY = interpolate(t, focus.at, focus.y, {
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
    easing: Easing.inOut(Easing.cubic),
  });
  const since = punch === undefined ? -1 : t - punch;
  const hit = since > 0 ? 0.035 * Math.exp(-since * 3) * Math.min(1, since * 12) : 0;
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
          transform: `translate(${sx}px, ${sy}px) scale(${push + hit})`,
          transformOrigin: `30% ${originY}%`,
        }}
      >
        <OffthreadVideo
          src={staticFile("app.mp4")}
          trimBefore={Math.round(from * FPS)}
          muted
          style={{width: "100%", height: "100%"}}
        />
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
