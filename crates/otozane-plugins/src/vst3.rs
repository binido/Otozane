//! VST3 hosting on Linux (`.vst3` bundles with `Contents/x86_64-linux/*.so`).
//!
//! Plugins run in-process. A crashing plugin takes down the process that
//! loaded it; scanning therefore happens in a separate process (see
//! `otozaned --scan`), but instances in the chain are not isolated.

use crate::{Instance, PluginControl};
use otozane_core::config::{PluginFormat, PluginRef};
use otozane_core::effects::Processor;
use otozane_core::param::ParamDesc;
use std::collections::HashMap;
use std::ffi::{c_char, c_void};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI32, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use vst3::Steinberg::Vst::{
    AudioBusBuffers, AudioBusBuffers__type0, BusDirections_, IAudioProcessor, IAudioProcessorTrait,
    IComponent, IComponentHandler, IComponentHandlerTrait, IComponentTrait, IConnectionPoint,
    IConnectionPointTrait, IEditController, IEditControllerTrait, IHostApplication,
    IHostApplicationTrait, IParamValueQueue, IParamValueQueueTrait, IParameterChanges,
    IParameterChangesTrait, MediaTypes_, ParamID, ParamValue, ParameterInfo, ProcessContext,
    ProcessData, ProcessModes_, ProcessSetup, SpeakerArr, SpeakerArrangement, String128,
    SymbolicSampleSizes_, TChar,
};
use vst3::Steinberg::{
    FUnknown, IBStream, IBStreamTrait, IPluginBaseTrait, IPluginFactory, IPluginFactory2,
    IPluginFactory2Trait, IPluginFactoryTrait, PClassInfo, PClassInfo2, PFactoryInfo, TUID, int32,
    int64, kInvalidArgument, kNotImplemented, kResultFalse, kResultOk, kResultTrue, tresult,
};
use vst3::{Class, ComPtr, ComWrapper, Interface};

const AUDIO_EFFECT_CLASS: &str = "Audio Module Class";

// ---------------------------------------------------------------------------
// Strings and ids

