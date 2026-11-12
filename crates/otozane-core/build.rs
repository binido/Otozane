fn main() {
    // RNNoise comes from the system library (Arch Linux: `rnnoise` package).
    // Release builds set RNNOISE_STATIC_DIR to a directory with librnnoise.a,
    // because Debian 12 and Ubuntu do not package RNNoise.
    println!("cargo:rerun-if-env-changed=RNNOISE_STATIC_DIR");
    match std::env::var("RNNOISE_STATIC_DIR") {
        Ok(dir) => {
            println!("cargo:rustc-link-search=native={dir}");
            println!("cargo:rustc-link-lib=static=rnnoise");
        }
        Err(_) => println!("cargo:rustc-link-lib=rnnoise"),
    }
}
