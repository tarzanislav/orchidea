//! Meno – lacno klonovateľný, overený názov (osi, operácie, parametra, vlastnosti…).

use crate::chyba::{Chyba, Vysledok};
use std::fmt;
use std::sync::Arc;

/// Najväčšia dĺžka mena v bajtoch UTF-8.
pub const NAJDLHSIE_MENO: usize = 256;

/// Overené meno. Nesmie byť prázdne, dlhšie ako [`NAJDLHSIE_MENO`] bajtov ani obsahovať riadiace znaky.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Meno(Arc<str>);

impl Meno {
    /// Vytvorí meno a overí ho.
    pub fn nove(text: &str) -> Vysledok<Meno> {
        if text.is_empty() {
            return Err(Chyba::NeplatneMeno("prázdne meno".into()));
        }
        if text.len() > NAJDLHSIE_MENO {
            return Err(Chyba::NeplatneMeno(format!("dlhšie ako {NAJDLHSIE_MENO} bajtov")));
        }
        if text.chars().any(char::is_control) {
            return Err(Chyba::NeplatneMeno(format!("obsahuje riadiaci znak: {text:?}")));
        }
        Ok(Meno(Arc::from(text)))
    }

    /// Meno ako text.
    pub fn ako_text(&self) -> &str {
        &self.0
    }
}

/// Pohodlné vytvorenie mena zo statického textu, ktorý je známy pri písaní kódu.
///
/// # Panics
/// Ak text nie je platné meno – to je chyba programátora, nie vstupných dát.
pub fn m(text: &str) -> Meno {
    match Meno::nove(text) {
        Ok(meno) => meno,
        Err(e) => panic!("neplatné meno v zdrojovom kóde: {e}"),
    }
}

impl fmt::Debug for Meno {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", &*self.0)
    }
}

impl fmt::Display for Meno {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::borrow::Borrow<str> for Meno {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for Meno {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod testy {
    use super::*;

    #[test]
    fn overenie() {
        assert!(Meno::nove("x").is_ok());
        assert!(Meno::nove("λ").is_ok());
        assert!(Meno::nove("").is_err());
        assert!(Meno::nove("a\nb").is_err());
        assert!(Meno::nove(&"a".repeat(NAJDLHSIE_MENO + 1)).is_err());
        assert_eq!(m("kanál").ako_text(), "kanál");
    }
}
