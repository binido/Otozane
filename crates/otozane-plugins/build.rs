fn main() {
    // ysfx (JSFX compiler and runtime) is built from third_party/ysfx, fetched by
    // scripts/fetch-ysfx.sh. Without it, JSFX plugins fail to load with an error.
    println!("cargo::rustc-check-cfg=cfg(ysfx)");
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../third_party/ysfx");
    if !src.join("thirdparty/dr_libs/dr_wav.h").exists() {
        // Fetching the sources touches third_party.
        println!("cargo:rerun-if-changed={}", src.parent().unwrap().display());
        println!(
            "cargo:warning=third_party/ysfx not found, building without JSFX support (run scripts/fetch-ysfx.sh)"
        );
        return;
    }
    println!("cargo:rerun-if-changed={}", src.display());
    // third_party is ignored, so the tracked patch is what makes the source
    // bounds part of the build instead of a local edit.
    let patch =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("patches/ysfx-source-budget.patch");
    println!("cargo:rerun-if-changed={}", patch.display());
    apply_patch(&src, &patch);
    let out = cmake::Config::new(&src)
        .profile("Release")
        .define("YSFX_PLUGIN", "OFF")
        .define("YSFX_GFX", "OFF")
        .define("YSFX_TESTS", "OFF")
        .define("YSFX_TOOLS", "OFF")
        .build_target("ysfx")
        .build();
    println!(
        "cargo:rustc-link-search=native={}",
        out.join("build").display()
    );
    println!("cargo:rustc-link-lib=static=ysfx");
    let cxx = if cfg!(target_os = "macos") {
        "c++"
    } else {
        "stdc++"
    };
    println!("cargo:rustc-link-lib={cxx}");
    println!("cargo:rustc-cfg=ysfx");
}

fn apply_patch(src: &std::path::Path, patch: &std::path::Path) {
    // Reverse-check tells an already-patched tree from a pristine one.
    let applied = std::process::Command::new("git")
        .arg("-C")
        .arg(src)
        .args(["apply", "--reverse", "--check"])
        .arg(patch)
        .status()
        .is_ok_and(|s| s.success());
    if applied {
        return;
    }
    let status = std::process::Command::new("git")
        .arg("-C")
        .arg(src)
        .arg("apply")
        .arg(patch)
        .status();
    if !status.is_ok_and(|s| s.success()) {
        panic!(
            "cannot apply {} to {}: the fetched sources have conflicting edits; restore them, then rerun scripts/fetch-ysfx.sh",
            patch.display(),
            src.display()
        );
    }
}
