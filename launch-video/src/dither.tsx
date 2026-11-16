// Dither drawing shared by every scene: text, bars and wipes are all dots on
// one grid, the same way the app paints its bar and treemap.
import React, {useLayoutEffect, useMemo, useRef} from "react";

export const FPS = 60;
export const SANS = '-apple-system, "SF Pro Display", "Helvetica Neue", sans-serif';
export const MONO = 'Menlo, "SF Mono", monospace';

// The app's tones, as on the icon's color strip.
export const TONES = ["#4a8cff", "#2fbf71", "#ff4f8b", "#9b6bff", "#22c3b5", "#ff8a3d"];

const BAYER = [0, 8, 2, 10, 12, 4, 14, 6, 3, 11, 1, 9, 15, 7, 13, 5];
export const bayer = (col: number, row: number) =>
  (BAYER[(row & 3) * 4 + (col & 3)] + 0.5) / 16;

export const hash = (x: number, y: number) => {
  let h = Math.imul(x, 374761393) + Math.imul(y, 668265263);
  h = Math.imul(h ^ (h >>> 13), 1274126177);
  return ((h ^ (h >>> 16)) >>> 0) / 4294967295;
};

export const clamp = (v: number, lo = 0, hi = 1) => Math.min(hi, Math.max(lo, v));

/** A canvas redrawn every frame by `draw`. */
const Canvas: React.FC<{
  width: number;
  height: number;
  draw: (ctx: CanvasRenderingContext2D) => void;
  style?: React.CSSProperties;
}> = ({width, height, draw, style}) => {
  const ref = useRef<HTMLCanvasElement>(null);
  useLayoutEffect(() => {
    const ctx = ref.current?.getContext("2d");
    if (!ctx) return;
    ctx.clearRect(0, 0, width, height);
    draw(ctx);
  });
  return <canvas ref={ref} width={width} height={height} style={{display: "block", ...style}} />;
};

export type Run = {text: string; color: string};

/**
 * Text drawn as dots. `reveal` sweeps in from the left with a bright front;
 * `density` thins the dots out (1 is solid, 0 is gone).
 */
export const DitherText: React.FC<{
  lines: Run[][];
  size: number;
  width: number;
  weight?: number;
  font?: string;
  lineHeight?: number;
  align?: "left" | "center";
  reveal?: number;
  density?: number;
  cell?: number;
  style?: React.CSSProperties;
}> = ({
  lines,
  size,
  width,
  weight = 650,
  font = SANS,
  lineHeight = 1.08,
  align = "left",
  reveal = 1,
  density = 1,
  cell = 4,
  style,
}) => {
  const height = Math.ceil(lines.length * size * lineHeight + size * 0.3);
  const mask = useMemo(() => {
    const c = document.createElement("canvas");
    c.width = width;
    c.height = height;
    const ctx = c.getContext("2d")!;
    ctx.font = `${weight} ${size}px ${font}`;
    ctx.textBaseline = "alphabetic";
    lines.forEach((runs, row) => {
      const total = runs.reduce((w, r) => w + ctx.measureText(r.text).width, 0);
      let x = align === "center" ? (width - total) / 2 : 0;
      const y = (row + 0.82) * size * lineHeight;
      for (const r of runs) {
        ctx.fillStyle = r.color;
        ctx.fillText(r.text, x, y);
        x += ctx.measureText(r.text).width;
      }
    });
    return ctx.getImageData(0, 0, width, height).data;
  }, [lines, size, width, height, weight, font, lineHeight, align]);

  const dot = cell - 1;
  const draw = (ctx: CanvasRenderingContext2D) => {
    const front = reveal * 1.25 - 0.1;
    for (let row = 0; row * cell + 1 < height; row++) {
      for (let col = 0; col * cell + 1 < width; col++) {
        const x = col * cell + 1;
        const y = row * cell + 1;
        const i = (y * width + x) * 4;
        const a = mask[i + 3] / 255;
        if (a < 0.35) continue;
        const u = x / width;
        const jitter = hash(col, row) * 0.12;
        const ahead = front - (u + jitter);
        if (ahead < 0) continue;
        if ((0.78 + 0.22 * a) * density <= bayer(col, row)) continue;
        const flash = ahead < 0.05 && reveal < 1;
        ctx.fillStyle = flash ? "#ffffff" : `rgb(${mask[i]},${mask[i + 1]},${mask[i + 2]})`;
        ctx.globalAlpha = 0.78 + 0.22 * hash(col ^ 0x2f, row);
        ctx.fillRect(x - 1, y - 1, dot, dot);
      }
    }
    ctx.globalAlpha = 1;
  };
  return <Canvas width={width} height={height} draw={draw} style={style} />;
};

