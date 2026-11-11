//! Loads one effect, prints its parameters and the peak of a processed 1 kHz sine:
//! `cargo run --example probe -- BUNDLE NAME [param-index=value ...]`.
use otozane_plugins::{instantiate, vst3};

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("usage: probe BUNDLE NAME");
    let name = args.next().expect("usage: probe BUNDLE NAME");
    let classes = vst3::scan(std::path::Path::new(&path)).expect("scan");
    let (plugin, _) = classes
        .into_iter()
        .find(|(p, _)| p.name == name)
        .expect("no such class");
    let mut inst = instantiate(&plugin, None, 48000.0, 1, 1024).expect("instantiate");
    let mut values = inst.values.clone();
    for a in args {
        let (i, v) = a.split_once('=').expect("index=value");
        values[i.parse::<usize>().unwrap()] = v.parse().unwrap();
    }
    for (i, (d, v)) in inst.params.iter().zip(&values).enumerate() {
        println!(
            "{i:3} {:>6} {:<28} {:.3} = {} {}",
            d.key,
            d.name,
            v,
            inst.control.value_text(i, *v),
            d.unit
        );
    }
    println!("latency {}", inst.processor.latency());
    let mut peak = 0f32;
    for block in 0..100 {
        let mut buf: Vec<f32> = (0..480)
            .map(|i| {
                0.5 * ((block * 480 + i) as f32 * 2.0 * std::f32::consts::PI * 1000.0 / 48000.0)
                    .sin()
            })
            .collect();
        inst.processor.process(&mut [&mut buf], &values);
        if block >= 50 {
            peak = buf.iter().fold(peak, |m, x| m.max(x.abs()));
        }
    }
    println!("input peak 0.500, output peak {peak:.3}");
    let state = inst.control.save_state().expect("state");
    println!("state {} bytes", state.len());
    let _keep = std::env::var_os("KEEP").is_some().then_some(inst);
    let again = instantiate(&plugin, Some(&state), 48000.0, 1, 1024).expect("reload");
    let diff = again
        .values
        .iter()
        .zip(&values)
        .filter(|(a, b)| (*a - *b).abs() > 1e-4)
        .count();
    println!(
        "reloaded with state: {diff} values differ from what was set, latency {}",
        again.processor.latency()
    );
}
