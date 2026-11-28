//! Sieť ORCHIDEA – vlastné protokoly internetu, napísané podľa textu RFC (nie podľa iných programov).
//!
//! Balík je **čistý**: kóduje a dekóduje správy a vedie stavové automaty, ale sám nič neposiela ani nečaká.
//! Vstup a výstup (zásuvky, rtnetlink, hodiny) robí volajúci – správca systému (KLÍČ) na krabičkách, neskôr
//! prehrávač médií LÝRA. Vďaka tomu sa každý protokol dá overiť skúškami bajt po bajte bez siete a bez
//! oprávnení a nezávislý rozhodca (cudzí server) sa používa len v CI.
//!
//! | Modul | Čo | Normy |
//! |---|---|---|
//! | [`dhcp`] | adresa IPv4, maska, brána, servery DNS a NTP z routera; obnova prenájmu | RFC 2131, RFC 2132 |
//! | [`dns`] | preklad mena na adresu IPv4 a IPv6, reťaz CNAME, vyrovnávacia pamäť podľa TTL | RFC 1035, RFC 3596, RFC 2308 |
//! | [`sntp`] | presný čas zo siete (krabičky nemajú hodiny s batériou) | RFC 4330, RFC 5905 |
//!
//! **Chladný systém:** nič sa neopakuje naprázdno – automat DHCP povie volajúcemu, kedy ho má zobudiť
//! ([`dhcp::Klient::kedy`]), a medzi tým nič nebeží; pri platnom prenájme je to až čas obnovy (T1, bežne
//! polovica prenájmu).

pub mod dhcp;
pub mod dns;
pub mod sntp;
