//! Small shared helpers (pure std).

/// Howard Hinnant's civil-from-days: days since epoch -> "YYYY-MM-DD".
pub fn civil_date_string(z: i64) -> String {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

/// Compact UTC timestamp "YYYYMMDD-HHMMSS-mmm-pid", for run-folder names.
/// Millisecond + pid suffix so two runs auto-tagged back-to-back (e.g. a
/// script looping `zseval run` without an explicit `--tag`) never collide on
/// the same `results/<tag>/` directory and silently overwrite each other.
pub fn compact_timestamp() -> String {
    let dur = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = dur.as_secs();
    let ymd: String = civil_date_string((secs / 86_400) as i64)
        .chars()
        .filter(|c| *c != '-')
        .collect();
    let rem = secs % 86_400;
    format!(
        "{ymd}-{:02}{:02}{:02}-{:03}-{}",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60,
        dur.subsec_millis(),
        std::process::id()
    )
}

/// FNV-1a 64-bit hash of arbitrary bytes, as lowercase hex (16 chars). Used
/// to fingerprint a scenario's raw TOML source so `compare` can tell whether
/// a scenario definition changed between a baseline and a candidate run —
/// independent of `domains::memory::project_slug`'s own FNV-1a use, which
/// must stay byte-for-byte pinned to zerostack's algorithm and must never be
/// refactored to share code with an unrelated hash consumer.
pub fn fnv1a_hex(bytes: &[u8]) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for &byte in bytes {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

/// Tail of a text file, for self-explanatory error messages.
pub fn tail_of(path: &std::path::Path, lines: usize) -> String {
    match std::fs::read_to_string(path) {
        Ok(s) if !s.trim().is_empty() => {
            let all: Vec<&str> = s.lines().collect();
            let start = all.len().saturating_sub(lines);
            all[start..].join("\n")
        }
        _ => String::from("(empty)"),
    }
}

#[cfg(test)]
mod compact_timestamp_tests {
    use super::*;

    #[test]
    fn format_is_ymd_hms_millis_pid() {
        let ts = compact_timestamp();
        let parts: Vec<&str> = ts.split('-').collect();
        assert_eq!(parts.len(), 4, "{ts}");
        assert_eq!(parts[0].len(), 8, "{ts}"); // YYYYMMDD
        assert_eq!(parts[1].len(), 6, "{ts}"); // HHMMSS
        assert_eq!(parts[2].len(), 3, "{ts}"); // millis
        assert!(parts[0].chars().all(|c| c.is_ascii_digit()), "{ts}");
        assert!(parts[1].chars().all(|c| c.is_ascii_digit()), "{ts}");
        assert!(parts[2].chars().all(|c| c.is_ascii_digit()), "{ts}");
        assert_eq!(
            parts[3],
            std::process::id().to_string(),
            "pid suffix should be this process's own pid"
        );
    }
}

#[cfg(test)]
mod fnv1a_tests {
    use super::*;

    #[test]
    fn same_bytes_hash_the_same() {
        assert_eq!(fnv1a_hex(b"hello"), fnv1a_hex(b"hello"));
    }

    #[test]
    fn different_bytes_hash_differently() {
        assert_ne!(fnv1a_hex(b"hello"), fnv1a_hex(b"hellp"));
    }

    #[test]
    fn output_is_16_lowercase_hex_chars() {
        let h = fnv1a_hex(b"scenario.toml contents");
        assert_eq!(h.len(), 16, "{h}");
        assert!(
            h.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "{h}"
        );
    }
}
