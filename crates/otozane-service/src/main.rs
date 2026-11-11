use otozane_core::config::{EffectConfig, Profile};
use otozane_core::effects::EffectKind;
use otozane_core::{SAMPLE_RATE, engine};
use std::sync::atomic::Ordering;
use std::time::Duration;

fn main() {
    let input = std::env::args().nth(1);
    let (mut ctl, eng) = engine::new(SAMPLE_RATE as f32, 1);
    let mut profile = Profile {
        chain: vec![EffectConfig::new(1, EffectKind::Gain)],
    };
    profile.chain[0].params.insert("gain".into(), 6.0);
    ctl.set_chain(&profile.chain)
        .expect("fresh engine accepts a chain");
    let backend = otozane_pipewire::Backend::start(eng, input);
    loop {
        std::thread::sleep(Duration::from_secs(1));
        ctl.collect_garbage();
        let st = backend.status.lock().unwrap().clone();
        let c = &backend.counters;
        eprintln!(
            "{:?} vsrc={} blocks={} in={:.3} out={:.3} q={}/{} under={} over={} skip={} fill={}",
            st.capture,
            st.virtual_source_ready,
            ctl.shared.blocks.load(Ordering::Relaxed),
            ctl.shared.input_peak.swap(0.0),
            ctl.shared.output_peak.swap(0.0),
            c.capture_quantum.load(Ordering::Relaxed),
            c.playback_quantum.load(Ordering::Relaxed),
            c.underrun_frames.load(Ordering::Relaxed),
            c.overrun_frames.load(Ordering::Relaxed),
            c.skipped_frames.load(Ordering::Relaxed),
            c.ring_fill.load(Ordering::Relaxed),
        );
    }
}
