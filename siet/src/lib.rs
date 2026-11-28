//! Sieť ORCHIDEA – vlastné protokoly internetu, napísané podľa textu RFC (nie podľa iných programov).
//!
//! Protokoly sú **čisté**: kódujú a dekódujú správy a vedú stavové automaty, samy nič neposielajú ani nečakajú.
//! Jediná výnimka je pohodlná funkcia [`http::stiahni`] so štandardnými zásuvkami (TCP, UDP) – pre prehrávač
//! médií LÝRA na krabičke aj na počítači.
//! Vstup a výstup (zásuvky, rtnetlink, hodiny) robí volajúci – správca systému (KLÍČ) na krabičkách, neskôr
//! prehrávač médií LÝRA. Vďaka tomu sa každý protokol dá overiť skúškami bajt po bajte bez siete a bez
//! oprávnení a nezávislý rozhodca (cudzí server) sa používa len v CI.
//!
//! | Modul | Čo | Normy |
//! |---|---|---|
//! | [`dhcp`] | adresa IPv4, maska, brána, servery DNS a NTP z routera; obnova prenájmu | RFC 2131, RFC 2132 |
//! | [`dns`] | preklad mena na adresu IPv4 a IPv6, reťaz CNAME, vyrovnávacia pamäť podľa TTL | RFC 1035, RFC 3596, RFC 2308 |
//! | [`sntp`] | presný čas zo siete (krabičky nemajú hodiny s batériou) | RFC 4330, RFC 5905 |
//! | [`tls`] | šifrované spojenie s webom – klient TLS 1.3 (HTTPS), A12 | RFC 8446, RFC 8448 |
//! | [`x509`] | overenie servera: certifikáty X.509, reťaz k autorite, meno | RFC 5280, RFC 9525, X.690 |
//! | [`korene`] | dôveryhodné koreňové autority (zoznam Mozilla CA ako dáta) | – |
//! | [`http`] | sťahovanie prúdom (zoznamy staníc, súbory, živé prúdy) – rozhranie R7; HTTPS až s A12 | RFC 9112, RFC 9110, RFC 3986 |
//!
//! **Chladný systém:** nič sa neopakuje naprázdno – automat DHCP povie volajúcemu, kedy ho má zobudiť
//! ([`dhcp::Klient::kedy`]), a medzi tým nič nebeží; pri platnom prenájme je to až čas obnovy (T1, bežne
//! polovica prenájmu).

pub mod dhcp;
pub mod dns;
pub mod http;
pub mod korene;
pub mod sntp;
pub mod tls;
pub mod x509;
