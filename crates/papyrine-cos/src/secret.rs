use std::fmt;

use zeroize::Zeroizing;

/// Password or key bytes: zeroed on drop, never printed.
///
/// PDF passwords are byte strings, not necessarily UTF-8. A password containing a NUL byte is
/// truncated at the NUL by the underlying qpdf API.
#[derive(Clone, Default)]
pub struct Secret(Zeroizing<Vec<u8>>);

impl Secret {
    pub fn new(bytes: impl Into<Vec<u8>>) -> Self {
        Secret(Zeroizing::new(bytes.into()))
    }

    pub fn expose(&self) -> &[u8] {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }
}

impl From<&str> for Secret {
    fn from(s: &str) -> Self {
        Secret::new(s.as_bytes())
    }
}

impl From<String> for Secret {
    fn from(s: String) -> Self {
        Secret::new(s.into_bytes())
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(***)")
    }
}
