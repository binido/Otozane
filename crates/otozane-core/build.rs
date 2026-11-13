fn main() {
    // RNNoise comes from the system library (Arch Linux: `rnnoise` package).
    // Release builds set RNNOISE_STATIC_DIR to a directory with librnnoise.a,
    // because Debian 12, Ubuntu and Homebrew do not package RNNoise.
    // Without it, third_party/rnnoise from scripts/build-rnnoise.sh is used when present.
    println!("cargo:rerun-if-env-changed=RNNOISE_STATIC_DIR");
    let local = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../third_party/rnnoise");
    println!("cargo:rerun-if-changed={}", local.display());
    let dir = std::env::var("RNNOISE_STATIC_DIR").ok().or_else(|| {
        local
            .join("librnnoise.a")
            .exists()
            .then(|| local.display().to_string())
    });
    match dir {
        Some(dir) => {
            println!("cargo:rustc-link-search=native={dir}");
            println!("cargo:rustc-link-lib=static=rnnoise");
        }
        None => println!("cargo:rustc-link-lib=rnnoise"),
    }
}
