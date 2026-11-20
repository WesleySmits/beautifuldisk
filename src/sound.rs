//! Quiet synthesized sounds for the dither: a tick as bars grow, the same
//! tick played backwards as they shrink, and a soft whoosh when an item is
//! cleared away. Without an audio device, or with `--no-sound`, the app
//! stays silent.
//!
//! The output stream lives on its own thread and opens only while a sound
//! plays, then closes after `IDLE`. An open stream keeps the output device
//! active, which on macOS can pull Bluetooth headphones (AirPods) over to
//! this Mac even though nothing is audible.

use std::cell::{Cell, RefCell};
use std::f32::consts::TAU;
use std::fs::File;
use std::io::Write;
use std::num::NonZero;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use rodio::buffer::SamplesBuffer;
use rodio::cpal::BufferSize;
use rodio::{DeviceSinkBuilder, MixerDeviceSink};

const RATE: u32 = 44_100;
/// At most this many ticks a second, however fast a bar moves.
const MAX_RATE: f32 = 24.0;
/// How long the stream stays open after the last sound ends, so the ticks
/// of one animation share a stream instead of reopening it each time.
const IDLE: Duration = Duration::from_secs(2);

pub struct Sound {
    /// Feeds the audio thread. `None` when sound is off: no thread, no stream.
    player: Option<Sender<Vec<f32>>>,
    seed: Cell<u32>,
    /// With `BEAUTIFULDISK_SOUND_LOG` set, every sound is also appended there
    /// (seconds since start as f64, sample count as u32, then f32 samples,
    /// little endian), so a screen recording can get the exact soundtrack.
    log: Option<RefCell<(Instant, File)>>,
}

impl Sound {
    /// With `enabled` false nothing ever touches an audio device.
    pub fn new(enabled: bool) -> Self {
        let player = enabled.then(|| {
            let (tx, rx) = mpsc::channel();
            std::thread::spawn(move || {
                run(rx, IDLE, open, |sink: &MixerDeviceSink, samples| {
                    let channels = NonZero::new(1).unwrap();
                    let rate = NonZero::new(RATE).unwrap();
                    sink.mixer()
                        .add(SamplesBuffer::new(channels, rate, samples));
                })
            });
            tx
        });
        let log = std::env::var_os("BEAUTIFULDISK_SOUND_LOG")
            .and_then(|path| File::create(path).ok())
            .map(|file| RefCell::new((Instant::now(), file)));
        Self {
            player,
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
        if let Some(player) = &self.player {
            let _ = player.send(samples);
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

fn open() -> Option<MixerDeviceSink> {
    // Let the device pick its buffer size: rodio's default (2048) is
    // more than some devices, like Bluetooth speakers, accept.
    let sink = DeviceSinkBuilder::from_default_device()
        .and_then(|b| b.with_buffer_size(BufferSize::Default).open_stream())
        .or_else(|_| DeviceSinkBuilder::open_default_sink());
    sink.ok().map(|mut sink| {
        sink.log_on_drop(false);
        sink
    })
}

/// The audio thread: opens a stream for the first sound, plays into it, and
/// drops it once `idle` has passed after the last sound ended. A failed open
/// is not retried until that idle point, so a missing device costs one
/// attempt per burst of sounds rather than one per tick.
fn run<S>(
    rx: Receiver<Vec<f32>>,
    idle: Duration,
    mut open: impl FnMut() -> Option<S>,
    play: impl Fn(&S, Vec<f32>),
) {
    // `None`: closed. `Some(None)`: tried and failed. `Some(Some(_))`: open.
    let mut stream: Option<Option<S>> = None;
    let mut ends = Instant::now();
    loop {
        let next = match stream {
            None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
            Some(_) => rx.recv_timeout((ends + idle).saturating_duration_since(Instant::now())),
        };
        match next {
            Ok(samples) => {
                let now = Instant::now();
                ends = ends.max(now) + Duration::from_secs_f32(samples.len() as f32 / RATE as f32);
                if let Some(sink) = stream.get_or_insert_with(&mut open) {
                    play(sink, samples);
                }
            }
            Err(RecvTimeoutError::Timeout) => stream = None,
            Err(RecvTimeoutError::Disconnected) => return,
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::rc::Rc;

    #[test]
    fn disabled_sound_has_no_audio_thread() {
        let sound = Sound::new(false);
        assert!(sound.player.is_none());
        sound.whoosh();
    }

    /// Counts how many streams are open at once.
    struct Stream(Rc<Cell<i32>>);
    impl Drop for Stream {
        fn drop(&mut self) {
            self.0.set(self.0.get() - 1);
        }
    }

    #[test]
    fn stream_opens_for_sound_and_closes_when_idle() {
        let (tx, rx) = mpsc::channel();
        let (report, events) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let live = Rc::new(Cell::new(0));
            let opens = Cell::new(0);
            run(
                rx,
                Duration::from_millis(50),
                || {
                    opens.set(opens.get() + 1);
                    live.set(live.get() + 1);
                    Some(Stream(live.clone()))
                },
                |stream: &Stream, _| report.send(("play", stream.0.get())).unwrap(),
            );
            report.send(("opens", opens.get())).unwrap();
            report.send(("live", live.get())).unwrap();
        });
        // Two quick sounds share one stream; one after the idle gap reopens.
        tx.send(vec![0.0; 10]).unwrap();
        tx.send(vec![0.0; 10]).unwrap();
        std::thread::sleep(Duration::from_millis(200));
        tx.send(vec![0.0; 10]).unwrap();
        std::thread::sleep(Duration::from_millis(200));
        drop(tx);
        worker.join().unwrap();
        let got: Vec<_> = events.iter().collect();
        assert_eq!(
            got,
            [
                ("play", 1),
                ("play", 1),
                ("play", 1),
                ("opens", 2),
                ("live", 0)
            ]
        );
    }
}
