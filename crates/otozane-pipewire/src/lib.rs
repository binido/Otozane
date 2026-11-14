//! PipeWire backend: discovers microphones, captures the selected one, runs
//! the engine on the capture thread, and publishes the result as the
//! `Otozane Microphone` virtual source.
//!
//! Two `pw_stream`s are used, the same shape as PipeWire's `module-loopback`:
//! a capture stream that always targets an explicit node, and a playback
//! stream that feeds a `support.null-audio-sink` node exposed with
//! `media.class = Audio/Source/Virtual`, which applications record from.
//! A lock-free SPSC ring buffer carries processed audio between the streams.
//! While monitoring is on, a third stream, `Otozane Monitor`, plays the same
//! processed audio from a second ring to the system default output.

#![cfg(target_os = "linux")]

use otozane_core::SAMPLE_RATE;
pub use otozane_core::backend::{CaptureState, Counters, Device, Status};
use otozane_core::backend::{auto_input, pull_stereo, push_stereo};
use otozane_core::engine::Engine;
use pipewire as pw;
use pw::properties::properties;
use pw::spa;
use pw::stream::{StreamFlags, StreamListener, StreamRc, StreamState};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

/// `node.name` of the virtual microphone. Never offered as an input.
/// Node names, prefixed with `$OTOZANE_NODE_PREFIX` when set, so a test
/// service can run next to a real one without either picking up the other's nodes.
static NAMES: std::sync::LazyLock<[String; 5]> = std::sync::LazyLock::new(|| {
    let p = std::env::var("OTOZANE_NODE_PREFIX").unwrap_or_default();
    let desc = if p.is_empty() {
        "Otozane Microphone".to_string()
    } else {
        format!("Otozane Microphone ({p})")
    };
    [
        format!("{p}otozane_microphone"),
        desc,
        format!("{p}otozane_capture"),
        format!("{p}otozane_output"),
        format!("{p}otozane_monitor"),
    ]
});

pub fn virtual_node_name() -> &'static str {
    &NAMES[0]
}
fn virtual_node_description() -> &'static str {
    &NAMES[1]
}
fn capture_node_name() -> &'static str {
    &NAMES[2]
}
fn output_node_name() -> &'static str {
    &NAMES[3]
}
fn monitor_node_name() -> &'static str {
    &NAMES[4]
}
const OUTPUT_CHANNELS: usize = 2;
/// Fade-in applied when a capture stream starts (20 ms).
const FADE_IN_FRAMES: usize = 960;
/// Output ring capacity (0.5 s). Excess is discarded, so this bounds memory, not latency.
const RING_FRAMES: usize = 24_000;

enum Command {
    SetInput(Option<String>),
    SetMuted(bool),
    SetMonitor(bool),
    Quit,
}

/// State touched by the capture callback. The callback only uses
/// `try_lock`, so it never blocks; the lock is otherwise uncontended because
/// at most one capture stream exists at a time.
struct RtState {
    engine: Engine,
    out: rtrb::Producer<f32>,
    /// Feeds the monitor stream; written only while `Shared::monitor_on`.
    monitor: rtrb::Producer<f32>,
}

type SharedSender = Arc<Mutex<Option<pw::channel::Sender<Command>>>>;

pub struct Backend {
    tx: SharedSender,
    thread: Option<JoinHandle<()>>,
    pub status: Arc<Mutex<Status>>,
    pub counters: Arc<Counters>,
}

impl Backend {
    /// Starts the PipeWire thread. `input` is the `node.name` of the
    /// microphone to capture, or `None` to follow the system default.
    pub fn start(engine: Engine, input: Option<String>) -> Self {
        let status = Arc::new(Mutex::new(Status::default()));
        let counters = Arc::new(Counters::default());
        let tx: SharedSender = Arc::default();
        let (out_tx, out_rx) = rtrb::RingBuffer::new(RING_FRAMES * OUTPUT_CHANNELS);
        let (mon_tx, mon_rx) = rtrb::RingBuffer::new(RING_FRAMES * OUTPUT_CHANNELS);
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
        let tx2 = tx.clone();
        let thread = std::thread::Builder::new()
            .name("otozane-pipewire".into())
            .spawn(move || {
                pw::init();
                let mut input = input;
                let mut muted = false;
                let mut monitoring = false;
                loop {
                    let (sender, rx) = pw::channel::channel();
                    *tx2.lock().unwrap() = Some(sender);
                    match run(&shared, rx, &mut input, &mut muted, &mut monitoring) {
                        Ok(true) => return,
                        Ok(false) => {}
                        Err(e) => shared.set_capture(CaptureState::Error(e.to_string())),
                    }
                    // Lost the PipeWire connection (e.g. daemon restart); retry.
                    shared.status.lock().unwrap().virtual_source_ready = false;
                    std::thread::sleep(Duration::from_secs(1));
                }
            })
            .expect("failed to spawn PipeWire thread");
        Self {
            tx,
            thread: Some(thread),
            status,
            counters,
        }
    }

