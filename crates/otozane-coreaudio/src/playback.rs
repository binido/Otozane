//! Clock matching and continuity for the virtual microphone.

use super::{RATE, Resampler, STEREO};

const TRANSITION_FRAMES: usize = 128;
const STALE_MARGIN: usize = 256;
const FILL_SMOOTH_SECONDS: f64 = 2.0;
const CORRECTION_PER_FRAME: f64 = 0.000_001;
const MAX_CORRECTION: f64 = 0.001;

pub(super) struct OutputRing {
    samples: rtrb::Consumer<f32>,
    capturing: bool,
    generation: u64,
}

impl OutputRing {
    pub(super) fn new(samples: rtrb::Consumer<f32>) -> Self {
        Self {
            samples,
            capturing: false,
            generation: 0,
        }
    }

    pub(super) fn set_capturing(&mut self, capturing: bool) {
        self.capturing = capturing;
        if !capturing {
            self.discard();
        }
    }

    pub(super) fn discard(&mut self) {
        let available = self.available();
        self.skip(available);
        self.generation = self.generation.wrapping_add(1);
    }

    fn available(&self) -> usize {
        self.samples.slots() / STEREO
    }

    fn skip(&mut self, frames: usize) {
        if let Ok(chunk) = self.samples.read_chunk(frames * STEREO) {
            chunk.commit_all();
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Buffering {
    Priming,
    Playing,
    Recovering,
}

pub(super) struct Playback {
    generation: u64,
    buffering: Buffering,
    mean_fill: Option<f64>,
    resampler: Resampler,
    last: [f32; STEREO],
    anchor: [f32; STEREO],
    fade_left: usize,
}

impl Playback {
    pub(super) fn new() -> Self {
        Self {
            generation: 0,
            buffering: Buffering::Priming,
            mean_fill: None,
            resampler: Resampler::new(RATE, RATE),
            last: [0.0; STEREO],
            anchor: [0.0; STEREO],
            fade_left: 0,
        }
    }

    fn start_transition(&mut self) {
        self.anchor = self.last;
        self.fade_left = TRANSITION_FRAMES;
    }

    fn reprime(&mut self, buffering: Buffering) {
        self.buffering = buffering;
        self.resampler = Resampler::new(RATE, RATE);
        self.mean_fill = None;
        self.start_transition();
    }

    fn write(&mut self, out: &mut [f32; STEREO], next: [f32; STEREO]) {
        self.last = next;
        if self.fade_left != 0 {
            self.fade_left -= 1;
            let gain = 1.0 - self.fade_left as f32 / TRANSITION_FRAMES as f32;
            self.last =
                std::array::from_fn(|ch| self.anchor[ch] + (next[ch] - self.anchor[ch]) * gain);
        }
        *out = self.last;
    }

    pub(super) fn gap(&mut self, out: &mut [f32]) -> usize {
        if self.buffering == Buffering::Playing {
            self.reprime(Buffering::Recovering);
        }
        let fade_samples = out.len().min(self.fade_left * STEREO);
        let (fade, silence) = out.split_at_mut(fade_samples);
        for frame in fade.as_chunks_mut::<STEREO>().0 {
            self.write(frame, [0.0; STEREO]);
        }
        silence.fill(0.0);
        if self.buffering == Buffering::Recovering {
            out.len() / STEREO
        } else {
            0
        }
    }

    /// Returns missed, stale, and remaining ring frames.
    pub(super) fn render(
        &mut self,
        ring: &mut OutputRing,
        out: &mut [f32],
        capture_quantum: usize,
    ) -> (usize, usize, usize) {
        if self.generation != ring.generation {
            self.generation = ring.generation;
            if !ring.capturing {
                self.last = [0.0; STEREO];
            }
            self.reprime(Buffering::Priming);
        }
        if !ring.capturing {
            out.fill(0.0);
            return (0, 0, 0);
        }

        let frames = out.len() / STEREO;
        let capacity = ring.samples.buffer().capacity() / STEREO;
        let target_fill = capture_quantum.max(2).min(capacity.saturating_sub(frames));
        let keep = frames.min(capacity) + target_fill;
        let mut available = ring.available();
        let mut skipped = 0;
        if available > keep + capture_quantum + STALE_MARGIN {
            skipped = available - keep;
            ring.skip(skipped);
            available = keep;
            self.reprime(Buffering::Priming);
        }
        if self.buffering != Buffering::Playing {
            if available < keep {
                return (self.gap(out), skipped, available);
            }
            self.buffering = Buffering::Playing;
            self.start_transition();
        }

        // Average out the queue's callback-to-callback swings before adjusting the clock ratio.
        let mean = self.mean_fill.get_or_insert(target_fill as f64);
        let weight = (frames as f64 / (f64::from(RATE) * FILL_SMOOTH_SECONDS)).min(1.0);
        *mean += weight * (available as f64 - frames as f64 - *mean);
        let correction = (*mean - target_fill as f64) * CORRECTION_PER_FRAME;
        self.resampler.step = 1.0 + correction.clamp(-MAX_CORRECTION, MAX_CORRECTION);

        // Stereo-aligned writes and reads keep every frame contiguous across ring wrap.
        let chunk = ring.samples.read_chunk(available * STEREO).unwrap();
        let (first, second) = chunk.as_slices();
        let mut input = first
            .as_chunks::<STEREO>()
            .0
            .iter()
            .chain(second.as_chunks::<STEREO>().0);
        let mut consumed = 0;
        let mut rendered = 0;
        for frame in out.as_chunks_mut::<STEREO>().0 {
            let Some(next) = self.resampler.next(|| {
                let frame = *input.next()?;
                consumed += 1;
                Some(frame)
            }) else {
                break;
            };
            self.write(frame, next);
            rendered += 1;
        }
        chunk.commit(consumed * STEREO);
        let missing = if rendered < frames {
            self.gap(&mut out[rendered * STEREO..])
        } else {
            0
        };
        (missing, skipped, ring.available())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use otozane_core::backend::push_stereo;

    const PLAY: usize = 512;
    const CAPTURE: usize = 557;
    const SIGNAL: [f32; STEREO] = [0.3, -0.6];
    const OTHER_SIGNAL: [f32; STEREO] = [-0.5, 0.25];

    fn setup() -> (rtrb::Producer<f32>, OutputRing, Playback) {
        let (tx, rx) = rtrb::RingBuffer::new(super::super::RING_FRAMES * STEREO);
        let mut ring = OutputRing::new(rx);
        ring.set_capturing(true);
        (tx, ring, Playback::new())
    }

    #[test]
    fn tracks_both_clock_directions_with_44_1k_capture() {
        for ppm in [-50, 50] {
            let (mut tx, mut ring, mut playback) = setup();
            let input = SIGNAL.repeat(CAPTURE + 1);
            let mut out = [0.0; PLAY * STEREO];
            let mut capture = 0_i64;
            let mut quantum = 0;
            let mut peak_fill = 0;
            let mut correction = 0.0;
            let mut measurements = 0;
            let end = 120 * i64::from(RATE) / PLAY as i64;
            for play in 1..=end {
                while capture * i64::from(RATE) * 1_000_000 <= play * 44_100 * (1_000_000 + ppm) {
                    let frames_at = |n| n * PLAY as i64 * i64::from(RATE) / 44_100;
                    quantum = (frames_at(capture + 1) - frames_at(capture)) as usize;
                    assert_eq!(
                        push_stereo(&mut tx, &input[..quantum * STEREO], STEREO),
                        quantum
                    );
                    capture += 1;
                }
                let (missing, skipped, fill) = playback.render(&mut ring, &mut out, quantum);
                assert_eq!((missing, skipped), (0, 0), "{ppm} ppm, callback {play}");
                peak_fill = peak_fill.max(fill);
                if play > end / 2 {
                    assert_eq!(&out[..STEREO], &SIGNAL);
                    correction += (playback.resampler.step - 1.0) * 1_000_000.0;
                    measurements += 1;
                }
            }
            let average_ppm = correction / f64::from(measurements);
            assert!(
                (average_ppm - ppm as f64).abs() < 15.0,
                "{ppm} ppm: {average_ppm}"
            );
            assert!(peak_fill <= 2 * (CAPTURE + 1), "{ppm} ppm: {peak_fill}");
        }
    }

    #[test]
    fn starvation_fades_to_silence_and_waits_for_headroom() {
        let (mut tx, mut ring, mut playback) = setup();
        let mut out = [0.0; PLAY * STEREO];
        push_stereo(&mut tx, &SIGNAL.repeat(PLAY + CAPTURE), STEREO);
        assert_eq!(playback.render(&mut ring, &mut out, CAPTURE).0, 0);
        assert_eq!(playback.render(&mut ring, &mut out, CAPTURE).0, 0);
        push_stereo(&mut tx, &SIGNAL.repeat(2 * PLAY - CAPTURE - 8), STEREO);
        let (missing, _, left) = playback.render(&mut ring, &mut out, CAPTURE);
        assert!((1..TRANSITION_FRAMES).contains(&missing));
        assert_eq!(left, 0);
        assert_smooth(&out, SIGNAL, 0.01);
        let tail = [out[out.len() - STEREO], out[out.len() - 1]];
        assert!(tail[0] > 0.0 && tail[0] < SIGNAL[0]);

        push_stereo(&mut tx, &SIGNAL.repeat(CAPTURE), STEREO);
        assert_eq!(
            playback.render(&mut ring, &mut out, CAPTURE),
            (PLAY, 0, CAPTURE)
        );
        assert_smooth(&out, tail, 0.01);
        assert!(out[TRANSITION_FRAMES * STEREO..].iter().all(|&s| s == 0.0));
        push_stereo(&mut tx, &SIGNAL.repeat(CAPTURE), STEREO);
        assert_eq!(playback.render(&mut ring, &mut out, CAPTURE).0, 0);
        assert_smooth(&out, [0.0; STEREO], 0.01);
        assert_eq!(&out[out.len() - STEREO..], &SIGNAL);
    }

    #[test]
    fn stale_trim_blends_to_recent_audio() {
        let (mut tx, mut ring, mut playback) = setup();
        let mut out = [0.0; PLAY * STEREO];
        push_stereo(&mut tx, &SIGNAL.repeat(PLAY + CAPTURE), STEREO);
        playback.render(&mut ring, &mut out, CAPTURE);
        push_stereo(&mut tx, &OTHER_SIGNAL.repeat(4 * PLAY), STEREO);
        let (missing, skipped, fill) = playback.render(&mut ring, &mut out, CAPTURE);
        assert_eq!((missing, skipped, fill), (0, 3 * PLAY, CAPTURE));
        assert_smooth(&out, SIGNAL, 0.015);
        assert_eq!(&out[out.len() - STEREO..], &OTHER_SIGNAL);
    }

    #[test]
    fn stopped_capture_clears_audio_and_restarts_with_fresh_headroom() {
        let (mut tx, mut ring, mut playback) = setup();
        let mut out = [0.0; PLAY * STEREO];
        push_stereo(&mut tx, &SIGNAL.repeat(PLAY + CAPTURE), STEREO);
        playback.render(&mut ring, &mut out, CAPTURE);
        ring.set_capturing(false);
        assert_eq!(ring.available(), 0);
        assert_eq!(playback.render(&mut ring, &mut out, CAPTURE), (0, 0, 0));
        assert!(out.iter().all(|&s| s == 0.0));

        ring.set_capturing(true);
        push_stereo(&mut tx, &OTHER_SIGNAL.repeat(CAPTURE), STEREO);
        assert_eq!(
            playback.render(&mut ring, &mut out, CAPTURE),
            (0, 0, CAPTURE)
        );
        assert!(out.iter().all(|&s| s == 0.0));
        push_stereo(&mut tx, &OTHER_SIGNAL.repeat(CAPTURE), STEREO);
        playback.render(&mut ring, &mut out, CAPTURE);
        assert_smooth(&out, [0.0; STEREO], 0.01);
        assert_eq!(&out[out.len() - STEREO..], &OTHER_SIGNAL);
    }

    #[test]
    fn replacing_the_output_clears_the_queue_and_primes_again() {
        let (mut tx, mut ring, mut playback) = setup();
        let mut out = [0.0; PLAY * STEREO];
        push_stereo(&mut tx, &SIGNAL.repeat(PLAY + CAPTURE), STEREO);
        playback.render(&mut ring, &mut out, CAPTURE);
        ring.discard();
        let mut playback = Playback::new();
        push_stereo(&mut tx, &OTHER_SIGNAL.repeat(CAPTURE), STEREO);
        assert_eq!(
            playback.render(&mut ring, &mut out, CAPTURE),
            (0, 0, CAPTURE)
        );
        assert!(out.iter().all(|&s| s == 0.0));
        push_stereo(&mut tx, &OTHER_SIGNAL.repeat(CAPTURE), STEREO);
        playback.render(&mut ring, &mut out, CAPTURE);
        assert_smooth(&out, [0.0; STEREO], 0.01);
        assert_eq!(&out[out.len() - STEREO..], &OTHER_SIGNAL);
    }

    fn assert_smooth(out: &[f32], mut previous: [f32; STEREO], limit: f32) {
        for frame in out.as_chunks::<STEREO>().0 {
            for ch in 0..STEREO {
                assert!(
                    (frame[ch] - previous[ch]).abs() < limit,
                    "{previous:?} -> {frame:?}"
                );
                previous[ch] = frame[ch];
            }
        }
    }
}
