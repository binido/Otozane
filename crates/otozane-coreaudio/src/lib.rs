//! Core Audio backend: captures the selected microphone, runs the engine in
//! its input callback and plays the result into BlackHole 2ch, which
//! applications record from as a microphone.
//!
//! Devices are polled once a second. Streams at rates other than 48 kHz are
//! resampled on the way in (capture) and out (monitor). BlackHole runs at 48 kHz.

#![cfg(target_os = "macos")]

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
pub use otozane_core::backend::{CaptureState, Counters, Device, Status};
use otozane_core::backend::{STEREO, auto_input, pull_stereo, push_stereo};
use otozane_core::engine::Engine;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const RATE: u32 = otozane_core::SAMPLE_RATE;
/// UID of BlackHole 2ch, the device apps record from. Never offered as an input.
const OUTPUT_UID: &str = "BlackHole2ch_UID";
/// Fade-in applied when a capture stream starts (20 ms).
const FADE_IN_FRAMES: usize = 960;
/// Output ring capacity (0.5 s). Excess is discarded, so this bounds memory, not latency.
const RING_FRAMES: usize = 24_000;
/// Input frames resampled per step. Bounds the scratch buffer for rates down to 8 kHz.
const CHUNK_FRAMES: usize = 1024;
const SCRATCH_FRAMES: usize = CHUNK_FRAMES * RATE as usize / 8000 + 2;

/// Id of the virtual microphone. The service rejects it as an input.
pub fn virtual_node_name() -> &'static str {
    OUTPUT_UID
}

enum Command {
    SetInput(Option<String>),
    SetMuted(bool),
    SetMonitor(bool),
    Quit,
}

/// State touched by the input callback, which only uses `try_lock`.
struct RtState {
    engine: Engine,
    out: rtrb::Producer<f32>,
    /// Feeds the monitor stream, written only while `Shared::monitor_on`.
    monitor: rtrb::Producer<f32>,
}

#[derive(Clone)]
struct Shared {
    rt: Arc<Mutex<RtState>>,
    out_rx: Arc<Mutex<rtrb::Consumer<f32>>>,
    monitor_rx: Arc<Mutex<rtrb::Consumer<f32>>>,
    monitor_on: Arc<AtomicBool>,
    status: Arc<Mutex<Status>>,
    counters: Arc<Counters>,
}

pub struct Backend {
    tx: Sender<Command>,
    thread: Option<JoinHandle<()>>,
    pub status: Arc<Mutex<Status>>,
    pub counters: Arc<Counters>,
}

impl Backend {
    /// Starts the backend thread. `input` is the device UID of the
    /// microphone to capture, or `None` to follow the system default.
    pub fn start(engine: Engine, input: Option<String>) -> Self {
        let status = Arc::new(Mutex::new(Status::default()));
        let counters = Arc::new(Counters::default());
        let (out_tx, out_rx) = rtrb::RingBuffer::new(RING_FRAMES * STEREO);
        let (mon_tx, mon_rx) = rtrb::RingBuffer::new(RING_FRAMES * STEREO);
        let shared = Shared {
            rt: Arc::new(Mutex::new(RtState {
                engine,
                out: out_tx,
                monitor: mon_tx,
            })),
            out_rx: Arc::new(Mutex::new(out_rx)),
            monitor_rx: Arc::new(Mutex::new(mon_rx)),
            monitor_on: Arc::default(),
            status: status.clone(),
            counters: counters.clone(),
        };
        let (tx, rx) = std::sync::mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("otozane-coreaudio".into())
            .spawn(move || run(shared, rx, input))
            .expect("failed to spawn Core Audio thread");
        Self {
            tx,
            thread: Some(thread),
            status,
            counters,
        }
    }

    /// Switches the captured microphone. Takes effect asynchronously.
    pub fn set_input(&self, input: Option<String>) {
        let _ = self.tx.send(Command::SetInput(input));
    }

    /// Stops or resumes capture. While muted BlackHole receives silence.
    pub fn set_muted(&self, muted: bool) {
        let _ = self.tx.send(Command::SetMuted(muted));
    }

    /// Plays the processed microphone to the system default output.
    pub fn set_monitor(&self, on: bool) {
        let _ = self.tx.send(Command::SetMonitor(on));
    }
}

