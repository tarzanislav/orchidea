//! Sieťová časť zberača: odpovedá na hľadanie (UDP) a prijíma rámce krabičiek (TCP) – R247.

use crate::aktualizacia::Ponuky;
use crate::sklad::{Cas, Sklad};
use sklenik_korene::zber::{self, Sprava};
use std::io::Write;
use std::net::{IpAddr, Ipv6Addr, TcpListener, TcpStream, UdpSocket};
use std::sync::Arc;
use std::time::Duration;

/// Kde zberač počúva.
#[derive(Debug, Clone)]
pub struct Nastavenie {
    pub port_hladania: u16,
    pub port_prenosu: u16,
    /// Adresa počúvania (`::` = všetky rozhrania; skúšky `::1`, bez IPv6 aj `127.0.0.1`).
    pub adresa: IpAddr,
}

impl Default for Nastavenie {
    fn default() -> Nastavenie {
        Nastavenie {
            port_hladania: zber::PORT_HLADANIA,
            port_prenosu: zber::PORT_PRENOSU,
            adresa: IpAddr::V6(Ipv6Addr::UNSPECIFIED),
        }
    }
}

/// Spoločné pre vlákna zberača.
pub struct Zberac {
    pub sklad: Sklad,
    /// R264: balíky aktualizácie v `MERANIA\_aktualizacia`.
    pub ponuky: Ponuky,
    /// Kľúč domácnosti – číta sa pri každom spojení (môže vzniknúť až po štarte zberača).
    pub kluc: Box<dyn Fn() -> Option<[u8; 32]> + Send + Sync>,
    pub cas: fn() -> Cas,
    pub hlas: Box<dyn Fn(&str) + Send + Sync>,
}

impl std::fmt::Debug for Zberac {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Zberac").field("sklad", &self.sklad).finish_non_exhaustive()
    }
}

/// Otvorí sokety (pri chybe – napríklad obsadený port – vráti chybu hneď) a spustí vlákna. Vráti skutočné
/// porty (pri porte 0 ich pridelí systém – skúšky).
pub fn spusti(z: Arc<Zberac>, n: &Nastavenie) -> std::io::Result<(u16, u16)> {
    // R264: nový zberač po samoaktualizácii chvíľu počká, kým starý uvoľní porty
    let naslednik = std::env::var_os(crate::aktualizacia::NASLEDNIK).is_some();
    let udp = viaz(|| UdpSocket::bind((n.adresa, n.port_hladania)), naslednik)?;
    // Windows doručí správu na skupinovú adresu len soketu, ktorý sa k nej pripojil – na každom rozhraní
    // (číslo rozhrania nepoznáme, preto skúsime prvých 128; neexistujúce vrátia chybu)
    if n.adresa == IpAddr::V6(Ipv6Addr::UNSPECIFIED) {
        let vsetky = Ipv6Addr::new(0xff02, 0, 0, 0, 0, 0, 0, 1);
        for i in 0..128 {
            let _ = udp.join_multicast_v6(&vsetky, i);
        }
    }
    let tcp = viaz(|| TcpListener::bind((n.adresa, n.port_prenosu)), naslednik)?;
    let port_prenosu = tcp.local_addr()?.port();
    let port_hladania = udp.local_addr()?.port();
    std::thread::Builder::new().name("hladanie".into()).spawn(move || {
        let mut b = [0u8; 512];
        loop {
            let Ok((d, od)) = udp.recv_from(&mut b) else { continue };
            if b[..d].starts_with(zber::HLADAM.as_bytes()) {
                let _ = udp.send_to(format!("{} {port_prenosu}", zber::SOM).as_bytes(), od);
            }
        }
    })?;
    std::thread::Builder::new().name("prenos".into()).spawn(move || {
        for spojenie in tcp.incoming().flatten() {
            let z = z.clone();
            let _ = std::thread::Builder::new().name("spojenie".into()).spawn(move || obsluz(&z, spojenie));
        }
    })?;
    Ok((port_hladania, port_prenosu))
}

/// Otvorí soket; nástupca po samoaktualizácii skúša až 20 s (starý zberač práve končí).
fn viaz<T>(f: impl Fn() -> std::io::Result<T>, naslednik: bool) -> std::io::Result<T> {
    let od = std::time::Instant::now();
    loop {
        match f() {
            Err(e) if naslednik && od.elapsed() < Duration::from_secs(20) => {
                let _ = e;
                std::thread::sleep(Duration::from_millis(250));
            }
            r => return r,
        }
    }
}

/// R264: verzia krabičky – posledné slovo poľa `verzia` (karta posiela krátky odtlačok commitu).
fn verzia_krabicky(s: &Sprava) -> &str {
    s.pole("verzia").and_then(|v| v.split_whitespace().last()).unwrap_or("")
}

