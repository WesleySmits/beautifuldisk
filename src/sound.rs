//! Quiet synthesized sounds for the dither: a tick as bars grow, the same
//! tick played backwards as they shrink, and a soft whoosh when an item is
//! cleared away. Without an audio device the app stays silent.

use std::cell::{Cell, RefCell};
use std::f32::consts::TAU;
use std::fs::File;
use std::io::Write;
use std::num::NonZero;
use std::time::Instant;

use rodio::buffer::SamplesBuffer;
use rodio::cpal::BufferSize;
use rodio::{DeviceSinkBuilder, MixerDeviceSink};

const RATE: u32 = 44_100;
/// At most this many ticks a second, however fast a bar moves.
const MAX_RATE: f32 = 24.0;

pub struct Sound {
    sink: Option<MixerDeviceSink>,
    seed: Cell<u32>,
    /// With `BEAUTIFULDISK_SOUND_LOG` set, every sound is also appended there
    /// (seconds since start as f64, sample count as u32, then f32 samples,
    /// little endian), so a screen recording can get the exact soundtrack.
    log: Option<RefCell<(Instant, File)>>,
}

impl Sound {
    pub fn new() -> Self {
        // Let the device pick its buffer size: rodio's default (2048) is
        // more than some devices, like Bluetooth speakers, accept.
        let sink = DeviceSinkBuilder::from_default_device()
            .and_then(|b| b.with_buffer_size(BufferSize::Default).open_stream())
            .or_else(|_| DeviceSinkBuilder::open_default_sink());
        let sink = sink.ok().map(|mut sink| {
            sink.log_on_drop(false);
            sink
        });
        let log = std::env::var_os("BEAUTIFULDISK_SOUND_LOG")
            .and_then(|path| File::create(path).ok())
            .map(|file| RefCell::new((Instant::now(), file)));
        Self {
            sink,
            seed: Cell::new(0x9E37_79B9),
            log,
        }
    }

    fn random(&self) -> f32 {
        let mut x = self.seed.get();
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.seed.set(x);
        x as f32 / u32::MAX as f32
    }

    fn play(&self, samples: Vec<f32>) {
        if let Some(log) = &self.log {
            let (start, file) = &mut *log.borrow_mut();
            let mut record = start.elapsed().as_secs_f64().to_le_bytes().to_vec();
            record.extend((samples.len() as u32).to_le_bytes());
            record.extend(samples.iter().flat_map(|s| s.to_le_bytes()));
            let _ = file.write_all(&record);
        }
        if let Some(sink) = &self.sink {
            let channels = NonZero::new(1).unwrap();
            let rate = NonZero::new(RATE).unwrap();
            sink.mixer()
                .add(SamplesBuffer::new(channels, rate, samples));
        }
    }

    /// A short click, pitched by `pitch` (0 low to 1 high).
    fn tick(&self, pitch: f32, reversed: bool) {
        self.play(self.tick_samples(pitch, reversed));
    }

    fn tick_samples(&self, pitch: f32, reversed: bool) -> Vec<f32> {
        let freq = 1500.0 + pitch * 1400.0 + self.random() * 180.0;
        let n = (RATE as f32 * 0.03) as usize;
        let mut samples: Vec<f32> = (0..n)
            .map(|i| {
                let t = i as f32 / RATE as f32;
                let tone = (TAU * freq * t).sin() * (-t / 0.004).exp();
                let click = (self.random() * 2.0 - 1.0) * (-t / 0.0007).exp();
                0.05 * tone + 0.025 * click
            })
            .collect();
        if reversed {
            // Swells in and snaps off, so it sounds like the tick rewinding.
            samples.reverse();
        }
        samples
    }

    /// Filtered noise that sweeps down, like air rushing out.
    pub fn whoosh(&self) {
        self.play(self.whoosh_samples());
    }

    fn whoosh_samples(&self) -> Vec<f32> {
        let n = (RATE as f32 * 0.5) as usize;
        let mut low = 0.0;
        (0..n)
            .map(|i| {
                let t = i as f32 / RATE as f32;
                let cutoff = 300.0 + 3200.0 * (-t / 0.12).exp();
                let a = 1.0 - (-TAU * cutoff / RATE as f32).exp();
                low += a * ((self.random() * 2.0 - 1.0) - low);
                let envelope = (t / 0.02).min(1.0) * (-t / 0.14).exp();
                0.22 * low * envelope
            })
            .collect()
    }
}

/// Turns a bar's motion into a stream of ticks: one per `step` of the bar
/// travelled, but never faster than `MAX_RATE`.
#[derive(Default)]
pub struct Ticker {
    travelled: f32,
    since: f32,
}

impl Ticker {
    /// `delta` is how far the bar moved this frame as a fraction of its width.
    /// Growing ticks forward; shrinking ticks backward. `pitch` follows the
    /// bar's fill so the sound climbs as it fills and falls as it empties.
    pub fn step(&mut self, sound: &Sound, delta: f32, dt: f32, pitch: f32) {
        const STEP: f32 = 0.006;
        self.since += dt;
        if delta.abs() < 1e-5 {
            self.travelled = 0.0;
            return;
        }
        self.travelled += delta.abs();
        if self.travelled >= STEP && self.since >= 1.0 / MAX_RATE {
            self.travelled = 0.0;
            self.since = 0.0;
            sound.tick(pitch.clamp(0.0, 1.0), delta < 0.0);
        }
    }
}
