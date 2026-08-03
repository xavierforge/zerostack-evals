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

/// SHA-256 of arbitrary bytes, as lowercase hex (64 chars). Pure std, no
/// dependency, same house style as `fnv1a_hex` and `civil_date_string`. Used
/// for the machine-comparable half of zerostack's build identity
/// (`Report::zs_bin_sha256`): unlike `fnv1a_hex` (a fast fingerprint for
/// scenario/judge/pack source), this is the field two runs' builds are
/// compared on, so it is a real cryptographic digest, not a 64-bit fold.
pub fn sha256_hex(bytes: &[u8]) -> String {
    // FIPS 180-4. Round constants (first 32 bits of the fractional parts of the
    // cube roots of the first 64 primes).
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];

    // Pad: 0x80, then zeros to 56 mod 64, then the bit length big-endian.
    let bit_len = (bytes.len() as u64).wrapping_mul(8);
    let mut msg = bytes.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in msg.chunks_exact(64) {
        let mut w = [0u32; 64];
        for (i, word) in w.iter_mut().enumerate().take(16) {
            let b = i * 4;
            *word = u32::from_be_bytes([chunk[b], chunk[b + 1], chunk[b + 2], chunk[b + 3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (slot, v) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
            *slot = slot.wrapping_add(v);
        }
    }

    let mut out = String::with_capacity(64);
    for word in h {
        out.push_str(&format!("{word:08x}"));
    }
    out
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

/// Round to 4 decimal places — the wire precision of every rate and dollar
/// figure a report or matrix carries.
pub fn round4(x: f64) -> f64 {
    (x * 10_000.0).round() / 10_000.0
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
mod sha256_tests {
    use super::*;

    /// The two canonical NIST test vectors pin the transcription: if the round
    /// function or padding is off by anything, one of these will not match.
    #[test]
    fn matches_the_canonical_vectors() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    /// A multi-block input (> 55 bytes forces a second padded block) to
    /// exercise the chunk loop, not just the single-block path.
    #[test]
    fn hashes_a_multi_block_input() {
        assert_eq!(
            sha256_hex(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
    }

    #[test]
    fn output_is_64_lowercase_hex_chars() {
        let h = sha256_hex(b"zerostack 1.7.2");
        assert_eq!(h.len(), 64, "{h}");
        assert!(
            h.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "{h}"
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
