//! Na Windows si SADIČ pri spustení vyžiada práva správcu (zápis na disk bez nich nie je možný).

fn main() {
    let ciel = std::env::var("TARGET").unwrap_or_default();
    if ciel.contains("windows-msvc") {
        println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg-bins=/MANIFESTUAC:level='requireAdministrator'");
    }
}
