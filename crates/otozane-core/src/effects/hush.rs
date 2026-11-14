//! Hush noise and background-speaker suppression, through FFI to the official
//! `libweya_nc` library (DeepFilterNet-based, 16 kHz), loaded at runtime.
//!
//! `weya_nc_process_frame` allocates (tract builds its tensors per call), so
//! inference runs on a worker thread. The audio thread resamples 48 kHz to
//! 16 kHz, sends 10 ms frames over a lock-free queue, and reads results back
//! after a fixed budget of one engine block. A frame that is not back in time
//! is replaced by the dry signal, so latency never changes.

use super::Processor;
use crate::engine::MAX_CHANNELS;
use crate::param::{AtomicF32, ParamSpec};
use std::ffi::{CString, c_char, c_float};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{JoinHandle, Thread};

pub const PARAMS: &[ParamSpec] = &[
    // libweya_nc's atten_lim_db: 100 dB or more means no limit.
    ParamSpec::new("atten_limit", "Attenuation limit", 0.0, 100.0, 100.0, "dB"),
    ParamSpec::new("mix", "Mix", 0.0, 100.0, 100.0, "%"),
];

pub const LIB: &str = if cfg!(target_os = "macos") {
    "libweya_nc.dylib"
} else {
    "libweya_nc.so"
};
pub const MODEL: &str = "advanced_dfnet16k_model_best_onnx.tar.gz";

/// Hush frame: 10 ms at 16 kHz.
const HOP: usize = 160;
/// Hush runs at 16 kHz, a third of the engine rate.
const RATIO: usize = 3;
/// Time the worker gets per frame, in 16 kHz samples (> one 1024-frame engine
/// block). A frame finished in one host callback is only needed in a later one.
const BUDGET: usize = 345;
/// Delay between a sample entering the decimator and leaving the interpolator.
const PIPE: usize = HOP + BUDGET;
/// STFT delay of the model (FFT 320, hop 160), in 16 kHz samples.
const MODEL_DELAY: usize = HOP;
/// Anti-aliasing / anti-imaging FIR length at 48 kHz (odd: linear phase).
const TAPS: usize = 127;
/// Interpolator polyphase branch length.
const BRANCH: usize = TAPS.div_ceil(RATIO);
/// Frames queued in each direction (the pipeline holds at most four).
const QUEUE: usize = 8;

/// Total latency at 48 kHz for a model with `model_delay` 16 kHz samples of delay.
const fn latency(model_delay: usize) -> usize {
    RATIO * (PIPE + model_delay) + TAPS - RATIO
}

pub const LATENCY: usize = latency(MODEL_DELAY);

#[derive(Clone, Copy)]
struct Frame {
    seq: u64,
    data: [[f32; HOP]; MAX_CHANNELS],
}

const EMPTY: Frame = Frame {
    seq: u64::MAX,
    data: [[0.0; HOP]; MAX_CHANNELS],
};

/// Per-channel denoiser run on the worker: `(channel, input, output) -> SNR dB`.
type Denoise = Box<dyn FnMut(usize, &[f32; HOP], &mut [f32; HOP]) -> f32 + Send>;

