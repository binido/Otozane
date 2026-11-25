//! JSFX hosting through ysfx. Scripts are compiled to native code on load
//! and run in-process, like VST3 plugins.

use crate::Instance;
use otozane_core::config::PluginRef;
use std::path::{Path, PathBuf};

/// A JSFX script is parsed in the service process; bound what it can make us read.
/// The patched ysfx loader uses the same per-file cap and adds a total import budget.
const MAX_SCRIPT_BYTES: u64 = 1 << 20;

/// Standard JSFX locations: the REAPER user effects folder.
pub fn default_dirs() -> Vec<PathBuf> {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return Vec::new();
    };
    if cfg!(target_os = "macos") {
        vec![home.join("Library/Application Support/REAPER/Effects")]
    } else {
        vec![home.join(".config/REAPER/Effects")]
    }
}

/// Finds JSFX scripts below `dir`: `*.jsfx` files, and files without an
/// extension that have a `desc:` line (REAPER's own effects are named so).
pub fn find_scripts(dir: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, depth: u32, out: &mut Vec<PathBuf>) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for e in rd.flatten() {
            let p = e.path();
            let Ok(ft) = std::fs::metadata(&p) else {
                continue;
            };
            if ft.is_dir() {
                // VST3 bundles share the search directories.
                if depth < 4 && p.extension().is_none_or(|x| x != "vst3") {
                    walk(&p, depth + 1, out);
                }
            } else if ft.is_file()
                && match p.extension() {
                    // List an oversized script so the scan can report why it
                    // was rejected; only extensionless files are read here.
                    Some(x) => x == "jsfx",
                    None => ft.len() < MAX_SCRIPT_BYTES && has_desc(&p),
                }
            {
                out.push(p);
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, 0, &mut out);
    out.sort();
    out
}

/// Discovery skips large scripts; a profile can still name one directly, so
/// the loader checks again.
fn script_too_large(path: &Path) -> bool {
    std::fs::metadata(path).map_or(true, |m| m.len() >= MAX_SCRIPT_BYTES)
}

fn has_desc(p: &Path) -> bool {
    std::fs::read(p).is_ok_and(|b| {
        b.split(|&c| c == b'\n')
            .any(|l| l.trim_ascii_start().starts_with(b"desc:"))
    })
}

#[cfg(ysfx)]
pub use imp::{instantiate, scan};

#[cfg(not(ysfx))]
pub fn instantiate(
    _: &PluginRef,
    _: Option<&[u8]>,
    _: f32,
    _: usize,
    _: usize,
) -> Result<Instance, String> {
    Err("built without JSFX support".into())
}

#[cfg(not(ysfx))]
pub fn scan(_: &Path) -> Result<(PluginRef, String), String> {
    Err("built without JSFX support".into())
}

#[cfg(ysfx)]
mod imp {
    use super::*;
    use crate::PluginControl;
    use otozane_core::config::PluginFormat;
    use otozane_core::effects::Processor;
    use otozane_core::param::ParamDesc;
    use std::cell::RefCell;
    use std::ffi::{CStr, CString, c_char};
    use std::sync::{Arc, Mutex};

    mod ffi {
        #![allow(non_camel_case_types)]
        use std::ffi::c_char;

        pub enum ysfx_t {}
        pub enum ysfx_config_t {}

        #[repr(C)]
        #[derive(Default)]
        pub struct ysfx_slider_range_t {
            pub def: f64,
            pub min: f64,
            pub max: f64,
            pub inc: f64,
        }

        #[repr(C)]
        pub struct ysfx_state_slider_t {
            pub index: u32,
            pub value: f64,
        }

        #[repr(C)]
        pub struct ysfx_state_t {
            pub sliders: *mut ysfx_state_slider_t,
            pub slider_count: u32,
            pub data: *mut u8,
            pub data_size: usize,
        }

        pub type LogReporter = extern "C" fn(userdata: isize, level: i32, message: *const c_char);

        pub const MAX_SLIDERS: u32 = 64;
        pub const LOG_ERROR: i32 = 2;
        pub const LOAD_IGNORING_IMPORTS: u32 = 1;

