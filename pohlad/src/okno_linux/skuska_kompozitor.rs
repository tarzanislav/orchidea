//! Skúška okna so zdanlivým kompozitorom Wayland: na druhom konci zásuvky odpovedá ako kompozitor,
//! číta obraz okna zo zdieľanej pamäte a posiela kláves, pohyb a kliknutie myši (v CI nie je ani
//! klávesnica, ani myš). Overí: zoznam rozhraní, zdieľanú pamäť s popisovačmi, configure/ack, snímky,
//! rozloženie klávesnice cez popisovač, povel klávesu „r“ (otočenie), vlastnú lištu (zvýraznenie
//! „zavrieť“ pod myšou) a zavretie okna kliknutím.

use super::sys;
use super::wayland::{Citac, Skladac, Sprava};
use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::fs::FileExt;
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

const ROZLOZENIE: &str = r#"xkb_keymap {
xkb_keycodes "skuska" { <ESC> = 9; <AD04> = 27; };
xkb_types "skuska" { type "ALPHABETIC" { modifiers= Shift+Lock; map[Shift]= 2; map[Lock]= 2; }; };
xkb_symbols "skuska" { key <ESC> { [ Escape ] }; key <AD04> { [ r, R ] }; };
};"#;

struct Server {
    z: UnixStream,
    serial: u32,
}

impl Server {
    fn posli(&mut self, s: Sprava) {
        sys::posli(&self.z, &s.bajty(), &[]).unwrap();
    }
    fn serial(&mut self) -> u32 {
        self.serial += 1;
        self.serial
    }
}

fn f(x: f64) -> i32 {
    (x * 256.0) as i32
}

/// Obdĺžnik červených bodov (obrázok) v obraze okna.
fn cervene(b: &[u32], w: usize) -> Option<(usize, usize)> {
    let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
    for (i, &p) in b.iter().enumerate() {
        let (r, g, bl) = ((p >> 16) & 0xff, (p >> 8) & 0xff, p & 0xff);
        if r > 200 && g < 60 && bl < 60 {
            let (x, y) = (i % w, i / w);
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
        }
    }
    (x1 >= x0 && x0 != usize::MAX).then(|| (x1 - x0 + 1, y1 - y0 + 1))
}

