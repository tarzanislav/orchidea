//! Prehliadač a editor fotografií (Pohľad). `pohlad [súbor | priečinok]`; súbor alebo priečinok sa dá aj pretiahnuť do okna.

#![cfg_attr(windows, windows_subsystem = "windows")]

fn main() {
    let argumenty: Vec<String> = std::env::args().skip(1).collect();
    #[cfg(windows)]
    {
        std::process::exit(orchidea_pohlad::okno_win::spusti(&argumenty));
    }
    #[cfg(not(windows))]
    {
        let _ = argumenty;
        eprintln!(
            "Prehliadač a editor fotografií (Pohľad): okno je zatiaľ pre Windows. Na Linuxe a macOS použite príkazový riadok \
             (cargo run -p orchidea-pohlad --example prikazy)."
        );
        std::process::exit(1);
    }
}