impl Drop for Backend {
    fn drop(&mut self) {
        let _ = self.tx.send(Command::Quit);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// A running stream and the device it plays or records.
struct Running {
    uid: String,
    /// Set by the error callback, for example when the device is unplugged.
    failed: Arc<AtomicBool>,
    _stream: cpal::Stream,
}

impl Running {
    fn alive_on(&self, uid: Option<&str>) -> bool {
        Some(self.uid.as_str()) == uid && !self.failed.load(Ordering::Relaxed)
    }
}

struct Main {
    host: cpal::Host,
    shared: Shared,
    desired: Option<String>,
    muted: bool,
    monitoring: bool,
    last_physical_default: Option<String>,
    capture: Option<Running>,
    output: Option<Running>,
    monitor: Option<Running>,
}

fn run(shared: Shared, rx: Receiver<Command>, input: Option<String>) {
    let mut m = Main {
        host: cpal::default_host(),
        shared,
        desired: input,
        muted: false,
        monitoring: false,
        last_physical_default: None,
        capture: None,
        output: None,
        monitor: None,
    };
    let mut next_poll = Instant::now();
    let mut last_cycles = u64::MAX;
    loop {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(Command::SetInput(d)) => m.desired = d,
            Ok(Command::SetMuted(on)) => m.muted = on,
            Ok(Command::SetMonitor(on)) => m.monitoring = on,
            Ok(Command::Quit) | Err(RecvTimeoutError::Disconnected) => return,
            Err(RecvTimeoutError::Timeout) => {
                // Without input callbacks the engine is pumped with empty
                // blocks, so chain updates still apply and old chains are freed.
                let cycles = m.shared.counters.capture_cycles.load(Ordering::Relaxed);
                if cycles == last_cycles
                    && let Ok(mut rt) = m.shared.rt.try_lock()
                {
                    rt.engine.process_interleaved(&mut []);
                }
                last_cycles = cycles;
                if Instant::now() < next_poll {
                    continue;
                }
            }
        }
        m.reconcile();
        next_poll = Instant::now() + Duration::from_secs(1);
    }
}

impl Main {
    /// Makes the streams match the device list, settings and mute state.
    fn reconcile(&mut self) {
        let devices = input_devices(&self.host);
        let default = self
            .host
            .default_input_device()
            .and_then(|d| d.id().ok())
            .map(|id| id.id().to_string());

        if !self
            .output
            .as_ref()
            .is_some_and(|o| o.alive_on(Some(OUTPUT_UID)))
        {
            self.output = None;
            self.output = find(self.host.output_devices(), OUTPUT_UID)
                .and_then(|d| log_err("BlackHole output", create_output(&d, &self.shared)));
        }
        let output_ready = self.output.is_some();

        let target = if self.muted || !output_ready {
            None
        } else if let Some(name) = &self.desired {
            devices
                .iter()
                .find(|d| &d.name == name)
                .map(|d| d.name.clone())
        } else {
            if devices
                .iter()
                .any(|d| !d.is_virtual && Some(&d.name) == default.as_ref())
            {
                self.last_physical_default = default.clone();
            }
            auto_input(
                devices.iter(),
                default.as_deref(),
                self.last_physical_default.as_deref(),
            )
        };

        let capture_state = if self
            .capture
            .as_ref()
            .is_some_and(|c| c.alive_on(target.as_deref()))
        {
            None
        } else {
            // Drop the old stream first, the engine can only be driven by one.
            self.capture = None;
            Some(match (&target, self.muted, output_ready) {
                (_, true, _) => CaptureState::Muted,
                (_, _, false) => CaptureState::Error(
                    "BlackHole 2ch is not installed: brew install blackhole-2ch".into(),
                ),
                (None, ..) => CaptureState::WaitingForDevice(self.desired.clone()),
                (Some(uid), ..) => {
                    match find(self.host.input_devices(), uid)
                        .ok_or_else(|| "device disappeared".to_string())
                        .and_then(|d| create_capture(&d, &self.shared))
                    {
                        Ok(c) => {
                            self.capture = Some(c);
                            CaptureState::Running(uid.clone())
                        }
                        Err(e) => CaptureState::Error(e),
                    }
                }
            })
        };

        let out_uid = self
            .host
            .default_output_device()
            .and_then(|d| d.id().ok())
            .map(|id| id.id().to_string());
        let want_monitor = self.monitoring && out_uid.as_deref() != Some(OUTPUT_UID);
        if !want_monitor
            || !self
                .monitor
                .as_ref()
                .is_some_and(|m| m.alive_on(out_uid.as_deref()))
        {
            self.shared.monitor_on.store(false, Ordering::Relaxed);
            self.monitor = None;
            if want_monitor {
                self.monitor = self
                    .host
                    .default_output_device()
                    .and_then(|d| log_err("monitor", create_monitor(&d, &self.shared)));
                self.shared
                    .monitor_on
                    .store(self.monitor.is_some(), Ordering::Relaxed);
            }
        }

        let mut st = self.shared.status.lock().unwrap();
        st.devices = devices;
        st.virtual_source_ready = output_ready;
        if let Some(s) = capture_state {
            st.capture = s;
        }
    }
}

fn log_err<T>(what: &str, r: Result<T, String>) -> Option<T> {
    r.map_err(|e| eprintln!("{what}: {e}")).ok()
}

fn uid(d: &cpal::Device) -> Option<String> {
    d.id().ok().map(|id| id.id().to_string())
}

fn find(
    devices: Result<impl Iterator<Item = cpal::Device>, cpal::Error>,
    uid_: &str,
) -> Option<cpal::Device> {
    devices.ok()?.find(|d| uid(d).as_deref() == Some(uid_))
}

/// Microphones, without BlackHole itself.
fn input_devices(host: &cpal::Host) -> Vec<Device> {
    let Ok(list) = host.input_devices() else {
        return Vec::new();
    };
    list.filter_map(|d| {
        let name = uid(&d)?;
        if name == OUTPUT_UID {
            return None;
        }
        let description = d
            .description()
            .map(|x| x.name().to_string())
            .unwrap_or_else(|_| name.clone());
        let is_virtual = is_virtual(&name);
        Some(Device {
            name,
            description,
            is_virtual,
        })
    })
    .collect()
}

/// Whether the device with this UID is virtual or aggregate (BlackHole,
/// Voice Enhancer, multi-output devices). "Automatic" skips those.
fn is_virtual(uid: &str) -> bool {
    use objc2_core_audio::*;
    use objc2_core_foundation::CFString;
    use std::ptr::NonNull;
    let get = |object: u32, selector: u32, qualifier: Option<&CFString>| -> Option<u32> {
        let address = AudioObjectPropertyAddress {
            mSelector: selector,
            mScope: kAudioObjectPropertyScopeGlobal,
            mElement: kAudioObjectPropertyElementMain,
        };
        let mut value = 0u32;
        let mut size = size_of::<u32>() as u32;
        let q: *const CFString = qualifier.map_or(std::ptr::null(), |q| q);
        let (q_size, q_ptr) = match qualifier {
            Some(_) => (size_of::<*const CFString>() as u32, (&raw const q).cast()),
            None => (0, std::ptr::null()),
        };
        // SAFETY: both properties are documented to write a UInt32, and the
        // UID translation takes a CFStringRef qualifier that outlives the call.
        let status = unsafe {
            AudioObjectGetPropertyData(
                object,
                NonNull::from(&address),
                q_size,
                q_ptr,
                NonNull::from(&mut size),
                NonNull::from(&mut value).cast(),
            )
        };
        (status == 0).then_some(value)
    };
    let cf = CFString::from_str(uid);
    get(
        kAudioObjectSystemObject as u32,
        kAudioHardwarePropertyTranslateUIDToDevice,
        Some(&*cf),
    )
    .filter(|&id| id != 0)
    .and_then(|id| get(id, kAudioDevicePropertyTransportType, None))
    .is_some_and(|t| {
        t == kAudioDeviceTransportTypeVirtual || t == kAudioDeviceTransportTypeAggregate
    })
}

fn error_flag() -> (Arc<AtomicBool>, impl FnMut(cpal::Error) + Send + 'static) {
    let failed = Arc::new(AtomicBool::new(false));
    let f = failed.clone();
    (failed, move |e| {
        eprintln!("stream error: {e}");
        f.store(true, Ordering::Relaxed);
    })
}

fn play(stream: cpal::Stream, uid: String, failed: Arc<AtomicBool>) -> Result<Running, String> {
    stream.play().map_err(|e| e.to_string())?;
    Ok(Running {
        uid,
        failed,
        _stream: stream,
    })
}

fn create_capture(device: &cpal::Device, shared: &Shared) -> Result<Running, String> {
    let config = device
        .default_input_config()
        .map_err(|e| e.to_string())?
        .config();
    let in_ch = config.channels as usize;
    let mut resampler = Resampler::new(config.sample_rate, RATE);
    let (rt, counters, monitor_on) = (
        shared.rt.clone(),
        shared.counters.clone(),
        shared.monitor_on.clone(),
    );
    let ch = rt.lock().unwrap().engine.channels();
    let mut scratch = vec![0.0f32; SCRATCH_FRAMES * ch];
    let mut faded = 0usize;
    let (failed, on_error) = error_flag();
    let stream = device
        .build_input_stream::<f32, _, _>(
            config,
            move |data, _| {
                let Ok(mut rt) = rt.try_lock() else { return };
                let mut quantum = 0;
                for chunk in data.chunks(CHUNK_FRAMES * in_ch) {
                    let mut frames = chunk
                        .chunks_exact(in_ch)
                        .map(|f| [f[0], f[(in_ch > 1) as usize]]);
                    let mut n = 0;
                    while n < SCRATCH_FRAMES
                        && let Some([l, r]) = resampler.next(|| frames.next())
                    {
                        if ch == 1 {
                            scratch[n] = l;
                        } else {
                            scratch[2 * n] = l;
                            scratch[2 * n + 1] = r;
                        }
                        n += 1;
                    }
                    let samples = &mut scratch[..n * ch];
                    rt.engine.process_interleaved(samples);
                    if faded < FADE_IN_FRAMES {
                        for frame in samples.chunks_exact_mut(ch) {
                            let g = (faded as f32 / FADE_IN_FRAMES as f32).min(1.0);
                            frame.iter_mut().for_each(|s| *s *= g);
                            faded += 1;
                        }
                    }
                    let pushed = push_stereo(&mut rt.out, samples, ch);
                    if pushed < n {
                        counters
                            .overrun_frames
                            .fetch_add((n - pushed) as u64, Ordering::Relaxed);
                    }
                    if monitor_on.load(Ordering::Relaxed) {
                        push_stereo(&mut rt.monitor, samples, ch);
                    }
                    quantum += n;
                }
                counters
                    .capture_quantum
                    .store(quantum as u32, Ordering::Relaxed);
                counters.capture_cycles.fetch_add(1, Ordering::Relaxed);
            },
            on_error,
            None,
        )
        .map_err(|e| e.to_string())?;
    play(stream, uid(device).unwrap_or_default(), failed)
}

/// Plays the output ring into BlackHole at 48 kHz.
fn create_output(device: &cpal::Device, shared: &Shared) -> Result<Running, String> {
    let config = cpal::StreamConfig {
        channels: STEREO as u16,
        sample_rate: RATE,
        buffer_size: cpal::BufferSize::Default,
    };
    let (rx, counters) = (shared.out_rx.clone(), shared.counters.clone());
    let (failed, on_error) = error_flag();
    let stream = device
        .build_output_stream::<f32, _, _>(
            config,
            move |out, _| {
                let frames = out.len() / STEREO;
                counters
                    .playback_quantum
                    .store(frames as u32, Ordering::Relaxed);
                let Ok(mut rx) = rx.try_lock() else {
                    out.fill(0.0);
                    return;
                };
                let q = counters.capture_quantum.load(Ordering::Relaxed) as usize;
                let (got, skipped, left) = pull_stereo(&mut rx, out, frames, q);
                counters
                    .underrun_frames
                    .fetch_add((frames - got) as u64, Ordering::Relaxed);
                counters
                    .skipped_frames
                    .fetch_add(skipped as u64, Ordering::Relaxed);
                counters.ring_fill.store(left as u32, Ordering::Relaxed);
            },
            on_error,
            None,
        )
        .map_err(|e| e.to_string())?;
    play(stream, OUTPUT_UID.into(), failed)
}

/// Plays the monitor ring to `device` at its own rate and channel count.
fn create_monitor(device: &cpal::Device, shared: &Shared) -> Result<Running, String> {
    let config = device
        .default_output_config()
        .map_err(|e| e.to_string())?
        .config();
    let out_ch = config.channels as usize;
    let mut resampler = Resampler::new(RATE, config.sample_rate);
    let (rx, counters) = (shared.monitor_rx.clone(), shared.counters.clone());
    let (failed, on_error) = error_flag();
    let stream = device
        .build_output_stream::<f32, _, _>(
            config,
            move |out, _| {
                let Ok(mut rx) = rx.try_lock() else {
                    out.fill(0.0);
                    return;
                };
                // Audio queued beyond this cycle plus one capture quantum is stale.
                let frames = out.len() / out_ch;
                let need = resampler.input_frames(frames);
                let keep = need + counters.capture_quantum.load(Ordering::Relaxed) as usize;
                let avail = rx.slots() / STEREO;
                if avail > keep + 256
                    && let Ok(c) = rx.read_chunk((avail - keep) * STEREO)
                {
                    c.commit_all();
                }
                let mut pop = || match (rx.pop(), rx.pop()) {
                    (Ok(l), Ok(r)) => [l, r],
                    _ => [0.0; 2],
                };
                for frame in out.chunks_exact_mut(out_ch) {
                    let [l, r] = resampler.next(|| Some(pop())).unwrap_or_default();
                    frame.fill(0.0);
                    frame[0] = l;
                    if out_ch > 1 {
                        frame[1] = r;
                    } else {
                        frame[0] = 0.5 * (l + r);
                    }
                }
            },
            on_error,
            None,
        )
        .map_err(|e| e.to_string())?;
    play(stream, uid(device).unwrap_or_default(), failed)
}

/// Streaming linear-interpolation resampler for stereo frames.
// ponytail: linear interpolation is enough for voice; swap for a windowed-sinc
// resampler (rubato) if aliasing becomes audible on music or high-rate monitoring.
struct Resampler {
    step: f64,
    pos: f64,
    prev: [f32; 2],
    cur: [f32; 2],
}

impl Resampler {
    fn new(from: u32, to: u32) -> Self {
        Self {
            step: from as f64 / to as f64,
            pos: 1.0,
            prev: [0.0; 2],
            cur: [0.0; 2],
        }
    }

