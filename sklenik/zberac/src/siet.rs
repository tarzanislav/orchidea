//! Sieťová časť zberača: odpovedá na hľadanie (UDP) a prijíma rámce krabičiek (TCP) – R247.

use crate::sklad::{Cas, Sklad};
use sklenik_korene::zber::{self, Sprava};
use std::io::Write;
use std::net::{Ipv6Addr, TcpListener, TcpStream, UdpSocket};
use std::sync::Arc;
use std::time::Duration;

/// Kde zberač počúva.
#[derive(Debug, Clone)]
pub struct Nastavenie {
    pub port_hladania: u16,
    pub port_prenosu: u16,
    /// Adresa počúvania (`::` = všetky rozhrania; skúšky `::1`).
    pub adresa: Ipv6Addr,
}

impl Default for Nastavenie {
    fn default() -> Nastavenie {
        Nastavenie {
            port_hladania: zber::PORT_HLADANIA,
            port_prenosu: zber::PORT_PRENOSU,
            adresa: Ipv6Addr::UNSPECIFIED,
        }
    }
}

/// Spoločné pre vlákna zberača.
pub struct Zberac {
    pub sklad: Sklad,
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
    let udp = UdpSocket::bind((n.adresa, n.port_hladania))?;
    // Windows doručí správu na skupinovú adresu len soketu, ktorý sa k nej pripojil – na každom rozhraní
    // (číslo rozhrania nepoznáme, preto skúsime prvých 128; neexistujúce vrátia chybu)
    if n.adresa.is_unspecified() {
        let vsetky = Ipv6Addr::new(0xff02, 0, 0, 0, 0, 0, 0, 1);
        for i in 0..128 {
            let _ = udp.join_multicast_v6(&vsetky, i);
        }
    }
    let tcp = TcpListener::bind((n.adresa, n.port_prenosu))?;
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
        let odpoved = match Sprava::z_bajtov(&data)
            .map_err(std::io::Error::other)
            .and_then(|s| z.sklad.prijmi(&s, (z.cas)()))
        {
            Ok((_, popis)) => {
                (z.hlas)(&popis);
                "ok".to_string()
            }
            Err(e) => {
                (z.hlas)(&format!("{od}: uloženie zlyhalo – {e}"));
                format!("chyba {e}")
            }
        };
        if t.write_all(&zber::odpoved(&kluc, &nonce, &odpoved)).is_err() {
            return;
        }
    }
}