/// Windowed-sinc low-pass at 7.6 kHz for 48 kHz, unity DC gain.
fn lowpass() -> [f32; TAPS] {
    use std::f64::consts::PI;
    let fc = 7600.0 / 48000.0;
    let mid = (TAPS / 2) as f64;
    let mut h = [0.0f64; TAPS];
    for (i, v) in h.iter_mut().enumerate() {
        let t = i as f64 - mid;
        let sinc = if t == 0.0 {
            2.0 * fc
        } else {
            (2.0 * PI * fc * t).sin() / (PI * t)
        };
        let w = 2.0 * PI * i as f64 / (TAPS - 1) as f64;
        *v = sinc * (0.42 - 0.5 * w.cos() + 0.08 * (2.0 * w).cos());
    }
    let sum: f64 = h.iter().sum();
    h.map(|v| (v / sum) as f32)
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// Sliding window with its history stored twice, so the last `N` samples are
/// always one contiguous slice (oldest first).
struct Window<const N: usize, const N2: usize> {
    buf: [f32; N2],
    pos: usize,
}

impl<const N: usize, const N2: usize> Window<N, N2> {
    fn new() -> Self {
        Self {
            buf: [0.0; N2],
            pos: 0,
        }
    }
    fn push(&mut self, x: f32) {
        self.buf[self.pos] = x;
        self.buf[self.pos + N] = x;
        self.pos = (self.pos + 1) % N;
    }
    fn get(&self) -> &[f32] {
        &self.buf[self.pos..self.pos + N]
    }
}

struct Channel {
    /// 48 kHz input history for the decimator.
    down: Window<TAPS, { 2 * TAPS }>,
    /// 16 kHz processed history for the interpolator.
    up: Window<BRANCH, { 2 * BRANCH }>,
    dry: crate::dsp::Delay,
}

pub struct Hush {
    channels: Vec<Channel>,
    /// Low-pass taps for the decimator.
    h: [f32; TAPS],
    /// Interpolator branches: `RATIO * h`, split by phase and time-reversed.
    poly: [[f32; BRANCH]; RATIO],
    latency: usize,
    /// Position in the 48 kHz <-> 16 kHz cycle.
    phase: usize,
    /// Frame being filled for the worker.
    send: Frame,
    fill: usize,
    /// Next 16 kHz sample to play, counted from the start of the stream.
    /// Starts at `-PIPE`: the first `PIPE` samples are silence.
    play: i64,
    /// Frame being played, `None` if it did not come back in time.
    recv: Option<Frame>,
    /// 48 kHz samples left to replace with the dry signal.
    fallback: usize,
    to_worker: rtrb::Producer<Frame>,
    from_worker: rtrb::Consumer<Frame>,
    worker: Thread,
    join: Option<JoinHandle<()>>,
    stop: Arc<AtomicBool>,
    snr: Arc<AtomicF32>,
    /// Frames sent to and finished by the worker (for tests and diagnostics).
    sent: u64,
    #[cfg_attr(not(test), allow(dead_code))]
    done: Arc<AtomicU64>,
}

impl Hush {
    /// Loads the Hush runtime (once per process) and starts a worker with one
    /// session per channel. Call this off the audio thread.
    pub fn new(channels: usize) -> Result<Self, String> {
        let rt = runtime()?;
        let mut sessions = (0..channels)
            .map(|_| Session::new(&rt))
            .collect::<Result<Vec<_>, _>>()?;
        let denoise: Denoise = Box::new(move |c, input, output| sessions[c].process(input, output));
        Self::with_denoiser(channels, MODEL_DELAY, denoise)
    }

    fn with_denoiser(
        channels: usize,
        model_delay: usize,
        mut denoise: Denoise,
    ) -> Result<Self, String> {
        let (to_worker, mut worker_rx) = rtrb::RingBuffer::<Frame>::new(QUEUE);
        let (mut worker_tx, from_worker) = rtrb::RingBuffer::new(QUEUE);
        let stop = Arc::new(AtomicBool::new(false));
        let snr = Arc::new(AtomicF32::new(0.0));
        let done = Arc::new(AtomicU64::new(0));
        let (stop2, snr2, done2) = (stop.clone(), snr.clone(), done.clone());
        let join = std::thread::Builder::new()
            .name("hush".into())
            .spawn(move || {
                while !stop2.load(Ordering::Acquire) {
                    while let Ok(frame) = worker_rx.pop() {
                        let mut out = Frame {
                            seq: frame.seq,
                            ..EMPTY
                        };
                        let mut best = f32::MIN;
                        for c in 0..channels {
                            best = best.max(denoise(c, &frame.data[c], &mut out.data[c]));
                        }
                        let _ = worker_tx.push(out);
                        snr2.store(best);
                        done2.fetch_add(1, Ordering::Release);
                    }
                    std::thread::park_timeout(std::time::Duration::from_millis(100));
                }
            })
            .map_err(|e| format!("cannot start the Hush thread: {e}"))?;
        let h = lowpass();
        let mut poly = [[0.0; BRANCH]; RATIO];
        for (p, branch) in poly.iter_mut().enumerate() {
            for (j, v) in branch.iter_mut().enumerate() {
                let k = BRANCH - 1 - j;
                *v = h.get(RATIO * k + p).map_or(0.0, |t| t * RATIO as f32);
            }
        }
        let latency = latency(model_delay);
        Ok(Self {
            channels: (0..channels)
                .map(|_| Channel {
                    down: Window::new(),
                    up: Window::new(),
                    dry: crate::dsp::Delay::new(latency),
                })
                .collect(),
            h,
            poly,
            latency,
            phase: 0,
            send: Frame { seq: 0, ..EMPTY },
            fill: 0,
            play: -(PIPE as i64),
            recv: None,
            fallback: 0,
            to_worker,
            from_worker,
            worker: join.thread().clone(),
            join: Some(join),
            stop,
            snr,
            sent: 0,
            done,
        })
    }

    /// Fetches the processed 16 kHz sample `self.play` for each channel.
    fn next_wet(&mut self) -> [f32; MAX_CHANNELS] {
        let pos = self.play;
        self.play += 1;
        if pos < 0 {
            return [0.0; MAX_CHANNELS];
        }
        let (seq, off) = (pos as u64 / HOP as u64, pos as usize % HOP);
        if off == 0 {
            // Drop late frames, then take this one if it is there.
            while self.from_worker.peek().is_ok_and(|f| f.seq < seq) {
                let _ = self.from_worker.pop();
            }
            self.recv = match self.from_worker.peek() {
                Ok(f) if f.seq == seq => self.from_worker.pop().ok(),
                _ => None,
            };
            if self.recv.is_none() {
                self.fallback = RATIO * HOP + TAPS;
            }
        }
        match &self.recv {
            Some(f) => std::array::from_fn(|c| f.data[c][off]),
            None => [0.0; MAX_CHANNELS],
        }
    }
}

impl Processor for Hush {
    fn process(&mut self, channels: &mut [&mut [f32]], p: &[f32]) {
        // The library limits attenuation by blending the delayed noisy signal
        // back in (`enh * (1 - lim) + noisy * lim`), which is a dry/wet mix:
        // done here so it can change without recreating the sessions.
        let lim = if p[0] >= 100.0 {
            0.0
        } else {
            crate::dsp::db_to_lin(-p[0])
        };
        let mix = p[1] / 100.0 * (1.0 - lim);
        let n = channels[0].len();
        for i in 0..n {
            for (ch, st) in channels.iter().zip(self.channels.iter_mut()) {
                st.down.push(ch[i]);
            }
            if self.phase == RATIO - 1 {
                for (c, st) in self.channels.iter().enumerate() {
                    self.send.data[c][self.fill] = dot(&self.h, st.down.get());
                }
                self.fill += 1;
                if self.fill == HOP {
                    self.fill = 0;
                    // A full queue means the worker stalled; that frame plays dry.
                    if self.to_worker.push(self.send).is_ok() {
                        self.sent += 1;
                        self.worker.unpark();
                    }
                    self.send.seq = self.send.seq.wrapping_add(1);
                }
            }
            if self.phase == 0 {
                let wet = self.next_wet();
                for (c, st) in self.channels.iter_mut().enumerate() {
                    st.up.push(wet[c]);
                }
            }
            let fallback = self.fallback > 0;
            self.fallback = self.fallback.saturating_sub(1);
            for (ch, st) in channels.iter_mut().zip(self.channels.iter_mut()) {
                let dry = st.dry.tick(ch[i]);
                let wet = if fallback {
                    dry
                } else {
                    dot(&self.poly[self.phase], st.up.get())
                };
                ch[i] = dry + (wet - dry) * mix;
            }
            self.phase = (self.phase + 1) % RATIO;
        }
    }

    fn latency(&self) -> usize {
        self.latency
    }

    /// Speech-to-noise ratio of the input estimated by the model, in dB.
    fn meter(&self) -> f32 {
        self.snr.load()
    }
}

impl Drop for Hush {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.worker.unpark();
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

mod ffi {
    use super::*;
    #[repr(C)]
    pub struct Model(std::ffi::c_void);
    #[repr(C)]
    pub struct Session(std::ffi::c_void);
    pub type ModelLoad = unsafe extern "C" fn(*const c_char) -> *mut Model;
    pub type ModelFree = unsafe extern "C" fn(*mut Model);
    pub type SessionCreate = unsafe extern "C" fn(*const Model, usize, c_float) -> *mut Session;
    pub type SessionFree = unsafe extern "C" fn(*mut Session);
    pub type Query = unsafe extern "C" fn(*const Session) -> usize;
    pub type Process = unsafe extern "C" fn(*mut Session, *const c_float, *mut c_float) -> c_float;
}

/// The loaded library and model, shared by every Hush instance.
struct Runtime {
    model: *mut ffi::Model,
    model_free: ffi::ModelFree,
    session_create: ffi::SessionCreate,
    session_free: ffi::SessionFree,
    frame_length: ffi::Query,
    sample_rate: ffi::Query,
    process: ffi::Process,
    _lib: libloading::Library,
}

// SAFETY: libweya_nc's model is immutable after loading and documented as
// shareable across threads; sessions are separate objects.
unsafe impl Send for Runtime {}
unsafe impl Sync for Runtime {}

impl Drop for Runtime {
    fn drop(&mut self) {
        // SAFETY: the model came from weya_nc_model_load_from_path and every
        // session holds an Arc to this runtime, so none outlives it.
        unsafe { (self.model_free)(self.model) }
    }
}

/// Directories searched for the library and model, in order.
fn search_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(d) = std::env::var_os("OTOZANE_HUSH_DIR") {
        dirs.push(PathBuf::from(d));
    }
    // Installed: <prefix>/bin/otozaned -> <prefix>/lib/otozane/hush.
    if let Some(bin) = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(PathBuf::from))
    {
        dirs.push(bin.join("../lib/otozane/hush"));
        // macOS app bundle: Contents/MacOS/otozaned -> Contents/Resources/hush.
        dirs.push(bin.join("../Resources/hush"));
    }
    // Development: scripts/build-hush.sh puts it here.
    dirs.push(PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../third_party/hush"
    )));
    dirs
}