#[test]
fn okno_so_zdanlivym_kompozitorom() {
    use orchidea_formaty::obraz::{Model, Obraz, Vzorky};
    let cas = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let priecinok = std::env::temp_dir().join(format!("pohlad_wayland_{cas}"));
    std::fs::create_dir_all(&priecinok).unwrap();
    // červený obrázok 60 × 20 – po otočení je vyšší ako širší
    let mut body = Vec::new();
    for _ in 0..60 * 20 {
        body.extend_from_slice(&[255, 0, 0]);
    }
    let o = Obraz::novy(60, 20, Model::Rgb, false, Vzorky::U8(body)).unwrap();
    std::fs::write(
        priecinok.join("cerveny.png"),
        orchidea_formaty::zapis(&o, orchidea_formaty::Format::Png).unwrap(),
    )
    .unwrap();

    let (klient, server) = UnixStream::pair().unwrap();
    let argumenty = vec![priecinok.to_string_lossy().into_owned()];
    let okno = std::thread::spawn(move || super::spusti_na(klient, &argumenty, None));

    server.set_read_timeout(Some(Duration::from_millis(50))).unwrap();
    let mut s = Server { z: server.try_clone().unwrap(), serial: 100 };
    let mut sk = Skladac::default();
    let mut fds: std::collections::VecDeque<OwnedFd> = Default::default();
    let mut buf = vec![0u8; 65536];
    let mut rozhrania: HashMap<u32, String> = HashMap::from([(1, "wl_display".to_string())]);
    let mut zasobniky: HashMap<u32, File> = HashMap::new();
    let mut obrazy: HashMap<u32, (u32, usize, usize, usize, usize)> = HashMap::new();
    let (mut okno_surface, mut xdg_surface, mut toplevel, mut pointer, mut keyboard) = (0, 0, 0, 0, 0);
    let (mut pripojeny, mut snimok, mut nakonfigurovane) = (0u32, 0u32, false);
    let mut titulky = Vec::new();
    let mut ack = false;
    let mut krok = 0;
    let mut snimkov = 0;
    let mut pred_otocenim = None;
    let zaciatok = Instant::now();
    let mut koniec_spojenia = false;

    while zaciatok.elapsed() < Duration::from_secs(30) && !koniec_spojenia {
        let mut prijate = Vec::new();
        match sys::prijmi(&server, &mut buf, &mut prijate) {
            Ok(0) => koniec_spojenia = true,
            Ok(n) => {
                fds.extend(prijate);
                sk.pridaj(&buf[..n]);
            }
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
            Err(e) => panic!("{e}"),
        }
        while let Some(u) = sk.dalsia() {
            let u = u.unwrap();
            let mut c = Citac::novy(&u.data);
            let r = rozhrania.get(&u.objekt).cloned().unwrap_or_default();
            match (r.as_str(), u.cislo) {
                ("wl_display", 1) => {
                    let id = c.u();
                    rozhrania.insert(id, "wl_registry".into());
                    for (meno, (i, v)) in [
                        ("wl_compositor", (1, 4)),
                        ("wl_shm", (2, 1)),
                        ("xdg_wm_base", (3, 2)),
                        ("wl_seat", (4, 7)),
                        ("wl_data_device_manager", (5, 3)),
                    ] {
                        s.posli(Sprava::nova(id, 0).u(i).s(meno).u(v));
                    }
                }
                ("wl_display", 0) => {
                    let id = c.u();
                    s.posli(Sprava::nova(id, 0).u(0));
                    s.posli(Sprava::nova(1, 1).u(id));
                }
                ("wl_registry", 0) => {
                    let (_meno, rozhranie, _v, id) = (c.u(), c.s(), c.u(), c.u());
                    if rozhranie == "wl_seat" {
                        s.posli(Sprava::nova(id, 0).u(3));
                    }
                    rozhrania.insert(id, rozhranie);
                }
                ("wl_compositor", 0) => {
                    let id = c.u();
                    rozhrania.insert(id, "wl_surface".into());
                    if okno_surface == 0 {
                        okno_surface = id;
                    }
                }
                ("wl_shm", 0) => {
                    let id = c.u();
                    zasobniky.insert(id, File::from(fds.pop_front().expect("popisovač zdieľanej pamäte")));
                    rozhrania.insert(id, "wl_shm_pool".into());
                }
                ("wl_shm_pool", 0) => {
                    let (id, posun, w, h, riadok, format) = (c.u(), c.i(), c.i(), c.i(), c.i(), c.u());
                    assert!(format == 1 || format == 0);
                    obrazy.insert(id, (u.objekt, posun as usize, w as usize, h as usize, riadok as usize));
                    rozhrania.insert(id, "wl_buffer".into());
                }
                ("xdg_wm_base", 2) => {
                    xdg_surface = c.u();
                    rozhrania.insert(xdg_surface, "xdg_surface".into());
                }
                ("xdg_surface", 1) => {
                    toplevel = c.u();
                    rozhrania.insert(toplevel, "xdg_toplevel".into());
                }
                ("xdg_surface", 4) => ack = c.u() == 77,
                ("xdg_toplevel", 2) => titulky.push(c.s()),
                ("wl_seat", 0) => {
                    pointer = c.u();
                    rozhrania.insert(pointer, "wl_pointer".into());
                }
                ("wl_seat", 1) => {
                    keyboard = c.u();
                    rozhrania.insert(keyboard, "wl_keyboard".into());
                    let mut t = super::docasny_subor(ROZLOZENIE.len() + 1).unwrap();
                    t.write_all(ROZLOZENIE.as_bytes()).unwrap();
                    let b = Sprava::nova(keyboard, 0).u(1).u(ROZLOZENIE.len() as u32 + 1).bajty();
                    sys::posli(&server, &b, &[t.as_fd()]).unwrap();
                    s.posli(Sprava::nova(keyboard, 5).i(0).i(0));
                    let sr = s.serial();
                    s.posli(Sprava::nova(keyboard, 1).u(sr).u(okno_surface).a(&[]));
                }
                ("wl_surface", 1) if u.objekt == okno_surface => pripojeny = c.u(),
                ("wl_surface", 3) if u.objekt == okno_surface => {
                    snimok = c.u();
                    rozhrania.insert(snimok, "wl_callback".into());
                }
                ("wl_surface", 6) if u.objekt == okno_surface => {
                    if !nakonfigurovane {
                        nakonfigurovane = true;
                        s.posli(Sprava::nova(toplevel, 0).i(800).i(600).a(&4u32.to_ne_bytes()));
                        s.posli(Sprava::nova(xdg_surface, 0).u(77));
                        continue;
                    }
                    assert!(ack, "okno kreslí pred potvrdením configure");
                    let Some(&(pool, posun, w, h, riadok)) = obrazy.get(&pripojeny) else { continue };
                    assert_eq!((w, h), (800, 600));
                    let mut b = vec![0u8; riadok * h];
                    zasobniky[&pool].read_exact_at(&mut b, posun as u64).unwrap();
                    let body: Vec<u32> =
                        b.chunks_exact(4).map(|x| u32::from_le_bytes([x[0], x[1], x[2], x[3]])).collect();
                    snimkov += 1;
                    s.posli(Sprava::nova(pripojeny, 0));
                    if snimok != 0 {
                        s.posli(Sprava::nova(snimok, 0).u(snimkov));
                        snimok = 0;
                    }
                    let obdlznik = cervene(&body[800 * super::LISTA..], 800);
                    match krok {
                        0 if obdlznik.is_some_and(|(w, h)| w > h) => {
                            pred_otocenim = obdlznik;
                            // kláves r (evdev 19) – otočiť vpravo
                            let (a, b) = (s.serial(), s.serial());
                            s.posli(Sprava::nova(keyboard, 3).u(a).u(1).u(19).u(1));
                            s.posli(Sprava::nova(keyboard, 3).u(b).u(2).u(19).u(0));
                            krok = 1;
                        }
                        1 if obdlznik.is_some_and(|(w, h)| h > w) => {
                            // myš na tlačidlo „zavrieť“ vlastnej lišty
                            let sr = s.serial();
                            s.posli(Sprava::nova(pointer, 0).u(sr).u(okno_surface).i(f(400.0)).i(f(300.0)));
                            s.posli(Sprava::nova(pointer, 2).u(10).i(f(790.0)).i(f(10.0)));
                            s.posli(Sprava::nova(pointer, 5));
                            krok = 2;
                        }
                        2 if body[..800 * super::LISTA].contains(&super::ZAVRIET_POD_MYSOU) => {
                            let sr = s.serial();
                            s.posli(Sprava::nova(pointer, 3).u(sr).u(11).u(super::BTN_LAVE).u(1));
                            krok = 3;
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
        if okno.is_finished() {
            break;
        }
    }
    let kod = okno.join().unwrap();
    let _ = std::fs::remove_dir_all(&priecinok);
    assert_eq!(krok, 3, "kroky: obrázok {pred_otocenim:?}, snímkov {snimkov}");
    assert_eq!(kod, 0);
    assert!(
        titulky
            .iter()
            .any(|t| t.contains("cerveny.png") && t.contains("prehliadač a editor fotografií (Pohľad)"))
    );
}