/// R264: ponuka aktualizácie pre krabičku, ktorá ju vie prijať (`aktualizacia=1`) – text za „ok“.
fn ponuka(z: &Zberac, s: &Sprava) -> String {
    if s.pole("aktualizacia") != Some("1") {
        return String::new();
    }
    let arch = s.pole("arch").unwrap_or("");
    let cas = s.pole("cas").and_then(|c| c.parse().ok()).unwrap_or(0);
    let verzia = verzia_krabicky(s);
    let Some(b) = z.ponuky.ponuka("klic", arch, verzia, cas) else { return String::new() };
    let kto = z.sklad.kto(s);
    if z.ponuky.prvy_raz(&kto, &b.verzia) {
        (z.hlas)(&format!(
            "{kto}: ponúkam aktualizáciu {verzia} → {} ({:.1} MB, R264)",
            b.verzia,
            b.spolu() as f64 / 1_048_576.0
        ));
    }
    format!(" nova={}", b.verzia)
}

/// R264: žiadosť krabičky o súbory aktualizácie – odpoveď bajtmi.
fn ziadost_bajty(z: &Zberac, s: &Sprava) -> Result<Vec<u8>, String> {
    let (druh, arch, verzia) =
        (s.pole("druh").unwrap_or("klic"), s.pole("arch").unwrap_or(""), s.pole("nova").unwrap_or(""));
    match s.pole("ziadost") {
        Some("popis") => z.ponuky.popis(druh, arch, verzia),
        Some("kus") => {
            let cesta = s.pole("subor").unwrap_or("");
            let od: u64 = s.pole("od").and_then(|x| x.parse().ok()).ok_or("bez začiatku kusu")?;
            let kolko: usize = s.pole("kolko").and_then(|x| x.parse().ok()).ok_or("bez dĺžky kusu")?;
            let d = z.ponuky.kus(druh, arch, verzia, cesta, od, kolko)?;
            if od == 0 && cesta == "init" {
                (z.hlas)(&format!("{}: sťahuje aktualizáciu {verzia}", z.sklad.kto(s)));
            }
            Ok(d)
        }
        _ => Err("neznáma žiadosť".into()),
    }
}

fn obsluz(z: &Zberac, mut t: TcpStream) {
    let od = t.peer_addr().map(|a| a.to_string()).unwrap_or_default();
    let _ = t.set_read_timeout(Some(Duration::from_secs(30)));
    let _ = t.set_write_timeout(Some(Duration::from_secs(30)));
    // jedno spojenie môže niesť viac rámcov za sebou
    while let Ok(ramec) = zber::citaj_ramec(&mut t) {
        let Some(kluc) = (z.kluc)() else {
            (z.hlas)(&format!("{od}: kľúč domácnosti ešte nie je – najprv zapíšte kartu Zapisovačom"));
            return;
        };
        let (nonce, data) = match zber::rozbal(&kluc, &ramec) {
            Ok(x) => x,
            Err(e) => {
                (z.hlas)(&format!("{od}: {e}"));
                return;
            }
        };
        let sprava = match Sprava::z_bajtov(&data) {
            Ok(s) => s,
            Err(e) => {
                let _ = t.write_all(&zber::odpoved(&kluc, &nonce, &format!("chyba {e}")));
                (z.hlas)(&format!("{od}: {e}"));
                return;
            }
        };
        // R264: žiadosti o aktualizáciu sa neukladajú ako záznamy
        let (ramec, popis) = match sprava.pole("ziadost") {
            Some("popis" | "kus") => {
                let r = ziadost_bajty(z, &sprava);
                let popis = r.as_ref().err().map(|e| format!("{}: aktualizácia – {e}", z.sklad.kto(&sprava)));
                (zber::odpoved_bajty(&kluc, &nonce, r.as_deref().map_err(String::as_str)), popis)
            }
            Some("ponuka") => (zber::odpoved(&kluc, &nonce, &format!("ok{}", ponuka(z, &sprava))), None),
            Some("hlasenie") => {
                let kto = z.sklad.kto(&sprava);
                let text = sprava.pole("text").unwrap_or("").to_string();
                let _ = z
                    .sklad
                    .zapis_do_prehladu(&format!("{} · {kto} · {text}", crate::sklad::cas_text((z.cas)())));
                (zber::odpoved(&kluc, &nonce, "ok"), Some(format!("{kto}: {text}")))
            }
            Some(x) => (zber::odpoved(&kluc, &nonce, &format!("chyba neznáma žiadosť {x}")), None),
            None => match z.sklad.prijmi(&sprava, (z.cas)()) {
                Ok((_, popis)) => {
                    (zber::odpoved(&kluc, &nonce, &format!("ok{}", ponuka(z, &sprava))), Some(popis))
                }
                Err(e) => (
                    zber::odpoved(&kluc, &nonce, &format!("chyba {e}")),
                    Some(format!("{od}: uloženie zlyhalo – {e}")),
                ),
            },
        };
        // R253: najprv potvrdenie krabičke, až potom riadok do okna – zaseknuté okno (Windows zastaví výpis,
        // keď v ňom používateľ označuje text) nesmie zastaviť prenos (X96 27. 11.: záznam uložený, potvrdenie
        // neodišlo, HOTOVÉ ostalo v rade krabičky)
        let poslane = t.write_all(&ramec);
        if let Some(p) = popis {
            (z.hlas)(&p);
        }
        if poslane.is_err() {
            return;
        }
    }
}