fn load(dir: &std::path::Path) -> Result<Runtime, String> {
    // SAFETY: libweya_nc has no load-time initializers with preconditions.
    let lib = unsafe { libloading::Library::new(dir.join(LIB)) }
        .map_err(|e| format!("cannot load {LIB}: {e}"))?;
    macro_rules! sym {
        ($name:literal) => {
            // SAFETY: the type matches the declaration in weya_nc.h.
            *unsafe { lib.get(concat!($name, "\0").as_bytes()) }
                .map_err(|e| format!("{LIB} lacks {}: {e}", $name))?
        };
    }
    let model_load: ffi::ModelLoad = sym!("weya_nc_model_load_from_path");
    let mut rt = Runtime {
        model: std::ptr::null_mut(),
        model_free: sym!("weya_nc_model_free"),
        session_create: sym!("weya_nc_session_create"),
        session_free: sym!("weya_nc_session_free"),
        frame_length: sym!("weya_nc_get_frame_length"),
        sample_rate: sym!("weya_nc_get_sample_rate"),
        process: sym!("weya_nc_process_frame"),
        _lib: lib,
    };
    let path = CString::new(dir.join(MODEL).into_os_string().into_encoded_bytes())
        .map_err(|_| "invalid model path".to_string())?;
    // SAFETY: valid NUL-terminated path; returns null on failure.
    rt.model = unsafe { model_load(path.as_ptr()) };
    if rt.model.is_null() {
        // Nothing to free; keep Drop from freeing null.
        rt.model_free = noop_free;
        return Err(format!("cannot load the Hush model {MODEL}"));
    }
    Ok(rt)
}

