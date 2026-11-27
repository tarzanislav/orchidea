//! Značky súboru pre rozhodcu (`media/nastroje/over_znacky.py`): `media_znacky <súbor>` otvorí súbor cez
//! `kontajner::otvor` a vypíše `FORMAT <formát>`, každú položku ako `ZNACKA <kľúč>\t<hodnota>` (koniec riadka
//! v hodnote ako `\n`) a ReplayGain ako `REPLAYGAIN stopa_db=… stopa_vrchol=… album_db=… album_vrchol=…`
//! (chýbajúca hodnota = `-`).

use orchidea_media::limity::Limity;
use orchidea_media::zdroj::SuborovyZdroj;

fn main() {
    let cesta = std::env::args().nth(1).expect("použitie: media_znacky <súbor>");
    let k = orchidea_media::kontajner::otvor(SuborovyZdroj::otvor(&cesta).expect("súbor"), Limity::default())
        .expect("kontajner");
    println!("FORMAT {:?}", k.format());
    let Some(z) = k.znacky() else { return };
    for (kluc, hodnota) in z.polozky() {
        println!("ZNACKA {kluc}\t{}", hodnota.replace('\n', "\\n"));
    }
    if let Some(rg) = z.replaygain() {
        let f = |x: Option<f32>| x.map_or("-".to_string(), |v| format!("{v}"));
        println!(
            "REPLAYGAIN stopa_db={} stopa_vrchol={} album_db={} album_vrchol={}",
            f(rg.stopa_db),
            f(rg.stopa_vrchol),
            f(rg.album_db),
            f(rg.album_vrchol)
        );
    }
}
