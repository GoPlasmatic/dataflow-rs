//! Workflow fingerprints: FNV-1a 64 over the operator names and the data
//! keys a workflow's expressions use.
//!
//! The classification is made by `Checks::expr`. Every single-key object is
//! classified by its key: an operator when the key names an entry of the
//! engine's vocabulary, data when it does not start with the escape, and
//! nothing when it does. The encoding is, with `ops` and `data` sorted and
//! deduplicated:
//!
//! ```text
//! for name in ops:  (name.len() as u32).to_le_bytes() ++ name.as_bytes()
//! 0xFFFF_FFFFu32.to_le_bytes()
//! for key in data:  (key.len() as u32).to_le_bytes() ++ key.as_bytes()
//! ```

use std::collections::BTreeSet;
use std::fmt;
use std::str::FromStr;

const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const PRIME: u64 = 0x0000_0100_0000_01b3;
const PREFIX: &str = "v1:";

/// Feeds `bytes` into an FNV-1a 64 state `h`.
fn fnv1a_step(h: u64, bytes: &[u8]) -> u64 {
    bytes
        .iter()
        .fold(h, |h, b| (h ^ u64::from(*b)).wrapping_mul(PRIME))
}

#[cfg(test)]
fn fnv1a(bytes: &[u8]) -> u64 {
    fnv1a_step(OFFSET, bytes)
}

/// The bytes the hash is taken over.
#[cfg(test)]
fn encode(ops: &BTreeSet<String>, data: &BTreeSet<String>) -> Vec<u8> {
    fn put(out: &mut Vec<u8>, s: &str) {
        let len = u32::try_from(s.len()).expect("a key is shorter than 2^32 bytes");
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(s.as_bytes());
    }
    let mut out = Vec::new();
    ops.iter().for_each(|s| put(&mut out, s));
    out.extend_from_slice(&u32::MAX.to_le_bytes());
    data.iter().for_each(|s| put(&mut out, s));
    out
}

/// A workflow's fingerprint: which operators and data keys its expressions
/// use, and nothing about values, order or ids. It shows as `v1:` and 16 hex
/// digits, and the same workflow under the same vocabulary has the same one
/// in every process and release.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Fingerprint(u64);

impl Fingerprint {
    /// The fingerprint of the two sets.
    pub(crate) fn of(ops: &BTreeSet<String>, data: &BTreeSet<String>) -> Self {
        fn put(h: u64, s: &str) -> u64 {
            let len = u32::try_from(s.len()).expect("a key is shorter than 2^32 bytes");
            let h = fnv1a_step(h, &len.to_le_bytes());
            fnv1a_step(h, s.as_bytes())
        }
        let h = ops.iter().fold(OFFSET, |h, s| put(h, s));
        let h = fnv1a_step(h, &u32::MAX.to_le_bytes());
        let h = data.iter().fold(h, |h, s| put(h, s));
        Self(h)
    }
}

impl fmt::Display for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{PREFIX}{:016x}", self.0)
    }
}

/// A string that is not a [`Fingerprint`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseFingerprintError(String);

impl fmt::Display for ParseFingerprintError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "`{}` is not a fingerprint: expected `v1:` and 16 hex digits",
            self.0
        )
    }
}

impl std::error::Error for ParseFingerprintError {}

impl FromStr for Fingerprint {
    type Err = ParseFingerprintError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let bad = || ParseFingerprintError(s.to_owned());
        let hex = s.strip_prefix(PREFIX).ok_or_else(bad)?;
        if hex.len() != 16 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(bad());
        }
        u64::from_str_radix(hex, 16).map(Self).map_err(|_| bad())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv1a_matches_the_standard_vectors() {
        assert_eq!(fnv1a(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv1a(b"foobar"), 0x8594_4171_f739_67e8);
    }

    #[test]
    fn the_encoding_is_lengths_names_a_separator_and_keys() {
        let set = |s: &str| BTreeSet::from([s.to_owned()]);
        let mut want = vec![3, 0, 0, 0, b'v', b'a', b'r', 255, 255, 255, 255];
        want.extend([3, 0, 0, 0, b'o', b'u', b't']);
        assert_eq!(encode(&set("var"), &set("out")), want);
    }
}