/** Faint dot grid over the whole frame, like the app's empty track. */
export const DotField: React.FC<{width: number; height: number; alpha?: number}> = ({
  width,
  height,
  alpha = 0.05,
}) => (
  <Canvas
    width={width}
    height={height}
    draw={(ctx) => {
      ctx.fillStyle = `rgba(255,255,255,${alpha})`;
      for (let y = 6; y < height; y += 14) for (let x = 6; x < width; x += 14) ctx.fillRect(x, y, 2, 2);
    }}
    style={{position: "absolute", inset: 0}}
  />
);

/** The usage bar: colored segments of dots filling to `fill` (0 to 1). */
export const DitherBar: React.FC<{
  width: number;
  height: number;
  fill: number;
  time: number;
  hot?: number;
}> = ({width, height, fill, time, hot = 0}) => {
  const cell = 7;
  const draw = (ctx: CanvasRenderingContext2D) => {
    ctx.fillStyle = "#1b1b1e";
    ctx.strokeStyle = "rgba(255,255,255,0.07)";
    ctx.beginPath();
    ctx.roundRect(0.5, 0.5, width - 1, height - 1, 12);
    ctx.fill();
    ctx.stroke();
    const cols = Math.floor(width / cell);
    const rows = Math.floor(height / cell);
    const end = fill * cols;
    const flicker = Math.floor(time * 20);
    for (let col = 0; col < cols; col++) {
      const used = col < end;
      const tone = TONES[Math.min(TONES.length - 1, Math.floor((col / cols) * TONES.length * 1.05))];
      const near = used ? Math.max(0, 1 - (end - col) / 8) : 0;
      for (let row = 0; row < rows; row++) {
        const x = col * cell + 2;
        const y = row * cell + 2;
        if (!used) {
          ctx.fillStyle = "rgba(255,255,255,0.05)";
          ctx.fillRect(x, y, 4, 4);
          continue;
        }
        const fy = row / Math.max(1, rows - 1);
        const density = 0.9 - 0.45 * fy + near * 0.6 + hot * 0.3;
        const seed = near > 0 ? row + flicker * 131 : row;
        if (hash(col, seed) >= density) continue;
        ctx.fillStyle = near > 0.5 ? "#ffffff" : hot > 0 && hash(col, flicker) < hot * 0.5 ? "#ff4f5e" : tone;
        ctx.globalAlpha = 0.55 + 0.45 * hash(col ^ 0x2f, row);
        ctx.fillRect(x, y, 4, 4);
      }
    }
    ctx.globalAlpha = 1;
  };
  return <Canvas width={width} height={height} draw={draw} />;
};

/** Covers the frame in black dots, Bayer order, as `amount` goes 0 to 1. */
export const DitherWipe: React.FC<{width: number; height: number; amount: number}> = ({
  width,
  height,
  amount,
}) => {
  if (amount <= 0) return null;
  const cell = 12;
  return (
    <Canvas
      width={width}
      height={height}
      style={{position: "absolute", inset: 0}}
      draw={(ctx) => {
        ctx.fillStyle = "#000";
        if (amount >= 1) {
          ctx.fillRect(0, 0, width, height);
          return;
        }
        for (let row = 0; row * cell < height; row++) {
          for (let col = 0; col * cell < width; col++) {
            const t = bayer(col, row) * 0.7 + hash(col, row) * 0.3;
            if (t < amount) ctx.fillRect(col * cell, row * cell, cell, cell);
          }
        }
      }}
    />
  );
};

/** Pixel dust bursting up from a box, like the app's clearing dust. */
export const Dust: React.FC<{
  width: number;
  height: number;
  box: [number, number, number, number];
  t: number;
  count?: number;
}> = ({width, height, box, t, count = 520}) => {
  if (t < 0 || t > 2.2) return null;
  const [x0, y0, x1, y1] = box;
  return (
    <Canvas
      width={width}
      height={height}
      style={{position: "absolute", inset: 0}}
      draw={(ctx) => {
        for (let i = 0; i < count; i++) {
          const born = hash(i, 7) * 0.35;
          const age = t - born;
          const life = 0.7 + hash(i, 3) * 1.2;
          if (age < 0 || age > life) continue;
          const vx = (hash(i, 1) - 0.5) * 520;
          const vy = -120 - hash(i, 2) * 520;
          const x = x0 + hash(i, 4) * (x1 - x0) + vx * age;
          const y = y0 + hash(i, 5) * (y1 - y0) + vy * age + 260 * age * age;
          const k = 1 - age / life;
          const side = 3 + 4 * k;
          ctx.globalAlpha = k * 0.95;
          ctx.fillStyle = hash(i, 6) > 0.7 ? "#ffffff" : TONES[i % TONES.length];
          ctx.fillRect(x, y, side, side);
        }
        ctx.globalAlpha = 1;
      }}
    />
  );
};
