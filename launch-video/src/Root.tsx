import React from "react";
import {Composition} from "remotion";
import {Banner} from "./Banner";
import {FPS} from "./dither";
import {LENGTH, Launch} from "./Launch";

export const RemotionRoot: React.FC = () => (
  <>
    <Composition id="Launch" component={Launch} durationInFrames={LENGTH * FPS} fps={FPS} width={1920} height={1080} />
    <Composition id="Banner" component={Banner} durationInFrames={1} fps={60} width={1600} height={480} />
  </>
);
