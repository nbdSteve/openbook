//! Identifiers: ULIDs and invite codes.

use std::sync::Mutex;

use ulid::{Generator, Ulid};

/// The 31 characters allowed in invite codes (no `0`, `O`, `1`, `I`, `L`).
pub const INVITE_ALPHABET: &[u8; 31] = b"ABCDEFGHJKMNPQRSTUVWXYZ23456789";
/// Length of an invite code.
pub const INVITE_CODE_LEN: usize = 8;

static GENERATOR: Mutex<Option<Generator>> = Mutex::new(None);

/// Returns a new ULID string. IDs from one process are strictly increasing, even inside one millisecond.
pub fn new_id() -> String {
    let mut guard = GENERATOR.lock().unwrap_or_else(|e| e.into_inner());
    let generator = guard.get_or_insert_with(Generator::new);
    generator
        .generate()
        .unwrap_or_else(|_| Ulid::generate())
        .to_string()
}

/// A ULID that sorts before every ULID created at or after `millis` (Unix milliseconds).
pub fn ulid_floor(millis: u64) -> String {
    Ulid::from_parts(millis, 0).to_string()
}

/// True when `s` is a canonical ULID string (26 characters, upper case Crockford base32).
pub fn is_ulid(s: &str) -> bool {
    s.len() == 26
        && s.bytes()
            .all(|b| b.is_ascii_digit() || b.is_ascii_uppercase())
        && Ulid::from_string(s).is_ok()
}

/// Returns a random invite code.
pub fn new_invite_code() -> String {
    // A ULID carries 80 random bits (the low 10 bytes). Reject bytes that would bias the
    // modulo, because 31 does not divide 256.
    let mut out = String::with_capacity(INVITE_CODE_LEN);
    while out.len() < INVITE_CODE_LEN {
        let bytes = Ulid::generate().random().to_be_bytes();
        for byte in &bytes[6..] {
            if *byte < 248 && out.len() < INVITE_CODE_LEN {
                out.push(INVITE_ALPHABET[usize::from(*byte) % INVITE_ALPHABET.len()] as char);
            }
        }
    }
    out
}

/// Normalizes user input to the stored form: upper case, spaces and hyphens removed.
/// Returns `None` when the result is not a valid invite code.
pub fn normalize_invite_code(input: &str) -> Option<String> {
    let code: String = input
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .flat_map(char::to_uppercase)
        .collect();
    let valid = code.len() == INVITE_CODE_LEN && code.bytes().all(|b| INVITE_ALPHABET.contains(&b));
    valid.then_some(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_increasing_ulids() {
        let a = new_id();
        let b = new_id();
        assert!(is_ulid(&a));
        assert!(a < b);
    }

    #[test]
    fn rejects_non_ulids() {
        assert!(!is_ulid(""));
        assert!(!is_ulid("short"));
        assert!(!is_ulid(&new_id().to_lowercase()));
        assert!(!is_ulid("01ARZ3NDEKTSV4RRFFQ69G5FA!"));
        assert!(!is_ulid("../../../../../etc/passwd--"));
    }

    #[test]
    fn invite_codes_use_the_alphabet() {
        for _ in 0..200 {
            let code = new_invite_code();
            assert_eq!(code.len(), INVITE_CODE_LEN);
            assert!(code.bytes().all(|b| INVITE_ALPHABET.contains(&b)));
            assert_eq!(normalize_invite_code(&code), Some(code));
        }
    }

    #[test]
    fn invite_codes_normalize() {
        assert_eq!(normalize_invite_code("abcd-2345"), Some("ABCD2345".into()));
        assert_eq!(
            normalize_invite_code("  ab cd 23 45 "),
            Some("ABCD2345".into())
        );
        assert_eq!(normalize_invite_code("ABCD234"), None);
        assert_eq!(normalize_invite_code("ABCD23456"), None);
        // 0, O, 1, I, L are not in the alphabet.
        for bad in ["ABCD0234", "ABCDO234", "ABCD1234", "ABCDI234", "ABCDL234"] {
            assert_eq!(normalize_invite_code(bad), None, "{bad}");
        }
    }
}
