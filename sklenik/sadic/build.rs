//! Na Windows si SADIČ pri spustení vyžiada práva správcu (zápis na disk bez nich nie je možný)
//! a zapne moderný vzhľad prvkov okna.

fn main() {
    let ciel = std::env::var("TARGET").unwrap_or_default();
    if ciel.contains("windows-msvc") {
        println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg-bins=/MANIFESTUAC:level='requireAdministrator'");
        // moderný vzhľad prvkov okna (Common Controls 6 – Windows 10 a 11)
        println!(
            "cargo:rustc-link-arg-bins=/MANIFESTDEPENDENCY:type='win32' name='Microsoft.Windows.Common-Controls' \
             version='6.0.0.0' processorArchitecture='*' publicKeyToken='6595b64144ccf1df' language='*'"
        );
    }
}