unsafe extern "C" fn noop_free(_: *mut ffi::Model) {}

/// Loads the runtime on first use. Failures are not cached, so installing the
/// files later only needs the effect to be added again.
fn runtime() -> Result<Arc<Runtime>, String> {
    static RUNTIME: Mutex<Option<Arc<Runtime>>> = Mutex::new(None);
    let mut slot = RUNTIME.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(rt) = slot.as_ref() {
        return Ok(rt.clone());
    }
    let dirs = search_dirs();
    let dir = dirs
        .iter()
        .find(|d| d.join(LIB).is_file() && d.join(MODEL).is_file())
        .ok_or_else(|| {
            let list: Vec<_> = dirs.iter().map(|d| d.display().to_string()).collect();
            format!(
                "Hush is not installed ({LIB} and model not found in {})",
                list.join(", ")
            )
        })?;
    let rt = Arc::new(load(dir)?);
    *slot = Some(rt.clone());
    Ok(rt)
}

/// One streaming session (one channel). Used only on the worker thread.
struct Session {
    ptr: *mut ffi::Session,
    rt: Arc<Runtime>,
}

// SAFETY: a session may be used from any thread, one at a time.
unsafe impl Send for Session {}

impl Session {
    fn new(rt: &Arc<Runtime>) -> Result<Self, String> {
        // SAFETY: valid model; 16 kHz input means no resampling inside the
        // library; 100 dB is "no attenuation limit" (the mix is ours).
        let ptr = unsafe { (rt.session_create)(rt.model, 16_000, 100.0) };
        if ptr.is_null() {
            return Err("cannot create a Hush session".into());
        }
        let s = Self {
            ptr,
            rt: rt.clone(),
        };
        // SAFETY: valid session.
        let (len, sr) = unsafe { ((rt.frame_length)(s.ptr), (rt.sample_rate)(s.ptr)) };
        if len != HOP || sr != 16_000 {
            return Err(format!("unsupported Hush model: {len} samples at {sr} Hz"));
        }
        Ok(s)
    }

