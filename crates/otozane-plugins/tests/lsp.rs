//! Checks against LSP plugins (`lsp-plugins-vst3`). Skipped when not installed.
use otozane_plugins::{instantiate, scan_bundle};
use std::path::Path;

const BUNDLE: &str = "/usr/lib/vst3/lsp-plugins.vst3";

fn peak_of_sine(inst: &mut otozane_plugins::Instance, values: &[f32]) -> f32 {
    let mut peak = 0f32;
    for block in 0..100 {
        let mut buf: Vec<f32> = (0..480)
            .map(|i| 0.5 * ((block * 480 + i) as f32 * 0.1309).sin())
            .collect();
        inst.processor.process(&mut [&mut buf], values);
        if block >= 50 {
            peak = buf.iter().fold(peak, |m, x| m.max(x.abs()));
        }
    }
    peak
}

#[test]
fn lsp_limiter_processes_and_restores_state() {
    if !Path::new(BUNDLE).exists() {
        eprintln!("skipped: {BUNDLE} not installed");
        return;
    }
    let list = scan_bundle(Path::new(BUNDLE)).unwrap();
    let info = list
        .iter()
        .find(|i| i.plugin.name == "Limiter Mono")
        .unwrap();
    let mut inst = instantiate(&info.plugin, None, 48000.0, 1, 1024).unwrap();
    assert_eq!(inst.processor.latency(), 240, "5 ms lookahead at 48 kHz");
    let out = inst
        .params
        .iter()
        .position(|p| p.name == "Output gain")
        .unwrap();
    let mut values = inst.values.clone();
    values[out] = 0.3; // about -38 dB
    let peak = peak_of_sine(&mut inst, &values);
    assert!(peak < 0.01, "output gain not applied: {peak}");

    let state = inst.control.save_state().unwrap();
    drop(inst);
    let again = instantiate(&info.plugin, Some(&state), 48000.0, 1, 1024).unwrap();
    assert!((again.values[out] - 0.3).abs() < 1e-4, "state not restored");
}