        unsafe extern "C" {
            pub fn ysfx_config_new() -> *mut ysfx_config_t;
            pub fn ysfx_config_free(config: *mut ysfx_config_t);
            pub fn ysfx_guess_file_roots(config: *mut ysfx_config_t, sourcepath: *const c_char);
            pub fn ysfx_register_builtin_audio_formats(config: *mut ysfx_config_t);
            pub fn ysfx_set_log_reporter(config: *mut ysfx_config_t, reporter: LogReporter);

            pub fn ysfx_new(config: *mut ysfx_config_t) -> *mut ysfx_t;
            pub fn ysfx_free(fx: *mut ysfx_t);
            pub fn ysfx_load_file(fx: *mut ysfx_t, filepath: *const c_char, loadopts: u32) -> bool;
            pub fn ysfx_compile(fx: *mut ysfx_t, compileopts: u32) -> bool;
            pub fn ysfx_get_name(fx: *mut ysfx_t) -> *const c_char;
            pub fn ysfx_get_author(fx: *mut ysfx_t) -> *const c_char;
            pub fn ysfx_get_tags(fx: *mut ysfx_t, dest: *mut *const c_char, size: u32) -> u32;
            pub fn ysfx_get_tag(fx: *mut ysfx_t, index: u32) -> *const c_char;
            pub fn ysfx_get_num_inputs(fx: *mut ysfx_t) -> u32;
            pub fn ysfx_get_num_outputs(fx: *mut ysfx_t) -> u32;

            pub fn ysfx_slider_exists(fx: *mut ysfx_t, index: u32) -> bool;
            pub fn ysfx_slider_get_name(fx: *mut ysfx_t, index: u32) -> *const c_char;
            pub fn ysfx_slider_get_range(
                fx: *mut ysfx_t,
                index: u32,
                range: *mut ysfx_slider_range_t,
            ) -> bool;
            pub fn ysfx_slider_is_enum(fx: *mut ysfx_t, index: u32) -> bool;
            pub fn ysfx_slider_get_enum_name(fx: *mut ysfx_t, index: u32, e: u32) -> *const c_char;
            pub fn ysfx_slider_is_path(fx: *mut ysfx_t, index: u32) -> bool;
            pub fn ysfx_slider_is_initially_visible(fx: *mut ysfx_t, index: u32) -> bool;
            pub fn ysfx_slider_get_value(fx: *mut ysfx_t, index: u32) -> f64;
            pub fn ysfx_slider_set_value(fx: *mut ysfx_t, index: u32, value: f64);

            pub fn ysfx_set_block_size(fx: *mut ysfx_t, blocksize: u32);
            pub fn ysfx_set_sample_rate(fx: *mut ysfx_t, samplerate: f64);
            pub fn ysfx_init(fx: *mut ysfx_t);
            pub fn ysfx_get_pdc_delay(fx: *mut ysfx_t) -> f64;
            pub fn ysfx_process_float(
                fx: *mut ysfx_t,
                ins: *const *const f32,
                outs: *const *mut f32,
                num_ins: u32,
                num_outs: u32,
                num_frames: u32,
            );

            pub fn ysfx_load_state(fx: *mut ysfx_t, state: *mut ysfx_state_t) -> bool;
            pub fn ysfx_save_state(fx: *mut ysfx_t) -> *mut ysfx_state_t;
            pub fn ysfx_state_free(state: *mut ysfx_state_t);
        }
    }

    thread_local! {
        /// Errors reported by ysfx while a script loads on this thread.
        static ERRORS: RefCell<Option<String>> = const { RefCell::new(None) };
    }

    extern "C" fn log_reporter(_: isize, level: i32, message: *const c_char) {
        let msg = unsafe { CStr::from_ptr(message) }.to_string_lossy();
        let collected = ERRORS.with_borrow_mut(|e| match e {
            Some(e) if level == ffi::LOG_ERROR => {
                if !e.is_empty() {
                    e.push_str("; ");
                }
                e.push_str(&msg);
                true
            }
            _ => false,
        });
        if !collected {
            eprintln!("[ysfx] {msg}");
        }
    }

    fn c_string(p: *const c_char) -> String {
        if p.is_null() {
            return String::new();
        }
        unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
    }

    /// An owned ysfx instance.
    struct Fx(*mut ffi::ysfx_t);

    // SAFETY: an instance is used by one thread at a time (behind a Mutex).
    unsafe impl Send for Fx {}

    impl Drop for Fx {
        fn drop(&mut self) {
            unsafe { ffi::ysfx_free(self.0) };
        }
    }

