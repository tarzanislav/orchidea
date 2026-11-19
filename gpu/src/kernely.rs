//! Štandardné kernely ORCHIDEE.

use crate::ir::*;

/// c[i] = a[i] + b[i] pre i < n.
pub fn sucet() -> Kernel {
    let mut k = Kernel::novy("súčet", [64, 1, 1]);
    let a = k.vazba("a", Typ::F32, Pristup::Citanie);
    let b = k.vazba("b", Typ::F32, Pristup::Citanie);
    let c = k.vazba("c", Typ::F32, Pristup::Zapis);
    let n = k.konstanta("n", Typ::U32);
    k.telo = vec![ak(id(0).mensie(self::k(n)), vec![zapis(c, id(0), citaj(a, id(0)) + citaj(b, id(0)))])];
    k
}

/// y[i] = x[i] · faktor + posun pre i < n.
pub fn skaluj() -> Kernel {
    let mut k = Kernel::novy("škáluj", [64, 1, 1]);
    let x = k.vazba("x", Typ::F32, Pristup::Citanie);
    let y = k.vazba("y", Typ::F32, Pristup::Zapis);
    let n = k.konstanta("n", Typ::U32);
    let faktor = k.konstanta("faktor", Typ::F32);
    let posun = k.konstanta("posun", Typ::F32);
    k.telo = vec![ak(
        id(0).mensie(self::k(n)),
        vec![zapis(y, id(0), citaj(x, id(0)) * self::k(faktor) + self::k(posun))],
    )];
    k
}

/// C (m×n) = A (m×k) · B (k×n) po riadkoch. Jedno vlákno počíta jeden prvok C v pevnom poradí sčítania
/// p = 0, 1, …, k−1 bez FMA – výsledok je bitovo rovnaký ako presný režim Neurónu na procesore.
pub fn nasobenie_presne() -> Kernel {
    let mut kr = Kernel::novy("násobenie matíc (presné)", [16, 16, 1]);
    let a = kr.vazba("A", Typ::F32, Pristup::Citanie);
    let b = kr.vazba("B", Typ::F32, Pristup::Citanie);
    let c = kr.vazba("C", Typ::F32, Pristup::Zapis);
    let m = kr.konstanta("m", Typ::U32);
    let n = kr.konstanta("n", Typ::U32);
    let kk = kr.konstanta("k", Typ::U32);
    let (r, s, q, suma) =
        (kr.premenna(Typ::U32), kr.premenna(Typ::U32), kr.premenna(Typ::U32), kr.premenna(Typ::F32));
    kr.telo = vec![
        nastav(s, id(0)),
        nastav(r, id(1)),
        ak(
            p(r).mensie(k(m)).a(p(s).mensie(k(n))),
            vec![
                nastav(suma, f(0.0)),
                pre(
                    q,
                    u(0),
                    k(kk),
                    vec![nastav(
                        suma,
                        p(suma) + citaj(a, p(r) * k(kk) + p(q)) * citaj(b, p(q) * k(n) + p(s)),
                    )],
                ),
                zapis(c, p(r) * k(n) + p(s), p(suma)),
            ],
        ),
    ];
    kr
}

/// Kernel, ktorý použije každú konštrukciu jazyka – na overenie prekladačov a zariadení.
pub fn skuska_jazyka() -> Kernel {
    let mut kr = Kernel::novy("skúška jazyka", [32, 1, 1]);
    let vf = kr.vazba("vf", Typ::F32, Pristup::Citanie);
    let vu = kr.vazba("vu", Typ::U32, Pristup::Citanie);
    let of = kr.vazba("of", Typ::F32, Pristup::Zapis);
    let ou = kr.vazba("ou", Typ::U32, Pristup::Zapis);
    let oi = kr.vazba("oi", Typ::I32, Pristup::CitanieZapis);
    let n = kr.konstanta("n", Typ::U32);
    let s = kr.konstanta("s", Typ::F32);
    let z = kr.konstanta("z", Typ::I32);
    let i0 = kr.premenna(Typ::U32);
    let x = kr.premenna(Typ::F32);
    let a = kr.premenna(Typ::U32);
    let j = kr.premenna(Typ::U32);
    let sf = kr.premenna(Typ::F32);
    let su = kr.premenna(Typ::U32);
    let iv = kr.premenna(Typ::I32);
    let zapor = kr.premenna(Typ::Bool);
    kr.telo = vec![
        nastav(i0, id(0)),
        ak(
            p(i0).mensie(k(n)),
            vec![
                nastav(x, citaj(vf, p(i0))),
                nastav(a, citaj(vu, p(i0))),
                nastav(sf, f(0.0)),
                nastav(su, u(0)),
                pre(
                    j,
                    u(0),
                    p(a) % u(7),
                    vec![nastav(sf, p(sf) + p(x) * k(s)), nastav(su, p(su) + p(j) * u(3))],
                ),
                nastav(zapor, p(x).mensie(f(0.0)).alebo(p(x).rovne(f(-0.25)))),
                nastav(iv, (p(x) * f(50.0)).na(Typ::I32)),
                ak_inak(
                    p(zapor).a(p(a).rozne(u(0)).un(Op1::Nie).un(Op1::Nie)),
                    vec![nastav(iv, -p(iv) + i(1))],
                    vec![nastav(iv, p(iv) - k(z))],
                ),
                zapis(
                    oi,
                    p(i0),
                    p(iv).un(Op1::Abs) + p(iv) / i(3) + p(iv) % i(4) + citaj(oi, p(i0))
                        - i(i32::MIN + 5).max(i(-7)),
                ),
                zapis(
                    ou,
                    p(i0),
                    p(su) + (p(x).un(Op1::Abs) * f(100.0)).na(Typ::U32) + dlzka(vf) + p(a) / u(3)
                        - (p(a) % u(5)).min(u(2))
                        + p(iv).na(Typ::U32).max(u(1)) * vyber(p(zapor), u(2), u(1)),
                ),
                zapis(
                    of,
                    p(i0),
                    p(sf) + (p(x) * f(10.0)).un(Op1::Dole) / f(10.0) + (-(p(x) * p(x))).un(Op1::Exp)
                        - (p(x).un(Op1::Abs) + f(1.0)).un(Op1::Ln)
                        + p(x).min(k(s))
                        - p(x).max(-k(s))
                        + p(x).un(Op1::Abs).un(Op1::Odmocnina)
                        + vyber(p(x).vacsie_rovne(f(0.5)).a(p(x).mensie_rovne(f(0.75))), f(1e-3), f(-2.5e-4))
                        + p(a).na(Typ::F32) * f(0.125)
                        + (p(iv).na(Typ::F32) % f(3.0))
                        + vyber(p(x).vacsie(f(0.0)).a(p(x).rozne(f(0.5))), f(0.0), f(1.0)),
                ),
            ],
        ),
    ];
    kr
}

/// Všetky štandardné kernely.
pub fn vsetky() -> Vec<Kernel> {
    vec![sucet(), skaluj(), nasobenie_presne(), skuska_jazyka()]
}
