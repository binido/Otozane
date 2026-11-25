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

#[test]
fn jsfx_sparse_sliders_num_ch_and_step_rounding() {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/sparse.jsfx");
    let plugin = match jsfx::scan(&script) {
        Err(e) if e.contains("without JSFX") => return eprintln!("skipped: {e}"),
        r => r.unwrap().0,
    };
    let mut inst = instantiate(&plugin, None, 48000.0, 2, 1024).unwrap();
    // 0..10 in steps of 6 must not round up to 12.
    assert_eq!(inst.control.value_text(0, 1.0), "10.00");

    // @slider ran with num_ch = 2 before the first block: gain 2 * 1.
    let mut values = inst.values.clone();
    let (mut l, mut r) = (vec![0.5f32; 64], vec![0.5f32; 64]);
    inst.processor.process(&mut [&mut l, &mut r], &values);
    assert_eq!(l[0], 1.0, "num_ch was 0 in the first @slider");

    values[1] = 0.75; // slider64 = 3
    inst.processor.process(&mut [&mut l, &mut r], &values);
    let state = inst.control.save_state().unwrap();
    drop(inst);
    let again = instantiate(&plugin, Some(&state), 48000.0, 2, 1024).unwrap();
    assert_eq!(again.values[1], 0.75, "slider64 lost in the saved state");
}

#[test]
fn jsfx_oversized_import_is_rejected_on_load() {
    let dir = std::env::temp_dir().join(format!("otozane-jsfx-import-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let big = dir.join("big.jsfx");
    std::fs::File::create(&big)
        .unwrap()
        .set_len(2 << 20)
        .unwrap();
    let entry = dir.join("entry.jsfx");
    std::fs::write(&entry, "desc:Import\nimport big.jsfx\n@init\n").unwrap();
    let plugin = match jsfx::scan(&entry) {
        Err(e) if e.contains("without JSFX") => return eprintln!("skipped: {e}"),
        r => r.unwrap().0,
    };

    let Err(err) = instantiate(&plugin, None, 48000.0, 1, 1024) else {
        panic!("oversized import was accepted");
    };
    assert!(err.contains("big.jsfx"), "{err}");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn jsfx_imports_compile_and_run() {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/import-main.jsfx");
    let plugin = match jsfx::scan(&script) {
        Err(e) if e.contains("without JSFX") => return eprintln!("skipped: {e}"),
        r => r.unwrap().0,
    };
    let mut inst = instantiate(&plugin, None, 48000.0, 2, 1024).unwrap();
    let values = inst.values.clone();
    let (mut l, mut r) = (vec![1.0f32; 64], vec![1.0f32; 64]);
    inst.processor.process(&mut [&mut l, &mut r], &values);
    assert_eq!((l[0], r[63]), (0.5, 0.5), "imported @init did not run");
}

#[cfg(unix)]
#[test]
fn jsfx_symlinked_import_is_followed() {
    let dir = std::env::temp_dir().join(format!("otozane-jsfx-link-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("real.jsfx"),
        "desc:Linked\n@init\nlinked_gain = 0.25;\n",
    )
    .unwrap();
    std::os::unix::fs::symlink("real.jsfx", dir.join("link.jsfx")).unwrap();
    let entry = dir.join("entry.jsfx");
    std::fs::write(
        &entry,
        "desc:Symlink\nimport link.jsfx\n@sample\nspl0 *= linked_gain;\nspl1 *= linked_gain;\n",
    )
    .unwrap();
    let plugin = match jsfx::scan(&entry) {
        Err(e) if e.contains("without JSFX") => return eprintln!("skipped: {e}"),
        r => r.unwrap().0,
    };

    let mut inst = instantiate(&plugin, None, 48000.0, 2, 1024).unwrap();
    let values = inst.values.clone();
    let (mut l, mut r) = (vec![1.0f32; 64], vec![1.0f32; 64]);
    inst.processor.process(&mut [&mut l, &mut r], &values);
    assert_eq!(
        (l[0], r[63]),
        (0.25, 0.25),
        "symlinked import was not followed"
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn jsfx_source_total_budget_is_enforced() {
    let dir = std::env::temp_dir().join(format!("otozane-jsfx-budget-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let filler = vec![b'a'; (1 << 20) - 1];
    let mut header = String::from("desc:Budget\n");
    for i in 0..9 {
        std::fs::write(dir.join(format!("part{i}.jsfx")), &filler).unwrap();
        header.push_str(&format!("import part{i}.jsfx\n"));
    }
    let entry = dir.join("entry.jsfx");
    std::fs::write(&entry, header).unwrap();
    let plugin = match jsfx::scan(&entry) {
        Err(e) if e.contains("without JSFX") => return eprintln!("skipped: {e}"),
        r => r.unwrap().0,
    };

    let Err(err) = instantiate(&plugin, None, 48000.0, 1, 1024) else {
        panic!("oversized import set was accepted");
    };
    assert!(err.contains("load budget"), "{err}");
    std::fs::remove_dir_all(dir).unwrap();
}

#[cfg(unix)]
#[test]
fn jsfx_fifo_import_is_rejected_without_blocking() {
    use std::sync::mpsc;
    use std::time::Duration;

    let dir = std::env::temp_dir().join(format!("otozane-jsfx-fifo-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    assert!(
        std::process::Command::new("mkfifo")
            .arg(dir.join("blocked.jsfx"))
            .status()
            .unwrap()
            .success()
    );
    let entry = dir.join("entry.jsfx");
    std::fs::write(&entry, "desc:Fifo\nimport blocked.jsfx\n@init\n").unwrap();
    let plugin = match jsfx::scan(&entry) {
        Err(e) if e.contains("without JSFX") => return eprintln!("skipped: {e}"),
        r => r.unwrap().0,
    };

    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(instantiate(&plugin, None, 48000.0, 1, 1024).err());
    });
    match rx.recv_timeout(Duration::from_secs(5)) {
        Ok(Some(err)) => assert!(err.contains("not a regular file"), "{err}"),
        Ok(None) => panic!("FIFO import was accepted"),
        Err(_) => panic!("opening a FIFO import blocked"),
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn jsfx_non_regular_import_is_rejected() {
    let dir = std::env::temp_dir().join(format!("otozane-jsfx-type-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::create_dir(dir.join("blocked.jsfx")).unwrap();
    let entry = dir.join("entry.jsfx");
    std::fs::write(&entry, "desc:Blocked\nimport blocked.jsfx\n@init\n").unwrap();
    let plugin = match jsfx::scan(&entry) {
        Err(e) if e.contains("without JSFX") => return eprintln!("skipped: {e}"),
        r => r.unwrap().0,
    };

    let Err(err) = instantiate(&plugin, None, 48000.0, 1, 1024) else {
        panic!("directory import was accepted");
    };
    assert!(err.contains("not a regular file"), "{err}");
    std::fs::remove_dir_all(dir).unwrap();
}