    /// Switches the captured microphone. Takes effect asynchronously.
    pub fn set_input(&self, input: Option<String>) {
        self.send(Command::SetInput(input));
    }

    /// Stops or resumes capture. While muted the virtual microphone stays
    /// present and outputs silence. Takes effect asynchronously.
    pub fn set_muted(&self, muted: bool) {
        self.send(Command::SetMuted(muted));
    }

    /// Plays the processed microphone to the system default output.
    /// Takes effect asynchronously.
    pub fn set_monitor(&self, on: bool) {
        self.send(Command::SetMonitor(on));
    }

    fn send(&self, cmd: Command) {
        if let Some(tx) = self.tx.lock().unwrap().as_ref() {
            let _ = tx.send(cmd);
        }
    }
}

impl Drop for Backend {
    fn drop(&mut self) {
        self.send(Command::Quit);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

#[derive(Clone)]
struct Shared {
    rt: Arc<Mutex<RtState>>,
    out_rx: Arc<Mutex<rtrb::Consumer<f32>>>,
    monitor_rx: Arc<Mutex<rtrb::Consumer<f32>>>,
    /// Whether the capture callback feeds the monitor ring. Kept off while no
    /// monitor stream drains it, so the ring never holds stale audio.
    monitor_on: Arc<AtomicBool>,
    status: Arc<Mutex<Status>>,
    counters: Arc<Counters>,
}

impl Shared {
    fn set_capture(&self, state: CaptureState) {
        self.status.lock().unwrap().capture = state;
    }
}

struct Capture {
    target: String,
    _stream: StreamRc,
    _listener: StreamListener<()>,
}

/// Main-loop state. Only accessed from the PipeWire thread.
struct Main {
    core: pw::core::CoreRc,
    shared: Shared,
    desired: Option<String>,
    muted: bool,
    devices: Vec<(u32, Device)>,
    /// System default source (`default.audio.source` metadata), if known.
    default_source: Option<String>,
    /// Last default source that was a physical device.
    last_physical_default: Option<String>,
    _metadata: Option<(pw::metadata::Metadata, pw::metadata::MetadataListener)>,
    capture: Option<Capture>,
    monitoring: bool,
    monitor: Option<(StreamRc, StreamListener<()>)>,
    /// Global ids of our feed stream node and our virtual microphone node.
    output_node: Option<u32>,
    virtual_node: Option<u32>,
    ports: Vec<Port>,
    /// Links from the feed stream to the virtual microphone. WirePlumber does
    /// not link a playback stream into a source node, so we do it ourselves.
    links: Vec<pw::link::Link>,
}

/// Node name from a `default.audio.source` value: `{"name":"..."}`.
fn default_name(value: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(value).ok()?;
    Some(v.get("name")?.as_str()?.to_string())
}

struct Port {
    id: u32,
    node: u32,
    output: bool,
    channel: String,
}

impl Main {
    fn link_output(&mut self) {
        let (Some(out), Some(vsrc)) = (self.output_node, self.virtual_node) else {
            return;
        };
        if !self.links.is_empty() {
            return;
        }
        let find = |node, output, ch: &str| {
            self.ports
                .iter()
                .find(|p| p.node == node && p.output == output && p.channel == ch)
                .map(|p| p.id)
        };
        let mut pairs = Vec::new();
        for ch in ["FL", "FR"] {
            match (find(out, true, ch), find(vsrc, false, ch)) {
                (Some(o), Some(i)) => pairs.push((o, i)),
                _ => return, // ports not announced yet
            }
        }
        for (o, i) in pairs {
            let link = self.core.create_object::<pw::link::Link>(
                "link-factory",
                &properties! {
                    "link.output.node" => out.to_string(),
                    "link.output.port" => o.to_string(),
                    "link.input.node" => vsrc.to_string(),
                    "link.input.port" => i.to_string(),
                    "object.linger" => "false",
                },
            );
            match link {
                Ok(l) => self.links.push(l),
                Err(e) => eprintln!("failed to link output to virtual microphone: {e}"),
            }
        }
    }

    /// Creates or drops the monitor stream to match `monitoring`.
    fn reconcile_monitor(&mut self) {
        if self.monitoring == self.monitor.is_some() {
            return;
        }
        self.shared.monitor_on.store(false, Ordering::Relaxed);
        self.monitor = None;
        if self.monitoring {
            match create_monitor(&self.core, &self.shared) {
                Ok(m) => {
                    self.monitor = Some(m);
                    self.shared.monitor_on.store(true, Ordering::Relaxed);
                }
                Err(e) => eprintln!("failed to create monitor stream: {e}"),
            }
        }
    }

    fn publish_devices(&self) {
        self.shared.status.lock().unwrap().devices =
            self.devices.iter().map(|(_, d)| d.clone()).collect();
    }

    /// Makes the capture stream match the desired device and current device list.
    fn reconcile(&mut self) {
        if self.muted {
            self.capture = None;
            self.shared.set_capture(CaptureState::Muted);
            return;
        }
        let target = match &self.desired {
            Some(name) => self
                .devices
                .iter()
                .find(|(_, d)| &d.name == name)
                .map(|(_, d)| d.name.clone()),
            None => {
                let d = self.default_source.as_deref();
                if self
                    .devices
                    .iter()
                    .any(|(_, x)| !x.is_virtual && Some(x.name.as_str()) == d)
                {
                    self.last_physical_default = d.map(str::to_string);
                }
                auto_input(
                    self.devices.iter().map(|(_, d)| d),
                    d,
                    self.last_physical_default.as_deref(),
                )
            }
        };
        if self.capture.as_ref().map(|c| &c.target) == target.as_ref() {
            return;
        }
        // Drop the old stream first: the engine can only be driven by one.
        self.capture = None;
        let Some(target) = target else {
            self.shared
                .set_capture(CaptureState::WaitingForDevice(self.desired.clone()));
            return;
        };
        let channels = self.shared.rt.lock().unwrap().engine.channels();
        self.shared
            .set_capture(CaptureState::Connecting(target.clone()));
        match create_capture(&self.core, &self.shared, &target, channels) {
            Ok(c) => self.capture = Some(c),
            Err(e) => self.shared.set_capture(CaptureState::Error(e.to_string())),
        }
    }
}

/// Runs one PipeWire connection. Returns `Ok(true)` when asked to quit and
/// `Ok(false)` when the connection was lost.
fn run(
    shared: &Shared,
    rx: pw::channel::Receiver<Command>,
    input: &mut Option<String>,
    muted: &mut bool,
    monitoring: &mut bool,
) -> Result<bool, pw::Error> {
    let mainloop = pw::main_loop::MainLoopRc::new(None)?;
    let context = pw::context::ContextRc::new(
        &mainloop,
        Some(properties! { *pw::keys::APP_NAME => "Otozane" }),
    )?;
    let core = context.connect_rc(None)?;
    let quit = Rc::new(Cell::new(false));

    let ml = mainloop.clone();
    let _core_listener = core
        .add_listener_local()
        .error(move |id, _seq, _res, msg| {
            if id == pw::core::PW_ID_CORE {
                eprintln!("PipeWire core error: {msg}");
                ml.quit();
            }
        })
        .register();

    let (_virtual_node, _playback, _playback_listener) = create_playback(&core, shared)?;

    let state = Rc::new(RefCell::new(Main {
        core: core.clone(),
        shared: shared.clone(),
        desired: input.clone(),
        muted: *muted,
        devices: Vec::new(),
        default_source: None,
        last_physical_default: None,
        _metadata: None,
        capture: None,
        monitoring: *monitoring,
        monitor: None,
        output_node: None,
        virtual_node: None,
        ports: Vec::new(),
        links: Vec::new(),
    }));
    state.borrow_mut().reconcile_monitor();

    let registry = core.get_registry_rc()?;
    let (s1, s2) = (state.clone(), state.clone());
    let reg = registry.clone();
    let weak = Rc::downgrade(&state);
    let _registry_listener = registry
        .add_listener_local()
        .global(move |obj| {
            let Some(props) = obj.props else { return };
            if obj.type_ == pw::types::ObjectType::Metadata
                && props.get("metadata.name") == Some("default")
            {
                let Ok(md) = reg.bind::<pw::metadata::Metadata, _>(obj) else {
                    return;
                };
                let weak = weak.clone();
                let listener = md
                    .add_listener_local()
                    .property(move |subject, key, _, value| {
                        if subject == 0
                            && key.is_none_or(|k| k == "default.audio.source")
                            && let Some(st) = weak.upgrade()
                        {
                            let mut st = st.borrow_mut();
                            st.default_source = value.and_then(default_name);
                            st.reconcile();
                        }
                        0
                    })
                    .register();
                s1.borrow_mut()._metadata = Some((md, listener));
                return;
            }
            if obj.type_ == pw::types::ObjectType::Port {
                let node = props.get(*pw::keys::NODE_ID).and_then(|v| v.parse().ok());
                let (Some(node), Some(channel)) = (node, props.get(*pw::keys::AUDIO_CHANNEL))
                else {
                    return;
                };
                if props.get(*pw::keys::PORT_MONITOR) == Some("true") {
                    return;
                }
                let mut st = s1.borrow_mut();
                st.ports.push(Port {
                    id: obj.id,
                    node,
                    output: props.get(*pw::keys::PORT_DIRECTION) == Some("out"),
                    channel: channel.to_string(),
                });
                st.link_output();
                return;
            }
            if obj.type_ != pw::types::ObjectType::Node {
                return;
            }
            let class = props.get(*pw::keys::MEDIA_CLASS).unwrap_or_default();
            let Some(name) = props.get(*pw::keys::NODE_NAME) else {
                return;
            };
            if name == output_node_name() || name == virtual_node_name() {
                let mut st = s1.borrow_mut();
                if name == output_node_name() {
                    st.output_node = Some(obj.id);
                } else {
                    st.virtual_node = Some(obj.id);
                }
                st.link_output();
                return;
            }
            // Never list our own virtual source: capturing it would feed back.
            if !matches!(class, "Audio/Source" | "Audio/Source/Virtual")
                || name == virtual_node_name()
            {
                return;
            }
            let description = props
                .get(*pw::keys::NODE_DESCRIPTION)
                .or_else(|| props.get(*pw::keys::NODE_NICK))
                .unwrap_or(name);
            let mut st = s1.borrow_mut();
            st.devices.push((
                obj.id,
                Device {
                    name: name.to_string(),
                    description: description.to_string(),
                    is_virtual: class == "Audio/Source/Virtual",
                },
            ));
            st.publish_devices();
            st.reconcile();
        })
        .global_remove(move |id| {
            let mut st = s2.borrow_mut();
            st.ports.retain(|p| p.id != id && p.node != id);
            if st.output_node == Some(id) || st.virtual_node == Some(id) {
                st.links.clear();
                st.output_node = st.output_node.filter(|&n| n != id);
                st.virtual_node = st.virtual_node.filter(|&n| n != id);
            }
            let before = st.devices.len();
            st.devices.retain(|(i, _)| *i != id);
            if st.devices.len() != before {
                st.publish_devices();
                st.reconcile();
            }
        })
        .register();

    let (ml, s3, q) = (mainloop.clone(), state.clone(), quit.clone());
    let _rx = rx.attach(mainloop.loop_(), move |cmd| match cmd {
        Command::SetInput(name) => {
            let mut st = s3.borrow_mut();
            st.desired = name;
            st.reconcile();
        }
        Command::SetMuted(m) => {
            let mut st = s3.borrow_mut();
            st.muted = m;
            st.reconcile();
        }
        Command::SetMonitor(on) => {
            let mut st = s3.borrow_mut();
            st.monitoring = on;
            st.reconcile_monitor();
        }
        Command::Quit => {
            q.set(true);
            ml.quit();
        }
    });

    // While no capture callback runs (no device, or stream not started), the
    // engine is pumped with empty blocks so chain updates are still applied
    // and retired chains returned. The pump only runs after the callback has
    // been idle for a full timer period, so it does not contend with audio.
    let (pump, last) = (shared.clone(), Cell::new(u64::MAX));
    let timer = mainloop.loop_().add_timer(move |_| {
        let cycles = pump.counters.capture_cycles.load(Ordering::Relaxed);
        if cycles == last.get()
            && let Ok(mut rt) = pump.rt.try_lock()
        {
            rt.engine.process_interleaved(&mut []);
        }
        last.set(cycles);
    });
    let period = Some(Duration::from_millis(100));
    let _ = timer.update_timer(period, period);

    mainloop.run();

    let mut st = state.borrow_mut();
    *input = st.desired.clone();
    *muted = st.muted;
    *monitoring = st.monitoring;
    st.capture = None;
    st.monitor = None;
    shared.monitor_on.store(false, Ordering::Relaxed);
    Ok(quit.get())
}

fn format_param(channels: usize) -> Vec<u8> {
    let mut info = spa::param::audio::AudioInfoRaw::new();
    info.set_format(spa::param::audio::AudioFormat::F32LE);
    info.set_rate(SAMPLE_RATE);
    info.set_channels(channels as u32);
    let mut position = [0; spa::param::audio::MAX_CHANNELS];
    if channels == 1 {
        position[0] = spa::sys::SPA_AUDIO_CHANNEL_MONO;
    } else {
        position[0] = spa::sys::SPA_AUDIO_CHANNEL_FL;
        position[1] = spa::sys::SPA_AUDIO_CHANNEL_FR;
    }
    info.set_position(position);
    let obj = spa::pod::Object {
        type_: spa::utils::SpaTypes::ObjectParamFormat.as_raw(),
        id: spa::param::ParamType::EnumFormat.as_raw(),
        properties: info.into(),
    };
    spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &spa::pod::Value::Object(obj),
    )
    .expect("serializing a fixed audio format cannot fail")
    .0
    .into_inner()
}

/// Returns the mapped `f32` samples of the first data plane, or `None` if the
/// buffer is unmapped or misaligned. `use_chunk` limits the slice to the
/// chunk's valid region (capture); otherwise the full capacity is returned.
fn samples(data: &mut spa::buffer::Data, use_chunk: bool) -> Option<&mut [f32]> {
    let (offset, size) = (data.chunk().offset() as usize, data.chunk().size() as usize);
    let bytes = data.data()?;
    let bytes = if use_chunk {
        let start = offset.min(bytes.len());
        let end = (start + size).min(bytes.len());
        &mut bytes[start..end]
    } else {
        bytes
    };
    // SAFETY: any bit pattern is a valid f32; `align_to_mut` only yields the
    // correctly aligned middle part, and we reject buffers that have a prefix.
    let (pre, s, _) = unsafe { bytes.align_to_mut::<f32>() };
    pre.is_empty().then_some(s)
}

fn create_capture(
    core: &pw::core::CoreRc,
    shared: &Shared,
    target: &str,
    channels: usize,
) -> Result<Capture, pw::Error> {
    let stream = StreamRc::new(
        core.clone(),
        "otozane-capture",
        properties! {
            *pw::keys::MEDIA_TYPE => "Audio",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::NODE_NAME => capture_node_name(),
            *pw::keys::NODE_DESCRIPTION => "Otozane Capture",
            *pw::keys::TARGET_OBJECT => target,
            // Never fall back to the default source (which could be our own
            // virtual microphone) or get moved elsewhere when the target goes away.
            "node.dont-fallback" => "true",
            *pw::keys::NODE_DONT_RECONNECT => "true",
            // External tools (e.g. EasyEffects "process all inputs") retarget
            // streams through metadata; this capture must stay on its device.
            "node.dont-move" => "true",
            // Cap the graph quantum at one engine block (PipeWire may otherwise
            // raise it to max-quantum, 2048 by default). Hush relies on this:
            // its worker gets one block of time per frame.
            *pw::keys::NODE_LATENCY => "1024/48000",
        },
    )?;
    let (rt, counters, st, name, monitor_on) = (
        shared.rt.clone(),
        shared.counters.clone(),
        shared.clone(),
        target.to_string(),
        shared.monitor_on.clone(),
    );
    // Frames faded in so far: a new stream (unmute, device switch) starts
    // from silence instead of a click.
    let mut faded = 0usize;
    let listener = stream
        .add_local_listener::<()>()
        .state_changed(move |_, _, _, new| match new {
            StreamState::Streaming => st.set_capture(CaptureState::Running(name.clone())),
            StreamState::Error(e) => st.set_capture(CaptureState::Error(e)),
            _ => {}
        })
        .process(move |stream, _| {
            let Some(mut buffer) = stream.dequeue_buffer() else {
                return;
            };
            let Some(data) = buffer.datas_mut().first_mut() else {
                return;
            };
            let Some(samples) = samples(data, true) else {
                return;
            };
            let frames = samples.len() / channels;
            let samples = &mut samples[..frames * channels];
            counters
                .capture_quantum
                .store(frames as u32, Ordering::Relaxed);
            let Ok(mut rt) = rt.try_lock() else { return };
            rt.engine.process_interleaved(samples);
            counters.capture_cycles.fetch_add(1, Ordering::Relaxed);
            if faded < FADE_IN_FRAMES {
                for frame in samples.chunks_exact_mut(channels) {
                    let g = (faded as f32 / FADE_IN_FRAMES as f32).min(1.0);
                    frame.iter_mut().for_each(|s| *s *= g);
                    faded += 1;
                }
            }

            let n = push_stereo(&mut rt.out, samples, channels);
            if n < frames {
                counters
                    .overrun_frames
                    .fetch_add((frames - n) as u64, Ordering::Relaxed);
            }
            if monitor_on.load(Ordering::Relaxed) {
                push_stereo(&mut rt.monitor, samples, channels);
            }
        })
        .register()?;
    let format = format_param(channels);
    let mut params = [spa::pod::Pod::from_bytes(&format).expect("valid pod")];
    stream.connect(
        spa::utils::Direction::Input,
        None,
        StreamFlags::AUTOCONNECT | StreamFlags::MAP_BUFFERS | StreamFlags::RT_PROCESS,
        &mut params,
    )?;
    Ok(Capture {
        target: target.to_string(),
        _stream: stream,
        _listener: listener,
    })
}

/// Dequeues a playback buffer, lets `fill` write up to the requested number
/// of stereo frames, and sets the chunk to the frames written.
fn playback_cycle(stream: &pw::stream::Stream, fill: impl FnOnce(&mut [f32], usize)) {
    let Some(mut buffer) = stream.dequeue_buffer() else {
        return;
    };
    let requested = buffer.requested() as usize;
    let Some(data) = buffer.datas_mut().first_mut() else {
        return;
    };
    let Some(out) = samples(data, false) else {
        return;
    };
    let capacity = out.len() / OUTPUT_CHANNELS;
    let frames = if requested == 0 {
        capacity.min(1024)
    } else {
        requested.min(capacity)
    };
    fill(out, frames);
    if let Some(c) = data_chunk(&mut buffer) {
        *c.offset_mut() = 0;
        *c.stride_mut() = (OUTPUT_CHANNELS * 4) as i32;
        *c.size_mut() = (frames * OUTPUT_CHANNELS * 4) as u32;
    }
}

/// Plays the processed microphone to the system default output. Autoconnected,
/// so WirePlumber routes it and follows default output changes.
fn create_monitor(
    core: &pw::core::CoreRc,
    shared: &Shared,
) -> Result<(StreamRc, StreamListener<()>), pw::Error> {
    let stream = StreamRc::new(
        core.clone(),
        "otozane-monitor",
        properties! {
            *pw::keys::MEDIA_TYPE => "Audio",
            *pw::keys::MEDIA_CATEGORY => "Playback",
            *pw::keys::MEDIA_ROLE => "Communication",
            *pw::keys::NODE_NAME => monitor_node_name(),
            *pw::keys::NODE_DESCRIPTION => "Otozane Monitor",
            *pw::keys::MEDIA_NAME => "Otozane Monitor",
        },
    )?;
    let (rx, counters) = (shared.monitor_rx.clone(), shared.counters.clone());
    let listener = stream
        .add_local_listener::<()>()
        .process(move |stream, _| {
            playback_cycle(stream, |out, frames| {
                let q = counters.capture_quantum.load(Ordering::Relaxed) as usize;
                match rx.try_lock() {
                    Ok(mut rx) => {
                        pull_stereo(&mut rx, out, frames, q);
                    }
                    Err(_) => out[..frames * OUTPUT_CHANNELS].fill(0.0),
                }
            });
        })
        .register()?;
    let format = format_param(OUTPUT_CHANNELS);
    let mut params = [spa::pod::Pod::from_bytes(&format).expect("valid pod")];
    stream.connect(
        spa::utils::Direction::Output,
        None,
        StreamFlags::AUTOCONNECT | StreamFlags::MAP_BUFFERS | StreamFlags::RT_PROCESS,
        &mut params,
    )?;
    Ok((stream, listener))
}

fn create_playback(
    core: &pw::core::CoreRc,
    shared: &Shared,
) -> Result<(pw::node::Node, StreamRc, StreamListener<()>), pw::Error> {
    // The virtual microphone itself: a null-audio-sink adapter exposed as a
    // source, the same construction EasyEffects uses. `object.linger = false`
    // ties its lifetime to our connection, so PipeWire removes it when the
    // service exits or crashes.
    let node: pw::node::Node = core.create_object(
        "adapter",
        &properties! {
            "factory.name" => "support.null-audio-sink",
            *pw::keys::NODE_NAME => virtual_node_name(),
            *pw::keys::NODE_DESCRIPTION => virtual_node_description(),
            *pw::keys::MEDIA_CLASS => "Audio/Source/Virtual",
            "audio.position" => "FL,FR",
            "monitor.channel-volumes" => "true",
            "object.linger" => "false",
            "media.icon-name" => "audio-input-microphone",
        },
    )?;
    // Playback stream feeding the virtual microphone.
    let stream = StreamRc::new(
        core.clone(),
        "otozane-output",
        properties! {
            *pw::keys::MEDIA_TYPE => "Audio",
            *pw::keys::MEDIA_CATEGORY => "Playback",
            *pw::keys::NODE_NAME => output_node_name(),
            *pw::keys::NODE_DESCRIPTION => "Otozane Output",
            // Not autoconnected: `Main::link_output` links it to the virtual
            // microphone, so it can never end up on a real sink.
            "node.dont-move" => "true",
        },
    )?;
    let (rx, counters, st) = (
        shared.out_rx.clone(),
        shared.counters.clone(),
        shared.status.clone(),
    );
    let listener = stream
        .add_local_listener::<()>()
        .state_changed(move |_, _, _, new| {
            let ready = matches!(new, StreamState::Paused | StreamState::Streaming);
            st.lock().unwrap().virtual_source_ready = ready;
        })
        .process(move |stream, _| {
            playback_cycle(stream, |out, frames| {
                counters
                    .playback_quantum
                    .store(frames as u32, Ordering::Relaxed);
                let mut got = 0;
                if let Ok(mut rx) = rx.try_lock() {
                    let q = counters.capture_quantum.load(Ordering::Relaxed) as usize;
                    let (g, skipped, left) = pull_stereo(&mut rx, out, frames, q);
                    got = g;
                    counters
                        .skipped_frames
                        .fetch_add(skipped as u64, Ordering::Relaxed);
                    counters.ring_fill.store(left as u32, Ordering::Relaxed);
                } else {
                    out[..frames * OUTPUT_CHANNELS].fill(0.0);
                }
                if got < frames {
                    counters
                        .underrun_frames
                        .fetch_add((frames - got) as u64, Ordering::Relaxed);
                }
            });
        })
        .register()?;
    let format = format_param(OUTPUT_CHANNELS);
    let mut params = [spa::pod::Pod::from_bytes(&format).expect("valid pod")];
    stream.connect(
        spa::utils::Direction::Output,
        None,
        StreamFlags::MAP_BUFFERS | StreamFlags::RT_PROCESS,
        &mut params,
    )?;
    Ok((node, stream, listener))
}

fn data_chunk<'a>(buffer: &'a mut pw::buffer::Buffer<'_>) -> Option<&'a mut spa::buffer::Chunk> {
    buffer.datas_mut().first_mut().map(|d| d.chunk_mut())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(name: &str, description: &str, is_virtual: bool) -> Device {
        Device {
            name: name.into(),
            description: description.into(),
            is_virtual,
        }
    }

    #[test]
    fn automatic_input_choice() {
        let devs = [
            dev("line", "iD4 Line Input", false),
            dev("xlr", "iD4 XLR Microphone", false),
            dev("ee", "Easy Effects Source", true),
        ];
        let pick = |d, last| auto_input(devs.iter(), d, last);
        assert_eq!(
            pick(Some("xlr"), None).as_deref(),
            Some("xlr"),
            "follows the default"
        );
        assert_eq!(
            pick(Some("ee"), Some("xlr")).as_deref(),
            Some("xlr"),
            "keeps last physical"
        );
        assert_eq!(
            pick(Some("otozane_microphone"), None).as_deref(),
            Some("line")
        );
        assert_eq!(pick(None, None).as_deref(), Some("line"));
        assert_eq!(
            default_name(r#"{"name":"alsa_input.x"}"#).as_deref(),
            Some("alsa_input.x")
        );
    }
}