    fn process(&mut self, input: &[f32; HOP], output: &mut [f32; HOP]) -> f32 {
        // SAFETY: both buffers hold exactly the session's frame length.
        unsafe { (self.rt.process)(self.ptr, input.as_ptr(), output.as_mut_ptr()) }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // SAFETY: created by session_create, freed once, runtime still alive.
        unsafe { (self.rt.session_free)(self.ptr) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity() -> Denoise {
        Box::new(|_, i, o| {
            *o = *i;
            0.0
        })
    }

    /// Two tones inside the 16 kHz band, at 48 kHz.
    fn tones(n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| {
                let t = i as f32 / 48_000.0;
                0.3 * (2.0 * std::f32::consts::PI * 440.0 * t).sin()
                    + 0.2 * (2.0 * std::f32::consts::PI * 3100.0 * t).sin()
            })
            .collect()
    }

    fn noise(n: usize, amp: f32) -> Vec<f32> {
        let mut seed = 0x1234_5678u32;
        (0..n)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                (seed as f32 / u32::MAX as f32 * 2.0 - 1.0) * amp
            })
            .collect()
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
    }

    impl Hush {
        /// Waits for the worker, standing in for real time between callbacks.
        fn settle(&self) {
            while self.done.load(Ordering::Acquire) < self.sent {
                std::thread::yield_now();
            }
        }
    }

    /// Processes planar `input` in blocks of the given sizes (cycled), letting
    /// the worker catch up between blocks like a real-time host would.
    fn run(h: &mut Hush, input: &[Vec<f32>], blocks: &[usize], mix: f32) -> Vec<Vec<f32>> {
        let mut out = input.to_vec();
        let (mut pos, mut b) = (0, 0);
        while pos < out[0].len() {
            let n = blocks[b % blocks.len()].min(out[0].len() - pos);
            let mut planes: Vec<&mut [f32]> =
                out.iter_mut().map(|c| &mut c[pos..pos + n]).collect();
            h.process(&mut planes, &[100.0, mix]);
            h.settle();
            pos += n;
            b += 1;
        }
        out
    }

    #[test]
    fn resampling_round_trip_matches_reported_latency() {
        let mut h = Hush::with_denoiser(1, 0, identity()).unwrap();
        let lat = h.latency();
        assert_eq!(lat, latency(0));
        let input = tones(48_000);
        let out = run(&mut h, std::slice::from_ref(&input), &[256], 100.0);
        let err = (lat + 2000..input.len())
            .map(|i| (out[0][i] - input[i - lat]).abs())
            .fold(0.0f32, f32::max);
        assert!(err < 0.01, "max error {err}");
    }

    #[test]
    fn output_does_not_depend_on_block_size() {
        let input = tones(30_000);
        let reference = {
            let mut h = Hush::with_denoiser(1, 0, identity()).unwrap();
            run(&mut h, std::slice::from_ref(&input), &[480], 100.0)
        };
        for blocks in [&[1usize][..], &[7, 129, 1024], &[1024], &[333, 64, 1000, 5]] {
            let mut h = Hush::with_denoiser(1, 0, identity()).unwrap();
            let out = run(&mut h, std::slice::from_ref(&input), blocks, 100.0);
            assert_eq!(out, reference, "blocks {blocks:?}");
        }
    }

    #[test]
    fn channels_are_processed_independently() {
        // Channel 1 is silenced by its denoiser, channel 0 passes.
        let denoise: Denoise = Box::new(|c, i, o| {
            *o = if c == 0 { *i } else { [0.0; HOP] };
            0.0
        });
        let mut h = Hush::with_denoiser(2, 0, denoise).unwrap();
        let input = vec![tones(20_000), tones(20_000)];
        let out = run(&mut h, &input, &[512], 100.0);
        let tail = 10_000..20_000;
        assert!(rms(&out[0][tail.clone()]) > 0.2);
        assert!(rms(&out[1][tail]) < 1e-6);
    }

    #[test]
    fn zero_mix_is_dry_signal_delayed_by_latency() {
        let mut h = Hush::with_denoiser(
            1,
            MODEL_DELAY,
            Box::new(|_, _, o| {
                *o = [0.0; HOP];
                0.0
            }),
        )
        .unwrap();
        let input = noise(20_000, 0.5);
        let out = run(&mut h, std::slice::from_ref(&input), &[300], 0.0);
        for i in LATENCY..input.len() {
            assert_eq!(out[0][i], input[i - LATENCY]);
        }
    }

    #[test]
    fn attenuation_limit_keeps_that_much_of_the_dry_signal() {
        let silence: Denoise = Box::new(|_, _, o| {
            *o = [0.0; HOP];
            0.0
        });
        let mut h = Hush::with_denoiser(1, MODEL_DELAY, silence).unwrap();
        let input = noise(20_000, 0.5);
        let mut out = input.clone();
        for chunk in out.chunks_mut(480) {
            // 20 dB limit: a fully suppressed signal stays at 0.1 of the input.
            h.process(&mut [chunk], &[20.0, 100.0]);
            h.settle();
        }
        for i in LATENCY..input.len() {
            assert!(
                (out[i] - 0.1 * input[i - LATENCY]).abs() < 1e-6,
                "sample {i}"
            );
        }
    }

    #[test]
    fn late_frames_play_dry_without_changing_latency() {
        // A worker that never keeps up: every frame misses its deadline.
        let slow: Denoise = Box::new(|_, i, o| {
            std::thread::sleep(std::time::Duration::from_millis(30));
            *o = *i;
            0.0
        });
        let mut h = Hush::with_denoiser(1, MODEL_DELAY, slow).unwrap();
        let input = noise(20_000, 0.5);
        let mut out = input.clone();
        for chunk in out.chunks_mut(1024) {
            h.process(&mut [chunk], &[100.0, 100.0]);
        }
        for i in LATENCY..input.len() {
            assert_eq!(out[i], input[i - LATENCY], "sample {i}");
        }
    }

    #[test]
    fn missing_runtime_is_an_error() {
        let dir = std::env::temp_dir().join("otozane-hush-missing");
        assert!(load(&dir).is_err());
    }

    /// Real model tests; they skip when `scripts/build-hush.sh` has not been run.
    fn real(channels: usize) -> Option<Hush> {
        match Hush::new(channels) {
            Ok(h) => Some(h),
            Err(e) => {
                eprintln!("skipping: {e}");
                None
            }
        }
    }

    #[test]
    fn model_suppresses_white_noise() {
        let Some(mut h) = real(1) else { return };
        assert_eq!(h.latency(), LATENCY);
        let input = noise(96_000, 0.1);
        let out = run(&mut h, std::slice::from_ref(&input), &[256], 100.0);
        let (a, b) = (rms(&input[48_000..]), rms(&out[0][48_000..]));
        assert!(b < a * 0.1, "noise rms {a} -> {b}");
        assert!(h.meter().is_finite());
    }

    /// `cargo test --release -p otozane-core hush::tests::bench -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn bench() {
        for channels in 1..=2 {
            let Some(mut h) = real(channels) else { return };
            let seconds = 20;
            let mut bufs = vec![noise(48_000 * seconds, 0.1); channels];
            // One 10 ms block at a time, waiting for the worker each time.
            let mut worst = std::time::Duration::ZERO;
            let start = std::time::Instant::now();
            for i in (0..bufs[0].len()).step_by(480) {
                let t = std::time::Instant::now();
                let mut copy: Vec<Vec<f32>> = bufs.iter().map(|c| c[i..i + 480].to_vec()).collect();
                let mut planes: Vec<&mut [f32]> =
                    copy.iter_mut().map(|c| c.as_mut_slice()).collect();
                h.process(&mut planes, &[100.0, 100.0]);
                h.settle();
                worst = worst.max(t.elapsed());
            }
            let total = start.elapsed();
            // Audio-thread cost alone, without waiting for the worker.
            let start = std::time::Instant::now();
            for i in (0..bufs[0].len()).step_by(480) {
                let mut planes: Vec<&mut [f32]> =
                    bufs.iter_mut().map(|c| &mut c[i..i + 480]).collect();
                h.process(&mut planes, &[100.0, 100.0]);
            }
            let rt = start.elapsed();
            eprintln!(
                "{channels} ch: {:.1}% of one core in total, audio thread {:.2}%, worst 10 ms block {worst:?}",
                total.as_secs_f64() / seconds as f64 * 100.0,
                rt.as_secs_f64() / seconds as f64 * 100.0
            );
        }
    }

    #[test]
    fn model_respects_attenuation_limit() {
        let Some(mut h) = real(1) else { return };
        let input = noise(96_000, 0.1);
        let mut out = input.clone();
        for chunk in out.chunks_mut(480) {
            h.process(&mut [chunk], &[12.0, 100.0]);
            h.settle();
        }
        let ratio = rms(&out[48_000..]) / rms(&input[48_000..]);
        // 12 dB is a factor of 0.25; the model removes nearly all the rest.
        assert!((0.24..0.3).contains(&ratio), "ratio {ratio}");
    }

    #[test]
    fn model_keeps_stereo_channels_and_any_block_size() {
        let Some(mut h) = real(2) else { return };
        let input = vec![noise(48_000, 0.1), vec![0.0; 48_000]];
        let out = run(&mut h, &input, &[1, 480, 1024, 77], 100.0);
        assert!(out.iter().flatten().all(|s| s.is_finite()));
        assert!(out[1].iter().all(|&s| s == 0.0));
    }
}
