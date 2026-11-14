fn main() {
    // launchd starts otozaned on its own, without the app bundle around it, so
    // macOS reads the microphone usage text from a plist embedded in the binary.
    // Without it the system denies the microphone instead of asking.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        let plist = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../packaging/macos/otozaned-Info.plist");
        println!("cargo:rerun-if-changed={}", plist.display());
        println!(
            "cargo:rustc-link-arg-bins=-Wl,-sectcreate,__TEXT,__info_plist,{}",
            plist.display()
        );
    }
}