fn c_str(buf: &[c_char]) -> String {
    let bytes: Vec<u8> = buf
        .iter()
        .take_while(|&&c| c != 0)
        .map(|&c| c as u8)
        .collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

fn utf16_str(buf: &[TChar]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

fn uid_hex(tuid: &TUID) -> String {
    tuid.iter().map(|b| format!("{:02X}", *b as u8)).collect()
}

fn parse_uid(hex: &str) -> Option<TUID> {
    if hex.len() != 32 {
        return None;
    }
    let mut out: TUID = [0; 16];
    for (i, o) in out.iter_mut().enumerate() {
        *o = u8::from_str_radix(hex.get(i * 2..i * 2 + 2)?, 16).ok()? as c_char;
    }
    Some(out)
}

fn ok(r: tresult) -> bool {
    r == kResultOk || r == kResultTrue
}

// ---------------------------------------------------------------------------
// Module

/// A loaded `.so` and its plugin factory. Shared by all instances from the
/// same bundle; unloaded when the last instance is dropped.
pub struct Module {
    factory: Option<ComPtr<IPluginFactory>>,
    lib: Option<libloading::os::unix::Library>,
}

// SAFETY: the factory pointer is a COM object designed for cross-thread use;
// the library handle is only used in Drop.
unsafe impl Send for Module {}
unsafe impl Sync for Module {}

static MODULES: Mutex<Option<HashMap<PathBuf, Weak<Module>>>> = Mutex::new(None);

/// Path of the shared object inside a bundle (or the path itself for a bare `.so`).
pub fn binary_path(bundle: &Path) -> PathBuf {
    if bundle.is_file() {
        return bundle.to_path_buf();
    }
    let stem = bundle.file_stem().unwrap_or_default();
    bundle
        .join("Contents")
        .join(format!("{}-linux", std::env::consts::ARCH))
        .join(stem)
        .with_extension("so")
}

impl Module {
    pub fn load(bundle: &Path) -> Result<Arc<Module>, String> {
        let key = bundle.to_path_buf();
        let mut cache = MODULES.lock().unwrap();
        let cache = cache.get_or_insert_with(HashMap::new);
        if let Some(m) = cache.get(&key).and_then(Weak::upgrade) {
            return Ok(m);
        }
        let so = binary_path(bundle);
        // SAFETY: loading a plugin runs its initializers. This is the
        // documented risk of hosting native plugins.
        let lib = unsafe {
            libloading::os::unix::Library::open(
                Some(&so),
                libloading::os::unix::RTLD_NOW | libloading::os::unix::RTLD_LOCAL,
            )
        }
        .map_err(|e| format!("cannot load {}: {e}", so.display()))?;
        let raw = lib.into_raw();
        // SAFETY: `raw` came from dlopen above; rewrapping keeps ownership.
        let lib = unsafe { libloading::os::unix::Library::from_raw(raw) };
        unsafe {
            // The Linux module entry receives the dlopen handle.
            type Entry = unsafe extern "C" fn(*mut c_void) -> bool;
            if let Ok(entry) = lib.get::<Entry>(b"ModuleEntry\0")
                && !entry(raw)
            {
                return Err("ModuleEntry failed".into());
            }
            type GetFactory = unsafe extern "C" fn() -> *mut IPluginFactory;
            let get = lib
                .get::<GetFactory>(b"GetPluginFactory\0")
                .map_err(|_| "not a VST3 plugin (no GetPluginFactory)".to_string())?;
            let factory = ComPtr::from_raw(get()).ok_or("GetPluginFactory returned null")?;
            let module = Arc::new(Module {
                factory: Some(factory),
                lib: Some(lib),
            });
            cache.insert(key, Arc::downgrade(&module));
            Ok(module)
        }
    }

    fn factory(&self) -> &ComPtr<IPluginFactory> {
        self.factory.as_ref().expect("factory lives until drop")
    }

    /// Audio effect classes exported by the module.
    pub fn classes(&self, path: &Path) -> Vec<(PluginRef, String)> {
        let f = self.factory();
        let mut finfo: PFactoryInfo = unsafe { std::mem::zeroed() };
        let factory_vendor = if unsafe { f.getFactoryInfo(&mut finfo) } == kResultOk {
            c_str(&finfo.vendor)
        } else {
            String::new()
        };
        let f2 = f.cast::<IPluginFactory2>();
        let mut out = Vec::new();
        for i in 0..unsafe { f.countClasses() } {
            let (cid, category, name, vendor, sub) = if let Some(f2) = &f2 {
                let mut c: PClassInfo2 = unsafe { std::mem::zeroed() };
                if unsafe { f2.getClassInfo2(i, &mut c) } != kResultOk {
                    continue;
                }
                (
                    c.cid,
                    c_str(&c.category),
                    c_str(&c.name),
                    c_str(&c.vendor),
                    c_str(&c.subCategories),
                )
            } else {
                let mut c: PClassInfo = unsafe { std::mem::zeroed() };
                if unsafe { f.getClassInfo(i, &mut c) } != kResultOk {
                    continue;
                }
                (
                    c.cid,
                    c_str(&c.category),
                    c_str(&c.name),
                    String::new(),
                    String::new(),
                )
            };
            if category != AUDIO_EFFECT_CLASS {
                continue;
            }
            // Instruments cannot process a microphone.
            if sub.split('|').any(|s| s == "Instrument") {
                continue;
            }
            out.push((
                PluginRef {
                    format: PluginFormat::Vst3,
                    path: path.to_string_lossy().into_owned(),
                    uid: uid_hex(&cid),
                    name,
                    vendor: if vendor.is_empty() {
                        factory_vendor.clone()
                    } else {
                        vendor
                    },
                },
                sub,
            ));
        }
        out
    }
}

impl Drop for Module {
    fn drop(&mut self) {
        self.factory = None;
        if let Some(lib) = self.lib.take() {
            unsafe {
                type Exit = unsafe extern "C" fn() -> bool;
                if let Ok(exit) = lib.get::<Exit>(b"ModuleExit\0") {
                    exit();
                }
            }
            drop(lib);
        }
    }
}

// ---------------------------------------------------------------------------
// Host-side COM objects

struct HostApp;

impl Class for HostApp {
    type Interfaces = (IHostApplication,);
}

impl IHostApplicationTrait for HostApp {
    unsafe fn getName(&self, name: *mut String128) -> tresult {
        let s: Vec<u16> = "Otozane".encode_utf16().collect();
        unsafe {
            let dst = &mut *name;
            dst.fill(0);
            dst[..s.len()].copy_from_slice(&s);
        }
        kResultOk
    }
    unsafe fn createInstance(
        &self,
        _cid: *mut TUID,
        _iid: *mut TUID,
        obj: *mut *mut c_void,
    ) -> tresult {
        if !obj.is_null() {
            unsafe { *obj = std::ptr::null_mut() };
        }
        kNotImplemented
    }
}

/// Receives edit notifications from the controller. Without a plugin editor
/// these only matter for `restartComponent`, which we record.
struct Handler {
    restart: AtomicI32,
}

impl Class for Handler {
    type Interfaces = (IComponentHandler,);
}

impl IComponentHandlerTrait for Handler {
    unsafe fn beginEdit(&self, _id: ParamID) -> tresult {
        kResultOk
    }
    unsafe fn performEdit(&self, _id: ParamID, _v: ParamValue) -> tresult {
        kResultOk
    }
    unsafe fn endEdit(&self, _id: ParamID) -> tresult {
        kResultOk
    }
    unsafe fn restartComponent(&self, flags: int32) -> tresult {
        self.restart.fetch_or(flags, Ordering::Relaxed);
        kResultOk
    }
}

/// In-memory IBStream used for plugin state.
struct MemStream {
    data: Mutex<(Vec<u8>, usize)>,
}

impl Class for MemStream {
    type Interfaces = (IBStream,);
}

impl MemStream {
    fn new(data: Vec<u8>) -> ComWrapper<MemStream> {
        ComWrapper::new(MemStream {
            data: Mutex::new((data, 0)),
        })
    }
}

impl IBStreamTrait for MemStream {
    unsafe fn read(&self, buffer: *mut c_void, n: int32, read: *mut int32) -> tresult {
        let mut d = self.data.lock().unwrap();
        let (data, pos) = &mut *d;
        let n = (n.max(0) as usize).min(data.len().saturating_sub(*pos));
        unsafe { std::ptr::copy_nonoverlapping(data.as_ptr().add(*pos), buffer as *mut u8, n) };
        *pos += n;
        if !read.is_null() {
            unsafe { *read = n as int32 };
        }
        kResultOk
    }
    unsafe fn write(&self, buffer: *mut c_void, n: int32, written: *mut int32) -> tresult {
        if n < 0 || (buffer.is_null() && n > 0) {
            return kInvalidArgument;
        }
        let mut d = self.data.lock().unwrap();
        let (data, pos) = &mut *d;
        let n = n as usize;
        let end = *pos + n;
        if data.len() < end {
            data.resize(end, 0);
        }
        unsafe {
            std::ptr::copy_nonoverlapping(buffer as *const u8, data.as_mut_ptr().add(*pos), n)
        };
        *pos = end;
        if !written.is_null() {
            unsafe { *written = n as int32 };
        }
        kResultOk
    }
    unsafe fn seek(&self, offset: int64, mode: int32, result: *mut int64) -> tresult {
        let mut d = self.data.lock().unwrap();
        let (data, pos) = &mut *d;
        let base = match mode {
            0 => 0,
            1 => *pos as i64,
            2 => data.len() as i64,
            _ => return kInvalidArgument,
        };
        let new = base + offset;
        if new < 0 {
            return kInvalidArgument;
        }
        *pos = new as usize;
        if !result.is_null() {
            unsafe { *result = new };
        }
        kResultOk
    }
    unsafe fn tell(&self, pos: *mut int64) -> tresult {
        if !pos.is_null() {
            unsafe { *pos = self.data.lock().unwrap().1 as int64 };
        }
        kResultOk
    }
}

/// One parameter change with a single point at sample offset 0.
struct Queue {
    id: AtomicU32,
    value: AtomicU64,
}

impl Class for Queue {
    type Interfaces = (IParamValueQueue,);
}

impl IParamValueQueueTrait for Queue {
    unsafe fn getParameterId(&self) -> ParamID {
        self.id.load(Ordering::Relaxed)
    }
    unsafe fn getPointCount(&self) -> int32 {
        1
    }
    unsafe fn getPoint(&self, index: int32, offset: *mut int32, value: *mut ParamValue) -> tresult {
        if index != 0 {
            return kResultFalse;
        }
        unsafe {
            *offset = 0;
            *value = f64::from_bits(self.value.load(Ordering::Relaxed));
        }
        kResultOk
    }
    unsafe fn addPoint(&self, _offset: int32, _value: ParamValue, _index: *mut int32) -> tresult {
        kResultFalse
    }
}

/// Preallocated input parameter changes: one queue per parameter, filled on
/// the audio thread without allocating.
struct Changes {
    count: AtomicI32,
    queues: Vec<ComWrapper<Queue>>,
}

impl Class for Changes {
    type Interfaces = (IParameterChanges,);
}

impl IParameterChangesTrait for Changes {
    unsafe fn getParameterCount(&self) -> int32 {
        self.count.load(Ordering::Relaxed)
    }
    unsafe fn getParameterData(&self, index: int32) -> *mut IParamValueQueue {
        if index < 0 || index >= self.count.load(Ordering::Relaxed) {
            return std::ptr::null_mut();
        }
        self.queues[index as usize]
            .as_com_ref::<IParamValueQueue>()
            .map_or(std::ptr::null_mut(), |r| r.as_ptr())
    }
    unsafe fn addParameterData(
        &self,
        _id: *const ParamID,
        index: *mut int32,
    ) -> *mut IParamValueQueue {
        if !index.is_null() {
            unsafe { *index = -1 };
        }
        std::ptr::null_mut()
    }
}

// ---------------------------------------------------------------------------
// Instance

/// Objects shared by the audio half and the control half. Torn down when both are dropped.
struct Inner {
    component: ComPtr<IComponent>,
    processor: ComPtr<IAudioProcessor>,
    controller: Option<ComPtr<IEditController>>,
    /// Controller is a separate object that we initialized and must terminate.
    separate_controller: bool,
    connection: Option<(ComPtr<IConnectionPoint>, ComPtr<IConnectionPoint>)>,
    handler: ComWrapper<Handler>,
    _host: ComWrapper<HostApp>,
    _module: Arc<Module>,
}

// SAFETY: VST3 COM objects are used from the service thread (setup, state,
// parameter text) and the audio thread (process), as the VST3 threading model allows.
unsafe impl Send for Inner {}
unsafe impl Sync for Inner {}

impl Drop for Inner {
    fn drop(&mut self) {
        unsafe {
            self.processor.setProcessing(0);
            self.component.setActive(0);
            if let Some((a, b)) = &self.connection {
                a.disconnect(b.as_ptr());
                b.disconnect(a.as_ptr());
            }
            if let Some(c) = &self.controller {
                c.setComponentHandler(std::ptr::null_mut());
                if self.separate_controller {
                    c.terminate();
                }
            }
            self.component.terminate();
        }
    }
}

fn host_unknown(host: &ComWrapper<HostApp>) -> *mut FUnknown {
    host.as_com_ref::<IHostApplication>()
        .map_or(std::ptr::null_mut(), |r| r.as_ptr() as *mut FUnknown)
}

fn arrangement_channels(a: SpeakerArrangement) -> usize {
    a.count_ones() as usize
}

/// State blob layout: u32 little-endian component length, component bytes, controller bytes.
fn split_state(state: &[u8]) -> Option<(&[u8], &[u8])> {
    let len = u32::from_le_bytes(state.get(..4)?.try_into().ok()?) as usize;
    let rest = &state[4..];
    (len <= rest.len()).then(|| rest.split_at(len))
}

fn stream_bytes(s: &ComWrapper<MemStream>) -> Vec<u8> {
    s.data.lock().unwrap().0.clone()
}

fn stream_ptr(s: &ComWrapper<MemStream>) -> *mut IBStream {
    s.as_com_ref::<IBStream>()
        .map_or(std::ptr::null_mut(), |r| r.as_ptr())
}

pub fn instantiate(
    plugin: &PluginRef,
    state: Option<&[u8]>,
    sample_rate: f32,
    channels: usize,
    max_block: usize,
) -> Result<Instance, String> {
    let module = Module::load(Path::new(&plugin.path))?;
    let cid = parse_uid(&plugin.uid).ok_or("invalid plugin id")?;
    let host = ComWrapper::new(HostApp);
    let factory = module.factory();
    unsafe {
        let mut obj = std::ptr::null_mut();
        let r = factory.createInstance(
            cid.as_ptr(),
            IComponent::IID.as_ptr() as *const c_char,
            &mut obj,
        );
        let component = ComPtr::from_raw(obj as *mut IComponent)
            .filter(|_| r == kResultOk)
            .ok_or_else(|| format!("cannot create '{}'", plugin.name))?;
        if !ok(component.initialize(host_unknown(&host))) {
            return Err("component initialization failed".into());
        }
        let processor = match component.cast::<IAudioProcessor>() {
            Some(p) => p,
            None => {
                component.terminate();
                return Err("plugin has no audio processor".into());
            }
        };

        // Edit controller: either the component itself or a separate class.
        let (controller, separate) = match component.cast::<IEditController>() {
            Some(c) => (Some(c), false),
            None => {
                let mut ccid: TUID = [0; 16];
                let mut c = None;
                if component.getControllerClassId(&mut ccid) == kResultOk {
                    let mut obj = std::ptr::null_mut();
                    if factory.createInstance(
                        ccid.as_ptr(),
                        IEditController::IID.as_ptr() as *const c_char,
                        &mut obj,
                    ) == kResultOk
                        && let Some(ctrl) = ComPtr::from_raw(obj as *mut IEditController)
                        && ok(ctrl.initialize(host_unknown(&host)))
                    {
                        c = Some(ctrl);
                    }
                }
                (c, true)
            }
        };
        let handler = ComWrapper::new(Handler {
            restart: AtomicI32::new(0),
        });
        let mut connection = None;
        if let Some(c) = &controller {
            if let Some(h) = handler.as_com_ref::<IComponentHandler>() {
                c.setComponentHandler(h.as_ptr());
            }
            if separate
                && let (Some(a), Some(b)) = (
                    component.cast::<IConnectionPoint>(),
                    c.cast::<IConnectionPoint>(),
                )
            {
                a.connect(b.as_ptr());
                b.connect(a.as_ptr());
                connection = Some((a, b));
            }
        }
        let inner = Arc::new(Inner {
            component,
            processor,
            controller,
            separate_controller: separate,
            connection,
            handler,
            _host: host,
            _module: module.clone(),
        });

        // Buses: the main input/output use our channel layout when the
        // plugin accepts it; other buses keep theirs and get silence.
        let ours = if channels == 1 {
            SpeakerArr::kMono
        } else {
            SpeakerArr::kStereo
        };
        let count = |dir| inner.component.getBusCount(MediaTypes_::kAudio as i32, dir);
        let n_in = count(BusDirections_::kInput as i32).max(0) as usize;
        let n_out = count(BusDirections_::kOutput as i32).max(0) as usize;
        if n_out == 0 {
            return Err("plugin has no audio output".into());
        }
        let current = |dir, i| {
            let mut a: SpeakerArrangement = 0;
            inner.processor.getBusArrangement(dir, i as i32, &mut a);
            a
        };
        let mut ins: Vec<SpeakerArrangement> = (0..n_in)
            .map(|i| current(BusDirections_::kInput as i32, i))
            .collect();
        let mut outs: Vec<SpeakerArrangement> = (0..n_out)
            .map(|i| current(BusDirections_::kOutput as i32, i))
            .collect();
        if let Some(a) = ins.first_mut() {
            *a = ours;
        }
        outs[0] = ours;
        // If rejected, the plugin keeps its own layout and process() adapts to it.
        inner.processor.setBusArrangements(
            ins.as_mut_ptr(),
            n_in as i32,
            outs.as_mut_ptr(),
            n_out as i32,
        );
        let ins: Vec<usize> = (0..n_in)
            .map(|i| arrangement_channels(current(BusDirections_::kInput as i32, i)))
            .collect();
        let outs: Vec<usize> = (0..n_out)
            .map(|i| arrangement_channels(current(BusDirections_::kOutput as i32, i)))
            .collect();
        // All buses are active: auxiliary inputs (e.g. sidechains) get
        // silence and auxiliary outputs are discarded.
        for (dir, n) in [
            (BusDirections_::kInput, n_in),
            (BusDirections_::kOutput, n_out),
        ] {
            for i in 0..n {
                inner
                    .component
                    .activateBus(MediaTypes_::kAudio as i32, dir as i32, i as i32, 1);
            }
        }
        if inner
            .processor
            .canProcessSampleSize(SymbolicSampleSizes_::kSample32 as i32)
            != kResultTrue
        {
            return Err("plugin does not support 32-bit float processing".into());
        }
        let mut setup = ProcessSetup {
            processMode: ProcessModes_::kRealtime as i32,
            symbolicSampleSize: SymbolicSampleSizes_::kSample32 as i32,
            maxSamplesPerBlock: max_block as i32,
            sampleRate: sample_rate as f64,
        };
        if !ok(inner.processor.setupProcessing(&mut setup)) {
            return Err("plugin rejected the processing setup".into());
        }
        // State: restore ours, or sync the controller with the component's defaults.
        // Some plugins derive internal values from the sample rate here, so
        // this comes after setupProcessing.
        let comp_stream;
        if let Some((cs, ctl)) = state.and_then(split_state) {
            comp_stream = MemStream::new(cs.to_vec());
            inner.component.setState(stream_ptr(&comp_stream));
            if let Some(c) = &inner.controller {
                comp_stream.seek(0, 0, std::ptr::null_mut());
                c.setComponentState(stream_ptr(&comp_stream));
                if !ctl.is_empty() {
                    let s = MemStream::new(ctl.to_vec());
                    c.setState(stream_ptr(&s));
                }
            }
        } else {
            // Feeding the default state back makes plugins that only compute
            // derived values (e.g. latency) in setState report them correctly.
            comp_stream = MemStream::new(Vec::new());
            if inner.component.getState(stream_ptr(&comp_stream)) == kResultOk {
                comp_stream.seek(0, 0, std::ptr::null_mut());
                inner.component.setState(stream_ptr(&comp_stream));
                if let Some(c) = &inner.controller {
                    comp_stream.seek(0, 0, std::ptr::null_mut());
                    c.setComponentState(stream_ptr(&comp_stream));
                }
            }
        }

        if !ok(inner.component.setActive(1)) {
            return Err("plugin failed to activate".into());
        }
        // The bypass path allocates a delay of this length; ignore absurd values.
        let latency = (inner.processor.getLatencySamples() as usize).min(sample_rate as usize);
        inner.processor.setProcessing(1);

        // Parameters (hidden, read-only and bypass parameters are not exposed;
        // bypass is handled by the chain).
        let mut descs = Vec::new();
        let mut ids = Vec::new();
        let mut values = Vec::new();
        if let Some(c) = &inner.controller {
            for i in 0..c.getParameterCount() {
                let mut p: ParameterInfo = std::mem::zeroed();
                if c.getParameterInfo(i, &mut p) != kResultOk {
                    continue;
                }
                use vst3::Steinberg::Vst::ParameterInfo_::ParameterFlags_::*;
                if p.flags & (kIsHidden | kIsReadOnly | kIsBypass) != 0 {
                    continue;
                }
                let steps = p.stepCount.max(0) as u32;
                let choices = if (1..=24).contains(&steps) {
                    (0..=steps)
                        .map(|k| text(c, p.id, k as f64 / steps as f64))
                        .collect()
                } else {
                    Vec::new()
                };
                descs.push(ParamDesc {
                    key: p.id.to_string(),
                    name: utf16_str(&p.title),
                    min: 0.0,
                    max: 1.0,
                    default: p.defaultNormalizedValue as f32,
                    unit: utf16_str(&p.units),
                    steps,
                    log: false,
                    choices,
                });
                ids.push(p.id);
                values.push(c.getParamNormalized(p.id) as f32);
            }
        }

        let changes = ComWrapper::new(Changes {
            count: AtomicI32::new(0),
            queues: ids
                .iter()
                .map(|_| {
                    ComWrapper::new(Queue {
                        id: AtomicU32::new(0),
                        value: AtomicU64::new(0),
                    })
                })
                .collect(),
        });
        let processor = Vst3Processor::new(
            inner.clone(),
            ids.clone(),
            values.clone(),
            changes,
            &ins,
            &outs,
            max_block,
            sample_rate,
            latency,
        );
        Ok(Instance {
            processor: Box::new(processor),
            control: Box::new(Vst3Control { inner, ids }),
            params: descs,
            values,
        })
    }
}

fn text(c: &ComPtr<IEditController>, id: ParamID, v: f64) -> String {
    let mut s: String128 = [0; 128];
    if unsafe { c.getParamStringByValue(id, v, &mut s) } == kResultOk {
        utf16_str(&s)
    } else {
        format!("{v:.3}")
    }
}

struct Vst3Control {
    inner: Arc<Inner>,
    ids: Vec<ParamID>,
}

impl PluginControl for Vst3Control {
    fn value_text(&self, index: usize, value: f32) -> String {
        match (&self.inner.controller, self.ids.get(index)) {
            (Some(c), Some(&id)) => text(c, id, value as f64),
            _ => String::new(),
        }
    }

    fn set_value(&self, index: usize, value: f32) {
        if let (Some(c), Some(&id)) = (&self.inner.controller, self.ids.get(index)) {
            unsafe { c.setParamNormalized(id, value as f64) };
        }
    }

    fn save_state(&self) -> Result<Vec<u8>, String> {
        let comp = MemStream::new(Vec::new());
        if unsafe { self.inner.component.getState(stream_ptr(&comp)) } != kResultOk {
            return Err("plugin could not save its state".into());
        }
        let ctl = MemStream::new(Vec::new());
        if let Some(c) = &self.inner.controller {
            unsafe { c.getState(stream_ptr(&ctl)) };
        }
        let (comp, ctl) = (stream_bytes(&comp), stream_bytes(&ctl));
        let mut out = (comp.len() as u32).to_le_bytes().to_vec();
        out.extend_from_slice(&comp);
        out.extend_from_slice(&ctl);
        Ok(out)
    }

    fn needs_restart(&self) -> bool {
        self.inner.handler.restart.load(Ordering::Relaxed) != 0
    }
}

/// Audio-thread half: owns preallocated bus buffers and parameter queues.
struct Vst3Processor {
    inner: Arc<Inner>,
    ids: Vec<ParamID>,
    last: Vec<f32>,
    changes: ComWrapper<Changes>,
    ins: Vec<Vec<Vec<f32>>>,
    outs: Vec<Vec<Vec<f32>>>,
    in_ptrs: Vec<Vec<*mut f32>>,
    out_ptrs: Vec<Vec<*mut f32>>,
    in_bufs: Vec<AudioBusBuffers>,
    out_bufs: Vec<AudioBusBuffers>,
    context: Box<ProcessContext>,
    latency: usize,
}

// SAFETY: the raw pointers point into buffers owned by this struct, which are
// never reallocated after construction; the struct is moved to the audio
// thread as a whole and only used there.
unsafe impl Send for Vst3Processor {}

impl Vst3Processor {
    #[allow(clippy::too_many_arguments)]
    fn new(
        inner: Arc<Inner>,
        ids: Vec<ParamID>,
        values: Vec<f32>,
        changes: ComWrapper<Changes>,
        ins: &[usize],
        outs: &[usize],
        max_block: usize,
        sample_rate: f32,
        latency: usize,
    ) -> Self {
        let alloc = |chs: &[usize]| -> Vec<Vec<Vec<f32>>> {
            chs.iter()
                .map(|&n| (0..n).map(|_| vec![0.0; max_block]).collect())
                .collect()
        };
        let mut ins_b = alloc(ins);
        let mut outs_b = alloc(outs);
        let ptrs = |b: &mut Vec<Vec<Vec<f32>>>| -> Vec<Vec<*mut f32>> {
            b.iter_mut()
                .map(|bus| bus.iter_mut().map(|c| c.as_mut_ptr()).collect())
                .collect()
        };
        let in_ptrs = ptrs(&mut ins_b);
        let out_ptrs = ptrs(&mut outs_b);
        let mut context: Box<ProcessContext> = Box::new(unsafe { std::mem::zeroed() });
        context.sampleRate = sample_rate as f64;
        context.tempo = 120.0;
        context.timeSigNumerator = 4;
        context.timeSigDenominator = 4;
        let mut s = Self {
            inner,
            ids,
            last: values,
            changes,
            ins: ins_b,
            outs: outs_b,
            in_ptrs,
            out_ptrs,
            in_bufs: Vec::new(),
            out_bufs: Vec::new(),
            context,
            latency,
        };
        let bufs = |p: &mut Vec<Vec<*mut f32>>| -> Vec<AudioBusBuffers> {
            p.iter_mut()
                .map(|bus| AudioBusBuffers {
                    numChannels: bus.len() as i32,
                    silenceFlags: 0,
                    __field0: AudioBusBuffers__type0 {
                        channelBuffers32: bus.as_mut_ptr(),
                    },
                })
                .collect()
        };
        s.in_bufs = bufs(&mut s.in_ptrs);
        s.out_bufs = bufs(&mut s.out_ptrs);
        s
    }
}

impl Processor for Vst3Processor {
    fn process(&mut self, ch: &mut [&mut [f32]], values: &[f32]) {
        let n = ch[0].len();
        if n == 0 {
            return;
        }
        // Parameter changes since the last block, at offset 0.
        let mut count = 0;
        for (i, (&v, last)) in values.iter().zip(self.last.iter_mut()).enumerate() {
            if v != *last {
                *last = v;
                let q = &self.changes.queues[count];
                q.id.store(self.ids[i], Ordering::Relaxed);
                q.value.store((v as f64).to_bits(), Ordering::Relaxed);
                count += 1;
            }
        }
        self.changes.count.store(count as i32, Ordering::Relaxed);

        // Main input: adapt our channel count to the plugin's.
        if let Some(main) = self.ins.first_mut() {
            if main.len() == 1 && ch.len() == 2 {
                for i in 0..n {
                    main[0][i] = 0.5 * (ch[0][i] + ch[1][i]);
                }
            } else {
                for (k, buf) in main.iter_mut().enumerate() {
                    buf[..n].copy_from_slice(&ch[k.min(ch.len() - 1)][..n]);
                }
            }
        }
        let mut data = ProcessData {
            processMode: ProcessModes_::kRealtime as i32,
            symbolicSampleSize: SymbolicSampleSizes_::kSample32 as i32,
            numSamples: n as i32,
            numInputs: self.in_bufs.len() as i32,
            numOutputs: self.out_bufs.len() as i32,
            inputs: if self.in_bufs.is_empty() {
                std::ptr::null_mut()
            } else {
                self.in_bufs.as_mut_ptr()
            },
            outputs: self.out_bufs.as_mut_ptr(),
            inputParameterChanges: self
                .changes
                .as_com_ref::<IParameterChanges>()
                .map_or(std::ptr::null_mut(), |r| r.as_ptr()),
            outputParameterChanges: std::ptr::null_mut(),
            inputEvents: std::ptr::null_mut(),
            outputEvents: std::ptr::null_mut(),
            processContext: &mut *self.context,
        };
        for b in self.out_bufs.iter_mut().chain(self.in_bufs.iter_mut()) {
            b.silenceFlags = 0;
        }
        // SAFETY: all buffers hold max_block frames and n <= max_block.
        let r = unsafe { self.inner.processor.process(&mut data) };
        self.context.projectTimeSamples += n as i64;
        self.context.continousTimeSamples += n as i64;
        if r != kResultOk {
            return; // leave the input untouched
        }
        let main = &self.outs[0];
        if main.len() == 1 {
            for c in ch.iter_mut() {
                c[..n].copy_from_slice(&main[0][..n]);
            }
        } else if ch.len() == 1 {
            for i in 0..n {
                ch[0][i] = 0.5 * (main[0][i] + main[1][i]);
            }
        } else {
            for (k, c) in ch.iter_mut().enumerate() {
                c[..n].copy_from_slice(&main[k.min(main.len() - 1)][..n]);
            }
        }
    }

    fn latency(&self) -> usize {
        self.latency
    }
}

/// Lists audio effect classes in a bundle. Run this in a separate process:
/// loading a plugin executes its code.
pub fn scan(path: &Path) -> Result<Vec<(PluginRef, String)>, String> {
    let m = Module::load(path)?;
    Ok(m.classes(path))
}

/// Standard VST3 locations on Linux.
pub fn default_dirs() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(h) = std::env::var_os("HOME") {
        v.push(PathBuf::from(h).join(".vst3"));
    }
    v.push("/usr/lib/vst3".into());
    v.push("/usr/local/lib/vst3".into());
    v
}

/// Finds `*.vst3` bundles directly or one level below `dir`.
pub fn find_bundles(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else {
        return out;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.extension().is_some_and(|x| x == "vst3") {
            out.push(p);
        } else if p.is_dir() {
            out.extend(
                std::fs::read_dir(&p)
                    .into_iter()
                    .flatten()
                    .flatten()
                    .map(|e| e.path())
                    .filter(|p| p.extension().is_some_and(|x| x == "vst3")),
            );
        }
    }
    out.sort();
    out
}