    /// Loads (and with `compile`, compiles) a script. Errors carry ysfx's messages.
    fn load(path: &str, compile: bool) -> Result<Fx, String> {
        if script_too_large(Path::new(path)) {
            return Err(format!(
                "script is missing or larger than {MAX_SCRIPT_BYTES} bytes"
            ));
        }
        let cpath = CString::new(path).map_err(|_| "invalid path")?;
        ERRORS.set(Some(String::new()));
        let fx = unsafe {
            let config = ffi::ysfx_config_new();
            ffi::ysfx_set_log_reporter(config, log_reporter);
            ffi::ysfx_register_builtin_audio_formats(config);
            ffi::ysfx_guess_file_roots(config, cpath.as_ptr());
            let fx = Fx(ffi::ysfx_new(config));
            ffi::ysfx_config_free(config);
            fx
        };
        let ok = unsafe {
            if compile {
                ffi::ysfx_load_file(fx.0, cpath.as_ptr(), 0) && ffi::ysfx_compile(fx.0, 0)
            } else {
                ffi::ysfx_load_file(fx.0, cpath.as_ptr(), ffi::LOAD_IGNORING_IMPORTS)
            }
        };
        let errors = ERRORS.take().unwrap_or_default();
        if ok {
            Ok(fx)
        } else if errors.is_empty() {
            Err(format!("cannot load {path}"))
        } else {
            Err(errors)
        }
    }

    /// Reads a script's name, author and tags without compiling it.
    pub fn scan(path: &Path) -> Result<(PluginRef, String), String> {
        let path_s = path
            .to_str()
            .ok_or("plugin path is not valid UTF-8".to_string())?;
        let fx = load(path_s, false)?;
        let mut name = c_string(unsafe { ffi::ysfx_get_name(fx.0) });
        if name.is_empty() {
            name = path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into();
        }
        let tags = unsafe { ffi::ysfx_get_tags(fx.0, std::ptr::null_mut(), 0) };
        let category = (0..tags)
            .map(|i| c_string(unsafe { ffi::ysfx_get_tag(fx.0, i) }))
            .collect::<Vec<_>>()
            .join("|");
        let plugin = PluginRef {
            format: PluginFormat::Jsfx,
            path: path_s.to_string(),
            uid: String::new(),
            name,
            vendor: c_string(unsafe { ffi::ysfx_get_author(fx.0) }),
        };
        Ok((plugin, category))
    }

    /// One exposed slider. The chain sees it normalized to 0..1, like VST3 parameters.
    #[derive(Clone)]
    struct Slider {
        index: u32,
        min: f64,
        max: f64,
        inc: f64,
        choices: Vec<String>,
    }

    impl Slider {
        fn real(&self, v: f32) -> f64 {
            let x = self.min + v.clamp(0.0, 1.0) as f64 * (self.max - self.min);
            if self.inc > 0.0 {
                let x = self.min + ((x - self.min) / self.inc).round() * self.inc;
                // A range that is not a multiple of the step rounds past the end.
                x.clamp(self.min.min(self.max), self.min.max(self.max))
            } else {
                x
            }
        }

        fn normalized(&self, x: f64) -> f32 {
            if self.max == self.min {
                return 0.0;
            }
            (((x - self.min) / (self.max - self.min)) as f32).clamp(0.0, 1.0)
        }

        fn steps(&self) -> u32 {
            if self.inc > 0.0 {
                ((self.max - self.min).abs() / self.inc).round() as u32
            } else {
                0
            }
        }
    }

    fn sliders(fx: &Fx) -> (Vec<Slider>, Vec<ParamDesc>) {
        let mut out = Vec::new();
        let mut descs = Vec::new();
        for i in 0..ffi::MAX_SLIDERS {
            unsafe {
                // Hidden sliders (`-` prefix) are internal; file sliders need a picker.
                if !ffi::ysfx_slider_exists(fx.0, i)
                    || ffi::ysfx_slider_is_path(fx.0, i)
                    || !ffi::ysfx_slider_is_initially_visible(fx.0, i)
                {
                    continue;
                }
                let mut r = ffi::ysfx_slider_range_t::default();
                ffi::ysfx_slider_get_range(fx.0, i, &mut r);
                let mut s = Slider {
                    index: i,
                    min: r.min,
                    max: r.max,
                    inc: r.inc.abs(),
                    choices: Vec::new(),
                };
                if ffi::ysfx_slider_is_enum(fx.0, i) {
                    s.choices = (0..=s.steps())
                        .map(|e| c_string(ffi::ysfx_slider_get_enum_name(fx.0, i, e)))
                        .collect();
                }
                descs.push(ParamDesc {
                    key: format!("slider{}", i + 1),
                    name: c_string(ffi::ysfx_slider_get_name(fx.0, i)),
                    min: 0.0,
                    max: 1.0,
                    default: s.normalized(r.def),
                    unit: String::new(),
                    steps: s.steps(),
                    log: false,
                    choices: s.choices.clone(),
                });
                out.push(s);
            }
        }
        (out, descs)
    }

