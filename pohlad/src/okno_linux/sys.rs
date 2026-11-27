//! Jediné miesto s `unsafe` v okne pre Linux: posielanie a prijímanie popisovačov súborov cez
//! zásuvku Unix (`sendmsg`/`recvmsg` so `SCM_RIGHTS` z knižnice C, ktorú používa aj štandardná knižnica Rustu).
//!
//! Wayland posiela takto zdieľanú pamäť obrazu a rozloženie klávesnice. Štandardná knižnica to zatiaľ
//! nemá v stabilnej podobe (`unix_socket_ancillary_data`).

#![allow(unsafe_code)]

use std::io;
use std::os::fd::{AsRawFd, BorrowedFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::net::UnixStream;

#[repr(C)]
struct IoVec {
    zaciatok: *mut u8,
    dlzka: usize,
}

#[repr(C)]
struct MsgHdr {
    meno: *mut u8,
    dlzka_mena: u32,
    iov: *mut IoVec,
    pocet_iov: usize,
    riadenie: *mut u8,
    dlzka_riadenia: usize,
    priznaky: i32,
}

unsafe extern "C" {
    fn sendmsg(fd: i32, msg: *const MsgHdr, priznaky: i32) -> isize;
    fn recvmsg(fd: i32, msg: *mut MsgHdr, priznaky: i32) -> isize;
}

const SOL_SOCKET: i32 = 1;
const SCM_RIGHTS: i32 = 1;
const MSG_NOSIGNAL: i32 = 0x4000;
const MSG_CMSG_CLOEXEC: i32 = 0x4000_0000;
/// Najviac popisovačov v jednej správe (Wayland: 28).
pub const NAJVIAC_FD: usize = 28;

const SLOVO: usize = std::mem::size_of::<usize>();
/// Hlavička riadiacej správy (cmsghdr): dĺžka (size_t), úroveň, typ.
const HLAVICKA: usize = (SLOVO + 8).div_ceil(SLOVO) * SLOVO;

fn zarovnaj(n: usize) -> usize {
    n.div_ceil(SLOVO) * SLOVO
}

/// Pošle bajty a popisovače súborov (všetky naraz).
pub fn posli(z: &UnixStream, data: &[u8], fds: &[BorrowedFd<'_>]) -> io::Result<()> {
    let mut poslane = 0;
    let mut fds = fds;
    while poslane < data.len() {
        let mut riadenie = [0usize; (HLAVICKA + NAJVIAC_FD * 4).div_ceil(SLOVO) + 1];
        let n_fd = fds.len().min(NAJVIAC_FD);
        let dlzka = HLAVICKA + n_fd * 4;
        if n_fd > 0 {
            let mut b = Vec::with_capacity(zarovnaj(dlzka));
            b.extend_from_slice(&dlzka.to_ne_bytes());
            b.extend_from_slice(&SOL_SOCKET.to_ne_bytes());
            b.extend_from_slice(&SCM_RIGHTS.to_ne_bytes());
            b.resize(HLAVICKA, 0);
            for f in &fds[..n_fd] {
                b.extend_from_slice(&f.as_raw_fd().to_ne_bytes());
            }
            b.resize(zarovnaj(dlzka), 0);
            for (i, c) in b.chunks(SLOVO).enumerate() {
                let mut w = [0u8; SLOVO];
                w[..c.len()].copy_from_slice(c);
                riadenie[i] = usize::from_ne_bytes(w);
            }
        }
        let mut iov = IoVec { zaciatok: data[poslane..].as_ptr() as *mut u8, dlzka: data.len() - poslane };
        let msg = MsgHdr {
            meno: std::ptr::null_mut(),
            dlzka_mena: 0,
            iov: &mut iov,
            pocet_iov: 1,
            riadenie: if n_fd > 0 { riadenie.as_mut_ptr() as *mut u8 } else { std::ptr::null_mut() },
            dlzka_riadenia: if n_fd > 0 { zarovnaj(dlzka) } else { 0 },
            priznaky: 0,
        };
        // SAFETY: msg ukazuje na platné polia na zásobníku po celý čas volania
        let n = unsafe { sendmsg(z.as_raw_fd(), &msg, MSG_NOSIGNAL) };
        if n < 0 {
            let e = io::Error::last_os_error();
            if e.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(e);
        }
        poslane += n as usize;
        fds = &fds[n_fd..];
    }
    Ok(())
}

/// Prijme bajty do `buf`; prijaté popisovače pridá do `fds`. Vráti počet bajtov (0 = koniec spojenia).
pub fn prijmi(z: &UnixStream, buf: &mut [u8], fds: &mut Vec<OwnedFd>) -> io::Result<usize> {
    loop {
        let mut riadenie = [0usize; (HLAVICKA + NAJVIAC_FD * 4).div_ceil(SLOVO) * 2 + 2];
        let mut iov = IoVec { zaciatok: buf.as_mut_ptr(), dlzka: buf.len() };
        let mut msg = MsgHdr {
            meno: std::ptr::null_mut(),
            dlzka_mena: 0,
            iov: &mut iov,
            pocet_iov: 1,
            riadenie: riadenie.as_mut_ptr() as *mut u8,
            dlzka_riadenia: riadenie.len() * SLOVO,
            priznaky: 0,
        };
        // SAFETY: buf a riadenie žijú počas volania; dĺžky zodpovedajú
        let n = unsafe { recvmsg(z.as_raw_fd(), &mut msg, MSG_CMSG_CLOEXEC) };
        if n < 0 {
            let e = io::Error::last_os_error();
            if e.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(e);
        }
        let bajty: Vec<u8> = riadenie.iter().flat_map(|w| w.to_ne_bytes()).collect();
        let koniec = msg.dlzka_riadenia.min(bajty.len());
        let mut i = 0;
        while i + HLAVICKA <= koniec {
            let mut w = [0u8; SLOVO];
            w.copy_from_slice(&bajty[i..i + SLOVO]);
            let dlzka = usize::from_ne_bytes(w);
            let uroven = i32::from_ne_bytes(bajty[i + SLOVO..i + SLOVO + 4].try_into().unwrap_or_default());
            let typ = i32::from_ne_bytes(bajty[i + SLOVO + 4..i + SLOVO + 8].try_into().unwrap_or_default());
            if dlzka < HLAVICKA || i + dlzka > koniec {
                break;
            }
            if uroven == SOL_SOCKET && typ == SCM_RIGHTS {
                for c in bajty[i + HLAVICKA..i + dlzka].chunks_exact(4) {
                    let fd = RawFd::from_ne_bytes([c[0], c[1], c[2], c[3]]);
                    // SAFETY: jadro systému práve odovzdalo tento popisovač tomuto procesu
                    fds.push(unsafe { OwnedFd::from_raw_fd(fd) });
                }
            }
            i += zarovnaj(dlzka);
        }
        return Ok(n as usize);
    }
}

#[cfg(test)]
mod skusky {
    use super::*;
    use std::io::{Read, Write};
    use std::os::fd::AsFd;

    #[test]
    fn popisovace_prejdu_zasuvkou() {
        let (a, b) = UnixStream::pair().unwrap();
        let (mut r, mut w) = std::io::pipe().unwrap();
        posli(&a, b"ahoj", &[w.as_fd()]).unwrap();
        let mut buf = [0u8; 64];
        let mut fds = Vec::new();
        let n = prijmi(&b, &mut buf, &mut fds).unwrap();
        assert_eq!(&buf[..n], b"ahoj");
        assert_eq!(fds.len(), 1);
        // prijatý popisovač je ten istý kanál
        let mut f = std::fs::File::from(fds.pop().unwrap());
        f.write_all(b"cez kanal").unwrap();
        drop(f);
        w.flush().unwrap();
        drop(w);
        let mut s = String::new();
        r.read_to_string(&mut s).unwrap();
        assert_eq!(s, "cez kanal");
        // bez popisovačov
        posli(&a, b"x", &[]).unwrap();
        assert_eq!(prijmi(&b, &mut buf, &mut fds).unwrap(), 1);
        assert!(fds.is_empty());
        drop(a);
        assert_eq!(prijmi(&b, &mut buf, &mut fds).unwrap(), 0);
    }
}
