//! Lists the effects in a VST3 bundle: `cargo run --example scan -- PATH`.
fn main() {
    let path = std::env::args().nth(1).expect("usage: scan PATH");
    match otozane_plugins::vst3::scan(std::path::Path::new(&path)) {
        Ok(list) => {
            for (p, sub) in list {
                println!("{} | {} | {} | {}", p.uid, p.name, p.vendor, sub);
            }
        }
        Err(e) => eprintln!("error: {e}"),
    }
}