    /// State blob: `ysfx_state_t` flattened as u32 slider count, then
    /// (u32 index, f64 value) per slider, then the `@serialize` data. Little-endian.
    fn encode(s: &ffi::ysfx_state_t) -> Vec<u8> {
        unsafe fn slice<'a, T>(p: *const T, n: usize) -> &'a [T] {
            if n == 0 {
                &[]
            } else {
                unsafe { std::slice::from_raw_parts(p, n) }
            }
        }
        let sliders = unsafe { slice(s.sliders, s.slider_count as usize) };
        let data = unsafe { slice(s.data, s.data_size) };
        let mut out = s.slider_count.to_le_bytes().to_vec();
        for sl in sliders {
            out.extend_from_slice(&sl.index.to_le_bytes());
            out.extend_from_slice(&sl.value.to_le_bytes());
        }
        out.extend_from_slice(data);
        out
    }

    fn decode(b: &[u8]) -> Option<(Vec<ffi::ysfx_state_slider_t>, Vec<u8>)> {
        let n = u32::from_le_bytes(b.get(..4)?.try_into().ok()?) as usize;
        let end = 4 + n.checked_mul(12)?;
        let sliders = b
            .get(4..end)?
            .as_chunks::<12>()
            .0
            .iter()
            .map(|c| ffi::ysfx_state_slider_t {
                index: u32::from_le_bytes(c[..4].try_into().unwrap()),
                value: f64::from_le_bytes(c[4..].try_into().unwrap()),
            })
            .collect();
        Some((sliders, b[end..].to_vec()))
    }

    pub fn instantiate(
        plugin: &PluginRef,
        state: Option<&[u8]>,
        sample_rate: f32,
        channels: usize,
        max_block: usize,
    ) -> Result<Instance, String> {
        let fx = load(&plugin.path, true)?;
        let (ins, outs) = unsafe {
            (
                ffi::ysfx_get_num_inputs(fx.0) as usize,
                ffi::ysfx_get_num_outputs(fx.0) as usize,
            )
        };
        if outs == 0 {
            return Err("plugin has no audio output".into());
        }
        unsafe {
            ffi::ysfx_set_sample_rate(fx.0, sample_rate as f64);
            ffi::ysfx_set_block_size(fx.0, max_block as u32);
            ffi::ysfx_init(fx.0);
            if let Some((mut sliders, mut data)) = state.and_then(decode) {
                let mut s = ffi::ysfx_state_t {
                    sliders: sliders.as_mut_ptr(),
                    slider_count: sliders.len() as u32,
                    data: data.as_mut_ptr(),
                    data_size: data.len(),
                };
                ffi::ysfx_load_state(fx.0, &mut s);
            }
            // An empty block runs @slider, where scripts usually set pdc_delay. It gets
            // the channel count of the audio path, because @slider may read num_ch.
            let k = if channels == 1 && ins >= 2 {
                2
            } else {
                channels.min(2)
            } as u32;
            let mut zero = [0.0f32; 1];
            let ptrs = [zero.as_mut_ptr(); 2];
            ffi::ysfx_process_float(
                fx.0,
                ptrs.as_ptr() as *const *const f32,
                ptrs.as_ptr(),
                k,
                k,
                0,
            );
        }
        // The bypass path allocates a delay of this length; ignore absurd values.
        let latency = (unsafe { ffi::ysfx_get_pdc_delay(fx.0) } as usize).min(sample_rate as usize);
        let (sliders, params) = sliders(&fx);
        let values: Vec<f32> = sliders
            .iter()
            .map(|s| s.normalized(unsafe { ffi::ysfx_slider_get_value(fx.0, s.index) }))
            .collect();
        let fx = Arc::new(Mutex::new(fx));
        let processor = JsfxProcessor {
            fx: fx.clone(),
            sliders: sliders.clone(),
            last: values.clone(),
            mono_in: channels == 2 && ins == 1,
            mono_out: channels == 2 && outs == 1,
            // A mono chain feeds both inputs of a stereo script and keeps its first output.
            spare: if channels == 1 && ins >= 2 {
                vec![0.0; max_block]
            } else {
                Vec::new()
            },
            latency,
        };
        Ok(Instance {
            processor: Box::new(processor),
            control: Box::new(JsfxControl { fx, sliders }),
            params,
            values,
        })
    }

    struct JsfxControl {
        fx: Arc<Mutex<Fx>>,
        sliders: Vec<Slider>,
    }

    impl PluginControl for JsfxControl {
        fn value_text(&self, index: usize, value: f32) -> String {
            let Some(s) = self.sliders.get(index) else {
                return String::new();
            };
            let x = s.real(value);
            if s.choices.is_empty() {
                format!("{x:.2}")
            } else {
                let i = ((x - s.min) / s.inc.max(1e-9)).round().max(0.0) as usize;
                s.choices.get(i).cloned().unwrap_or_default()
            }
        }