    /// Input frames needed to produce `out` frames.
    fn input_frames(&self, out: usize) -> usize {
        (self.pos + out as f64 * self.step).ceil() as usize
    }

    /// Returns the next output frame, pulling input as needed. Returns None
    /// when `pull` runs dry, the state then waits for more input.
    fn next(&mut self, mut pull: impl FnMut() -> Option<[f32; 2]>) -> Option<[f32; 2]> {
        while self.pos >= 1.0 {
            self.prev = self.cur;
            self.cur = pull()?;
            self.pos -= 1.0;
        }
        let t = self.pos as f32;
        let out = [0, 1].map(|i| self.prev[i] + (self.cur[i] - self.prev[i]) * t);
        self.pos += self.step;
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(from: u32, to: u32, input: &[f32]) -> Vec<f32> {
        let mut r = Resampler::new(from, to);
        let mut it = input.iter().map(|&s| [s, s]);
        std::iter::from_fn(|| r.next(|| it.next()).map(|f| f[0])).collect()
    }

    #[test]
    fn same_rate_passes_through_one_frame_late() {
        assert_eq!(run(48_000, 48_000, &[1.0, 2.0, 3.0]), [0.0, 1.0, 2.0]);
    }

    #[test]
    fn upsampling_interpolates_and_keeps_length_ratio() {
        assert_eq!(
            run(16_000, 48_000, &[3.0, 6.0]),
            [0.0, 1.0, 2.0, 3.0, 4.0, 5.0]
        );
        let out = run(44_100, 48_000, &vec![0.0; 44_100]);
        assert!((out.len() as i64 - 48_000).abs() <= 1, "{}", out.len());
    }
}
