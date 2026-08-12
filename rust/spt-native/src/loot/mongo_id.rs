//! MongoDB-style ObjectId generation, mirroring `Models/Common/MongoId.cs`.
//!
//! 12 bytes, hex-encoded lowercase: 4-byte big-endian unix seconds, 3-byte machine id drawn once
//! per process, 2-byte big-endian process id, 3-byte big-endian counter seeded at random. Ids from
//! here are handed straight to C# `new MongoId(hex)`, so the layout has to stay byte-for-byte.

use std::fmt::Write;
use std::process;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use rand::Rng;

/// Generates a new id as 24 lowercase hex characters.
pub fn generate() -> String {
    static MACHINE: OnceLock<u32> = OnceLock::new();
    static COUNTER: OnceLock<AtomicU32> = OnceLock::new();

    let machine = *MACHINE.get_or_init(|| rand::rng().random_range(0..=0x00FF_FFFF));
    let counter = COUNTER.get_or_init(|| AtomicU32::new(rand::rng().random_range(0..0x00FF_FFFF)));
    // A clock before the epoch would only cost us the timestamp prefix, so don't panic over it —
    // this runs behind FFI, where unwinding is undefined behaviour.
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs() as u32);
    let increment = counter.fetch_add(1, Ordering::Relaxed) & 0x00FF_FFFF;

    let mut bytes = [0u8; 12];
    bytes[0..4].copy_from_slice(&timestamp.to_be_bytes());
    bytes[4..7].copy_from_slice(&machine.to_be_bytes()[1..]);
    bytes[7..9].copy_from_slice(&(process::id() as u16).to_be_bytes());
    bytes[9..12].copy_from_slice(&increment.to_be_bytes()[1..]);

    let mut id = String::with_capacity(24);
    for byte in bytes {
        let _ = write!(id, "{byte:02x}");
    }

    id
}

/// Reports whether `s` is a 24-character hex string, i.e. whether C# `new MongoId(s)` would accept
/// it. Both cases are hex, matching the C# parser; only [`generate`] output is lowercase.
pub fn is_valid(s: &str) -> bool {
    s.len() == 24 && s.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    #[test]
    fn generates_24_lowercase_hex_characters() {
        for _ in 0..100 {
            let id = generate();
            assert_eq!(id.len(), 24, "{id} is not 24 characters");
            assert!(
                id.bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
                "{id} contains non-lowercase-hex characters"
            );
        }
    }

    #[test]
    fn generates_unique_ids() {
        let ids: HashSet<String> = (0..10_000).map(|_| generate()).collect();
        assert_eq!(ids.len(), 10_000, "generated ids collided");
    }

    #[test]
    fn leading_four_bytes_are_the_current_unix_seconds() {
        let before = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let id = generate();
        let after = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let timestamp = u64::from(u32::from_str_radix(&id[..8], 16).expect("hex timestamp prefix"));
        assert!(
            (before..=after).contains(&timestamp),
            "timestamp {timestamp} from {id} is outside {before}..={after}"
        );
    }

    #[test]
    fn generated_ids_are_unique() {
        let first = generate();
        let second = generate();
        assert_ne!(first, second);
    }

    #[test]
    fn is_valid_accepts_generated_ids() {
        assert!(is_valid(&generate()));
    }

    #[test]
    fn is_valid_matches_the_csharp_parser() {
        // `new MongoId(hex)` accepts 24 characters of either case and rejects everything else.
        assert!(is_valid("5449016a4bdc2d6f028b456f"));
        assert!(is_valid("5449016A4BDC2D6F028B456F"));
        assert!(!is_valid(""));
        assert!(!is_valid("5449016a4bdc2d6f028b456"));
        assert!(!is_valid("5449016a4bdc2d6f028b456ff"));
        assert!(!is_valid("5449016a4bdc2d6f028b456g"));
        assert!(!is_valid("5449016a4bdc2d6f028b456\u{00e9}"));
    }
}
