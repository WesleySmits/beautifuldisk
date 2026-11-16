import React from "react";
import {AbsoluteFill, Img, staticFile} from "remotion";
import {DitherBar, DitherText, DotField} from "./dither";

const W = 1600;
const H = 480;

/** The README header: icon, name and a usage bar, all dots. */
export const Banner: React.FC = () => (
  <AbsoluteFill style={{background: "#000"}}>
    <DotField width={W} height={H} alpha={0.07} />
    <Img
      src={staticFile("icon.png")}
      style={{position: "absolute", left: 120, top: 110, width: 260, height: 260, imageRendering: "pixelated"}}
    />
    <DitherText
      lines={[[{text: "Beautiful Disk", color: "#ededef"}]]}
      size={128}
      width={1060}
      style={{position: "absolute", left: 440, top: 118}}
    />
    <div style={{position: "absolute", left: 444, top: 300}}>
      <DitherBar width={1030} height={56} fill={0.72} time={0} />
    </div>
  </AbsoluteFill>
);