        /// The processor applies values from the chain; nothing to mirror.
        fn set_value(&self, _index: usize, _value: f32) {}

        fn save_state(&self) -> Result<Vec<u8>, String> {
            let fx = self.fx.lock().unwrap();
            let s = unsafe { ffi::ysfx_save_state(fx.0) };
            if s.is_null() {
                return Err("plugin could not save its state".into());
            }
            // ysfx counts the sliders but saves only those among the first `count`
            // indices, so a sparse script like slider1 + slider64 would lose slider64.
            let mut sliders: Vec<ffi::ysfx_state_slider_t> = (0..ffi::MAX_SLIDERS)
                .filter(|&i| unsafe { ffi::ysfx_slider_exists(fx.0, i) })
                .map(|i| ffi::ysfx_state_slider_t {
                    index: i,
                    value: unsafe { ffi::ysfx_slider_get_value(fx.0, i) },
                })
                .collect();
            let out = unsafe {
                encode(&ffi::ysfx_state_t {
                    sliders: sliders.as_mut_ptr(),
                    slider_count: sliders.len() as u32,
                    data: (*s).data,
                    data_size: (*s).data_size,
                })
            };
            unsafe { ffi::ysfx_state_free(s) };
            Ok(out)
        }

        fn needs_restart(&self) -> bool {
            false
        }
    }

    struct JsfxProcessor {
        fx: Arc<Mutex<Fx>>,
        sliders: Vec<Slider>,
        last: Vec<f32>,
        mono_in: bool,
        mono_out: bool,
        spare: Vec<f32>,
        latency: usize,
    }

    impl Processor for JsfxProcessor {
        fn process(&mut self, ch: &mut [&mut [f32]], values: &[f32]) {
            let n = ch[0].len();
            // ponytail: saving state (@serialize) holds the lock; that block passes through dry.
            let Ok(fx) = self.fx.try_lock() else {
                return;
            };
            for ((&v, last), s) in values.iter().zip(&mut self.last).zip(&self.sliders) {
                if v != *last {
                    *last = v;
                    unsafe { ffi::ysfx_slider_set_value(fx.0, s.index, s.real(v)) };
                }
            }
            if self.mono_in {
                let (l, r) = ch.split_at_mut(1);
                for (a, b) in l[0].iter_mut().zip(r[0].iter()) {
                    *a = 0.5 * (*a + *b);
                }
            }
            // ysfx reads each frame's inputs before writing its outputs, so in place is fine.
            let mut ptrs = [std::ptr::null_mut(); 2];
            let k = if self.spare.is_empty() {
                for (p, c) in ptrs.iter_mut().zip(ch.iter_mut()) {
                    *p = c.as_mut_ptr();
                }
                ch.len().min(2)
            } else {
                self.spare[..n].copy_from_slice(ch[0]);
                ptrs = [ch[0].as_mut_ptr(), self.spare.as_mut_ptr()];
                2
            };
            unsafe {
                ffi::ysfx_process_float(
                    fx.0,
                    ptrs.as_ptr() as *const *const f32,
                    ptrs.as_ptr(),
                    k as u32,
                    k as u32,
                    n as u32,
                );
            }
            if self.mono_out {
                let (l, r) = ch.split_at_mut(1);
                r[0].copy_from_slice(l[0]);
            }
        }

        fn latency(&self) -> usize {
            self.latency
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oversized_scripts_are_rejected_by_the_loader() {
        let dir = std::env::temp_dir().join(format!("otozane-jsfx-load-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let big = dir.join("big.jsfx");
        let file = std::fs::File::create(&big).unwrap();
        file.set_len(MAX_SCRIPT_BYTES + 1).unwrap();
        drop(file);
        let small = dir.join("small.jsfx");
        std::fs::write(&small, "desc:test\n").unwrap();
        assert!(script_too_large(&big));
        assert!(!script_too_large(&small));
        assert!(script_too_large(&dir.join("missing.jsfx")));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn oversized_scripts_are_listed_so_the_scan_can_report_them() {
        let dir = std::env::temp_dir().join(format!("otozane-jsfx-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let big = dir.join("big.jsfx");
        let file = std::fs::File::create(&big).unwrap();
        file.set_len(MAX_SCRIPT_BYTES + 1).unwrap();
        drop(file);
        let small = dir.join("small.jsfx");
        std::fs::write(&small, "desc:test\n").unwrap();
        let found = find_scripts(&dir);
        assert!(found.contains(&small));
        assert!(found.contains(&big));
        assert!(script_too_large(&big));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
