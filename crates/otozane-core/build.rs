fn main() {
    // RNNoise is linked from the system library (Arch Linux: `rnnoise` package).
    println!("cargo:rustc-link-lib=rnnoise");
}
