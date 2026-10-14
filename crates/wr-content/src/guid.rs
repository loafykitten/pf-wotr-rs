//! Blueprint and type GUIDs.
//!
//! [`Guid`] holds the 32 hex digits of the text form (`Guid.ToString("N")`) as one
//! big-endian number, so it sorts and prints the way the files spell it. The pack format
//! of the original game (`.bbp`) stores .NET's `Guid.ToByteArray()` order instead, which
//! [`Guid::from_dotnet_bytes`] converts.

use std::fmt;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Guid(pub u128);

impl Guid {
    pub const EMPTY: Self = Self(0);

    /// `Guid.TryParse` for the forms blueprints use: `N` (32 digits), `D` (dashed), and
    /// `B`/`P` (dashed in braces or parentheses). Case-insensitive.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        let inner = match (s.as_bytes().first(), s.as_bytes().last()) {
            (Some(b'{'), Some(b'}')) | (Some(b'('), Some(b')')) => &s[1..s.len() - 1],
            _ => s,
        };
        let hex: String = match inner.len() {
            32 => inner.to_owned(),
            36 => {
                let b = inner.as_bytes();
                if b[8] != b'-' || b[13] != b'-' || b[18] != b'-' || b[23] != b'-' {
                    return None;
                }
                inner.chars().filter(|&c| c != '-').collect()
            }
            _ => return None,
        };
        if hex.len() != 32 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        u128::from_str_radix(&hex, 16).ok().map(Self)
    }

    #[must_use]
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// From .NET's `Guid.ToByteArray()` layout: the first three groups little-endian.
    #[must_use]
    pub fn from_dotnet_bytes(b: [u8; 16]) -> Self {
        let mut o = [0u8; 16];
        o[0..4].copy_from_slice(&[b[3], b[2], b[1], b[0]]);
        o[4..6].copy_from_slice(&[b[5], b[4]]);
        o[6..8].copy_from_slice(&[b[7], b[6]]);
        o[8..16].copy_from_slice(&b[8..16]);
        Self(u128::from_be_bytes(o))
    }

    #[must_use]
    pub fn to_bytes(self) -> [u8; 16] {
        self.0.to_be_bytes()
    }

    #[must_use]
    pub fn from_bytes(b: [u8; 16]) -> Self {
        Self(u128::from_be_bytes(b))
    }
}

impl fmt::Display for Guid {
    /// `BlueprintGuid.ToString()`: 32 lowercase digits.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:032x}", self.0)
    }
}

impl fmt::Debug for Guid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Guid({self})")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forms() {
        let n = Guid::parse("6DBF169364e94e51b869ac4ed95d9487").unwrap();
        assert_eq!(n.to_string(), "6dbf169364e94e51b869ac4ed95d9487");
        assert_eq!(Guid::parse("6dbf1693-64e9-4e51-b869-ac4ed95d9487"), Some(n));
        assert_eq!(Guid::parse("{6dbf1693-64e9-4e51-b869-ac4ed95d9487}"), Some(n));
        assert_eq!(Guid::parse("6dbf1693"), None);
        assert_eq!(Guid::parse("zzbf169364e94e51b869ac4ed95d9487"), None);
    }

    #[test]
    fn dotnet_byte_order() {
        // Guid("00112233-4455-6677-8899-aabbccddeeff").ToByteArray()
        let b = [
            0x33, 0x22, 0x11, 0x00, 0x55, 0x44, 0x77, 0x66, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd,
            0xee, 0xff,
        ];
        assert_eq!(Guid::from_dotnet_bytes(b).to_string(), "00112233445566778899aabbccddeeff");
    }
}
