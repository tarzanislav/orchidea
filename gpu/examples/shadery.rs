//! Zapíše všetky štandardné kernely vo všetkých jazykoch do priečinka (pre nezávislé overenie v CI).
//! Spustenie: `cargo run -p orchidea-gpu --example shadery -- <priečinok>`

use orchidea_gpu::texty::{Jazyk, do_textu};
use orchidea_gpu::{kernely, spirv};

fn main() {
    let ciel = std::env::args().nth(1).unwrap_or_else(|| "shadery".into());
    std::fs::create_dir_all(&ciel).expect("priečinok");
    for (i, k) in kernely::vsetky().iter().enumerate() {
        let meno = format!("{ciel}/k{i}");
        std::fs::write(format!("{meno}.spv"), spirv::do_bajtov(&spirv::do_spirv(k).expect("SPIR-V")))
            .expect("zápis");
        std::fs::write(format!("{meno}.hlsl"), do_textu(k, Jazyk::Hlsl).expect("HLSL")).expect("zápis");
        std::fs::write(format!("{meno}.metal"), do_textu(k, Jazyk::Msl).expect("MSL")).expect("zápis");
        std::fs::write(format!("{meno}.wgsl"), do_textu(k, Jazyk::Wgsl).expect("WGSL")).expect("zápis");
        println!("k{i}: {}", k.meno);
    }
}
