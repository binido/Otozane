//! Backend-independent device state and ring helpers.

use std::sync::atomic::{AtomicU32, AtomicU64};

/// Channels carried by the output and monitor rings.
pub const STEREO: usize = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    /// PipeWire `node.name` or Core Audio device UID, stable across reconnects.
    pub name: String,
    pub description: String,
    pub is_virtual: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum CaptureState {
    #[default]
    Starting,
    /// The selected device is not present; capture resumes when it appears.
    WaitingForDevice(Option<String>),
    Connecting(String),
    Running(String),
    /// Capture stopped on request; the virtual microphone outputs silence.
    Muted,
    Error(String),
}

/// Snapshot of backend state for diagnostics and the UI.
#[derive(Debug, Clone, Default)]
pub struct Status {
    pub devices: Vec<Device>,
    pub capture: CaptureState,
    pub virtual_source_ready: bool,
}

/// Counters updated on the audio thread.
#[derive(Debug, Default)]
pub struct Counters {
    /// Frames dropped because the output ring was full.
    pub overrun_frames: AtomicU64,
    /// Frames of silence emitted because no processed audio was available.
    pub underrun_frames: AtomicU64,
    /// Frames discarded to keep output latency bounded.
    pub skipped_frames: AtomicU64,
    /// Frames in the most recent capture/playback buffers (graph quantum).
    pub capture_quantum: AtomicU32,
    pub playback_quantum: AtomicU32,
    /// Frames left queued in the output ring after the last playback cycle.
    pub ring_fill: AtomicU32,
    /// Number of capture callbacks that reached the engine.
    pub capture_cycles: AtomicU64,
}

/// Input for "Automatic": the system default source if it is a physical
/// device, else the last physical default seen while running (the default is
/// often set to our own microphone so apps use it), else the first physical device.
pub fn auto_input<'a>(
    devices: impl Iterator<Item = &'a Device> + Clone,
    default: Option<&str>,
    last_default: Option<&str>,
) -> Option<String> {
    let physical = devices.filter(|d| !d.is_virtual);
    let find = |n: Option<&str>| n.and_then(|n| physical.clone().find(|d| d.name == n));
    find(default)
        .or_else(|| find(last_default))
        .or_else(|| physical.clone().next())
        .map(|d| d.name.clone())
}

/// Writes interleaved `samples` to `ring` as stereo (mono is duplicated).
/// Returns the frames written; the rest are dropped when the ring is full.
pub fn push_stereo(ring: &mut rtrb::Producer<f32>, samples: &[f32], channels: usize) -> usize {
    let n = (samples.len() / channels).min(ring.slots() / STEREO);
    let Ok(mut chunk) = ring.write_chunk(n * STEREO) else {
        return 0;
    };
    let (a, b) = chunk.as_mut_slices();
    let mut out = a.iter_mut().chain(b.iter_mut());
    for frame in samples.chunks_exact(channels).take(n) {
        let (l, r) = (frame[0], frame[channels - 1]);
        if let (Some(o1), Some(o2)) = (out.next(), out.next()) {
            *o1 = l;
            *o2 = r;
        }
    }
    chunk.commit_all();
    n
}

/// Fills `out` with `frames` stereo frames from `ring`, padding with silence.
/// Queued audio beyond this cycle plus one capture quantum is stale (e.g.
/// buffered while nobody recorded) and is discarded first.
/// Returns (frames read, frames skipped, frames left queued).
pub fn pull_stereo(
    ring: &mut rtrb::Consumer<f32>,
    out: &mut [f32],
    frames: usize,
    capture_quantum: usize,
) -> (usize, usize, usize) {
    let avail = ring.slots() / STEREO;
    let keep = frames + capture_quantum;
    let mut skipped = 0;
    if avail > keep + 256 {
        skipped = avail - keep;
        if let Ok(c) = ring.read_chunk(skipped * STEREO) {
            c.commit_all();
        }
    }
    let avail = ring.slots() / STEREO;
    let got = avail.min(frames);
    if let Ok(chunk) = ring.read_chunk(got * STEREO) {
        let (a, b) = chunk.as_slices();
        out[..a.len()].copy_from_slice(a);
        out[a.len()..a.len() + b.len()].copy_from_slice(b);
        chunk.commit_all();
    }
    out[got * STEREO..frames * STEREO].fill(0.0);
    (got, skipped, avail - got)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_carries_stereo_and_drops_stale_audio() {
        let (mut tx, mut rx) = rtrb::RingBuffer::new(4096 * STEREO);
        // Mono is duplicated to both channels.
        assert_eq!(push_stereo(&mut tx, &[0.5, -0.5], 1), 2);
        let mut out = [9.0; 8];
        assert_eq!(pull_stereo(&mut rx, &mut out, 4, 0), (2, 0, 0));
        assert_eq!(out, [0.5, 0.5, -0.5, -0.5, 0.0, 0.0, 0.0, 0.0]);
        // A backlog beyond this cycle plus one quantum is skipped, newest kept.
        let block: Vec<f32> = (0..2000).map(|i| i as f32).collect();
        assert_eq!(push_stereo(&mut tx, &block, 1), 2000);
        let mut out = vec![0.0; 200];
        assert_eq!(pull_stereo(&mut rx, &mut out, 100, 100), (100, 1800, 100));
        assert_eq!(out[0], 1800.0);
    }
}
