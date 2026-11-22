//! Checks JSFX hosting against tests/gain.jsfx. Skipped without third_party/ysfx.
use otozane_plugins::{instantiate, jsfx};
use std::path::Path;

#[test]
fn jsfx_gain_changes_the_signal_and_restores_state() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let script = dir.join("gain.jsfx");
    assert!(jsfx::find_scripts(&dir).contains(&script));
    let (plugin, category) = match jsfx::scan(&script) {
        Err(e) if e.contains("without JSFX") => return eprintln!("skipped: {e}"),
        r => r.unwrap(),
    };
    assert_eq!(
        (plugin.name.as_str(), category.as_str()),
        ("Test Gain", "utility|gain")
    );

    let mut inst = instantiate(&plugin, None, 48000.0, 2, 1024).unwrap();
    assert_eq!(inst.params.len(), 1);
    let mut values = inst.values.clone();
    values[0] = 0.0; // -60 dB
    let (mut l, mut r) = (vec![0.5f32; 480], vec![0.5f32; 480]);
    inst.processor.process(&mut [&mut l, &mut r], &values);
    assert!(
        (l[0] - 0.0005).abs() < 1e-5 && (r[479] - 0.0005).abs() < 1e-5,
        "{}",
        l[0]
    );
    assert_eq!(inst.control.value_text(0, 0.0), "-60.00");

    let state = inst.control.save_state().unwrap();
    drop(inst);
    // Mono chain: the stereo script gets the signal on both inputs, output 0 is kept.
    let mut again = instantiate(&plugin, Some(&state), 48000.0, 1, 1024).unwrap();
    assert_eq!(again.values[0], 0.0, "state not restored");
    let mut m = vec![0.5f32; 480];
    again.processor.process(&mut [&mut m], &values);
    assert!((m[0] - 0.0005).abs() < 1e-5, "{}", m[0]);
}
