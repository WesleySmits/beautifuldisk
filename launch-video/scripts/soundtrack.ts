import {readFileSync, writeFileSync} from "node:fs";
// Builds public/soundtrack.wav: the app's own sounds from the recording
// (logged by BEAUTIFULDISK_SOUND_LOG), placed on the video timeline, plus
// extra hits made with the same tick and whoosh synth as src/sound.rs.
//
//   bun scripts/soundtrack.ts

const RATE = 44_100;
const LENGTH = 39;
const out = new Float32Array(RATE * LENGTH);

// The recording's sound log is in app seconds, 0.665s ahead of the take.
// Each app scene shows a stretch of the take; these match src/Launch.tsx.
const LOG_TO_TAKE = -0.665;
const SEGMENTS = [
  {take: 1.2, video: 2, length: 8},
  {take: 9.3, video: 16, length: 10.5},
];
const takeToVideo = (take: number) => {
  for (const s of SEGMENTS) if (take >= s.take && take < s.take + s.length) return take - s.take + s.video;
  return undefined;
};

let seed = 0x9e3779b9;
const random = () => {
  seed ^= seed << 13;
  seed ^= seed >>> 17;
  seed ^= seed << 5;
  return (seed >>> 0) / 0xffffffff;
};

const mix = (samples: ArrayLike<number>, at: number, gain = 1) => {
  const start = Math.round(at * RATE);
  for (let i = 0; i < samples.length; i++) {
    const j = start + i;
    if (j >= 0 && j < out.length) out[j] += samples[i] * gain;
  }
};

const tick = (pitch: number, reversed = false, decay = 0.004) => {
  const freq = 1500 + pitch * 1400 + random() * 180;
  const n = Math.round(RATE * Math.max(0.03, decay * 8));
  const s = Array.from({length: n}, (_, i) => {
    const t = i / RATE;
    const tone = Math.sin(2 * Math.PI * freq * t) * Math.exp(-t / decay);
    const click = (random() * 2 - 1) * Math.exp(-t / 0.0007);
    return 0.05 * tone + 0.025 * click;
  });
  return reversed ? s.reverse() : s;
};

const whoosh = (seconds = 0.5, from = 3500, to = 300, tail = 0.14) => {
  const n = Math.round(RATE * seconds);
  let low = 0;
  return Array.from({length: n}, (_, i) => {
    const t = i / RATE;
    const cutoff = to + (from - to) * Math.exp(-t / (tail * 0.85));
    const a = 1 - Math.exp((-2 * Math.PI * cutoff) / RATE);
    low += a * (random() * 2 - 1 - low);
    const envelope = Math.min(t / 0.02, 1) * Math.exp(-t / tail);
    return 0.22 * low * envelope;
  });
};

// Low thump under the big clear: a falling sine.
const thump = () => {
  const n = Math.round(RATE * 0.6);
  let phase = 0;
  return Array.from({length: n}, (_, i) => {
    const t = i / RATE;
    phase += (2 * Math.PI * (40 + 70 * Math.exp(-t / 0.05))) / RATE;
    return 0.35 * Math.sin(phase) * Math.min(t / 0.004, 1) * Math.exp(-t / 0.16);
  });
};

// Intro: the bar fills, ticking faster and higher as it goes.
for (let t = 0.25; t < 1.95; ) {
  const f = (t - 0.25) / 1.7;
  mix(tick(f), t, 2.4);
  t += 0.07 - 0.035 * f;
}
mix(whoosh(0.5, 5000, 400, 0.12), 1.98, 2.2);

// The app, as recorded.
const log = new DataView(readFileSync(new URL("app-sounds.bin", import.meta.url)).buffer);
for (let o = 0; o < log.byteLength; ) {
  const at = log.getFloat64(o, true);
  const len = log.getUint32(o + 8, true);
  const samples = new Float32Array(len);
  for (let i = 0; i < len; i++) samples[i] = log.getFloat32(o + 12 + i * 4, true);
  o += 12 + len * 4;
  const video = takeToVideo(at + LOG_TO_TAKE);
  if (video === undefined) continue;
  // Whooshes (long) are the payoff; ticks stay a texture.
  mix(samples, video, len > 5000 ? 3.2 : 2.4);
}
// The app is silent on hovers and clicks; a soft tick marks each one
// (take seconds: three treemap hovers, then the files tour).
for (const take of [6.0, 6.6, 7.2, 10.72, 11.52, 13.32, 14.52, 15.72, 16.34, 17.32]) {
  const video = takeToVideo(take);
  if (video !== undefined) mix(tick(0.8, false, 0.006), video, 2.2);
}

// Cuts into and out of the scenes drawn here.
for (const cut of [10, 16, 26.5]) mix(whoosh(0.45, 5000, 500, 0.1), cut - 0.05, 1.6);
// Features: a rising tick per row.
for (let i = 0; i < 6; i++) mix(tick(0.3 + i * 0.12, false, 0.02), 10 + 0.7 + i * 0.42, 2.4);
// Apps and Docker: each side lands, then its bar drains.
for (const [land, drain] of [
  [26.7, 27.4],
  [28.2, 28.9],
]) {
  mix(tick(0.9, false, 0.03), land, 2.6);
  mix(whoosh(1.1, 2500, 250, 0.3), drain, 2.2);
}

// Into the outro: the big bushh.
mix(whoosh(0.45, 5000, 500, 0.1), 31.8, 1.6);
mix(whoosh(1.4, 6500, 180, 0.38), 32.0, 4.2);
mix(thump(), 32.0, 1.4);
// The counter rolls up.
for (let t = 32.12; t < 33.0; t += 0.045) mix(tick((t - 32.12) / 0.9), t, 2.0);
// Logo.
mix(whoosh(0.6, 3000, 300, 0.16), 34.15, 1.8);
mix(tick(1, false, 0.05), 34.35, 3.0);
mix(tick(0.6, false, 0.05), 34.47, 2.2);

// Master: soft clip, then peak at -1 dB.
let peak = 0;
for (let i = 0; i < out.length; i++) {
  out[i] = Math.tanh(out[i] * 1.2);
  peak = Math.max(peak, Math.abs(out[i]));
}
const gain = 0.89 / peak;

const data = new DataView(new ArrayBuffer(44 + out.length * 2));
const str = (o: number, s: string) => [...s].forEach((c, i) => data.setUint8(o + i, c.charCodeAt(0)));
str(0, "RIFF");
data.setUint32(4, 36 + out.length * 2, true);
str(8, "WAVEfmt ");
data.setUint32(16, 16, true);
data.setUint16(20, 1, true);
data.setUint16(22, 1, true);
data.setUint32(24, RATE, true);
data.setUint32(28, RATE * 2, true);
data.setUint16(32, 2, true);
data.setUint16(34, 16, true);
str(36, "data");
data.setUint32(40, out.length * 2, true);
out.forEach((s, i) => data.setInt16(44 + i * 2, Math.round(Math.max(-1, Math.min(1, s * gain)) * 32767), true));
writeFileSync(new URL("../public/soundtrack.wav", import.meta.url), new Uint8Array(data.buffer));
console.log(`peak ${peak.toFixed(3)}, gain ${gain.toFixed(2)}`);
