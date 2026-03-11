//! Transcoder Engine: Binary ↔ DNA base conversion
//!
//! 2-bit encoding: 00=A, 01=C, 10=G, 11=T
//! Rotation cipher for GC balance and homopolymer avoidance.
//!
//! Performance: Zero-allocation rotation key search, byte-level DNA ops.
//! Correctness: Homopolymer threshold aligned to DNA Fountain spec (≥3).

use serde::Serialize;

/// Maximum allowed consecutive identical bases.
/// Runs up to this length are OK; runs strictly longer are violations.
/// Twist Bioscience spec: max 3 consecutive identical bases allowed.
pub const MAX_HOMOPOLYMER_RUN: usize = 3;

/// DNA base lookup table (ASCII bytes for zero-copy performance)
const BASE_TABLE: [u8; 4] = [b'A', b'C', b'G', b'T'];

/// Reverse lookup: ASCII byte → 2-bit value (0-3), 0xFF for invalid
const REVERSE_TABLE: [u8; 128] = {
    let mut table = [0xFFu8; 128];
    table[b'A' as usize] = 0;
    table[b'a' as usize] = 0;
    table[b'C' as usize] = 1;
    table[b'c' as usize] = 1;
    table[b'G' as usize] = 2;
    table[b'g' as usize] = 2;
    table[b'T' as usize] = 3;
    table[b't' as usize] = 3;
    table
};

#[derive(Debug, Clone, Serialize)]
pub struct TranscodeResult {
    pub sequence: String,
    pub rotation_key: u8,
    pub original_length: usize,
    pub gc_content: f64,
    pub homopolymer_safe: bool,
    pub length: usize,
}

pub struct Transcoder;

impl Default for Transcoder {
    fn default() -> Self { Transcoder }
}

impl Transcoder {
    pub fn new() -> Self {
        Transcoder
    }

    /// Encode binary data to DNA sequence
    pub fn encode(&self, data: &[u8]) -> TranscodeResult {
        let best_key = self.find_best_rotation(data);
        let rotated = apply_rotation(data, best_key);
        let sequence = bytes_to_dna(&rotated);
        let gc = calculate_gc(&sequence);
        let hp_safe = check_homopolymer(&sequence);

        TranscodeResult {
            length: sequence.len(),
            sequence,
            rotation_key: best_key,
            original_length: data.len(),
            gc_content: gc,
            homopolymer_safe: hp_safe,
        }
    }

    /// Decode DNA sequence back to binary data
    pub fn decode(
        &self,
        sequence: &str,
        rotation_key: u8,
        original_length: usize,
    ) -> Vec<u8> {
        let raw_bytes = dna_to_bytes(sequence);
        let unrotated = reverse_rotation(&raw_bytes, rotation_key);
        let keep = original_length.min(unrotated.len());
        unrotated[..keep].to_vec()
    }

    /// Find the rotation key that gives best GC balance (closest to 50%)
    /// PERF: Zero allocations — counts GC bits directly from rotated bytes
    fn find_best_rotation(&self, data: &[u8]) -> u8 {
        let sample = &data[..data.len().min(512)];
        let total_bases = sample.len() * 4;
        if total_bases == 0 {
            return 0;
        }

        let mut best_key = 0u8;
        let mut best_diff = f64::MAX;

        for key in 0..=255u8 {
            // Count GC directly without allocating DNA string
            // C=01, G=10 in 2-bit encoding
            let gc_count: usize = sample.iter()
                .map(|&b| {
                    let r = b.wrapping_add(key);
                    let b0 = (r >> 6) & 0x03;
                    let b1 = (r >> 4) & 0x03;
                    let b2 = (r >> 2) & 0x03;
                    let b3 = r & 0x03;
                    // C=1, G=2 → count crumbs that are 1 or 2
                    ((b0 == 1 || b0 == 2) as usize)
                        + ((b1 == 1 || b1 == 2) as usize)
                        + ((b2 == 1 || b2 == 2) as usize)
                        + ((b3 == 1 || b3 == 2) as usize)
                })
                .sum();

            let gc = gc_count as f64 / total_bases as f64;
            let diff = (gc - 0.5).abs();
            if diff < best_diff {
                best_diff = diff;
                best_key = key;
            }
            if diff < 0.005 {
                break; // Excellent GC balance
            }
        }

        best_key
    }
}

/// Convert bytes to DNA bases (2 bits per base, 4 bases per byte)
/// PERF: Direct byte-level ops, no char push overhead
pub fn bytes_to_dna(data: &[u8]) -> String {
    let mut buf = vec![0u8; data.len() * 4];
    for (i, &byte) in data.iter().enumerate() {
        let base = i * 4;
        buf[base]     = BASE_TABLE[((byte >> 6) & 0x03) as usize];
        buf[base + 1] = BASE_TABLE[((byte >> 4) & 0x03) as usize];
        buf[base + 2] = BASE_TABLE[((byte >> 2) & 0x03) as usize];
        buf[base + 3] = BASE_TABLE[(byte & 0x03) as usize];
    }
    // Safety: all bytes are ASCII A/C/G/T
    unsafe { String::from_utf8_unchecked(buf) }
}

/// Convert DNA bases back to bytes
/// PERF: Direct byte-level ops via lookup table, no Vec<char>
pub fn dna_to_bytes(seq: &str) -> Vec<u8> {
    let bytes = seq.as_bytes();
    let mut result = Vec::with_capacity(bytes.len() / 4 + 1);

    for chunk in bytes.chunks(4) {
        let mut byte: u8 = 0;
        for (i, &c) in chunk.iter().enumerate() {
            let val = if (c as usize) < 128 {
                let v = REVERSE_TABLE[c as usize];
                if v == 0xFF { 0 } else { v }
            } else {
                0
            };
            byte |= val << (6 - i * 2);
        }
        result.push(byte);
    }

    result
}

fn apply_rotation(data: &[u8], key: u8) -> Vec<u8> {
    data.iter().map(|&b| b.wrapping_add(key)).collect()
}

fn reverse_rotation(data: &[u8], key: u8) -> Vec<u8> {
    data.iter().map(|&b| b.wrapping_sub(key)).collect()
}

/// Calculate GC content of a DNA sequence (handles both cases)
pub fn calculate_gc(seq: &str) -> f64 {
    let bytes = seq.as_bytes();
    if bytes.is_empty() {
        return 0.0;
    }
    let gc = bytes.iter()
        .filter(|&&b| b == b'G' || b == b'C' || b == b'g' || b == b'c')
        .count() as f64;
    gc / bytes.len() as f64
}

/// Check if sequence has no homopolymer runs exceeding MAX_HOMOPOLYMER_RUN
/// Returns true if safe (no violations). Runs of exactly MAX_HOMOPOLYMER_RUN are allowed;
/// only runs strictly longer than the threshold are flagged.
pub fn check_homopolymer(seq: &str) -> bool {
    let bytes = seq.as_bytes();
    if bytes.len() < 2 {
        return true;
    }
    let mut run = 1usize;
    for i in 1..bytes.len() {
        if bytes[i] == bytes[i - 1] {
            run += 1;
            if run > MAX_HOMOPOLYMER_RUN {
                return false;
            }
        } else {
            run = 1;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_roundtrip() {
        let t = Transcoder::new();
        let data = b"Hello, DNA!";
        let encoded = t.encode(data);
        let decoded = t.decode(&encoded.sequence, encoded.rotation_key, encoded.original_length);
        assert_eq!(decoded, data);
    }

    #[test]
    fn test_gc_balance() {
        let t = Transcoder::new();
        let data = b"AAAAAAAAAAAAAAAA";
        let encoded = t.encode(data);
        assert!((encoded.gc_content - 0.5).abs() < 0.2);
    }

    #[test]
    fn test_empty_input() {
        let t = Transcoder::new();
        let encoded = t.encode(b"");
        assert_eq!(encoded.length, 0);
        assert!(encoded.homopolymer_safe);
        let decoded = t.decode(&encoded.sequence, encoded.rotation_key, 0);
        assert!(decoded.is_empty());
    }

    #[test]
    fn test_homopolymer_strict() {
        // MAX_HOMOPOLYMER_RUN = 3, meaning runs up to 3 are safe, >3 are violations
        assert!(check_homopolymer("AAACGT"));    // 3×A = safe (exactly at threshold)
        assert!(check_homopolymer("AACGT"));     // 2×A = safe
        assert!(!check_homopolymer("AAAACGT"));  // 4×A = violation (exceeds threshold)
        assert!(!check_homopolymer("ACGTTTTACG")); // 4×T = violation
        assert!(check_homopolymer("ACGTTTACG")); // 3×T = safe
    }

    #[test]
    fn test_roundtrip_binary() {
        let t = Transcoder::new();
        let data: Vec<u8> = (0..256).map(|i| i as u8).collect();
        let encoded = t.encode(&data);
        let decoded = t.decode(&encoded.sequence, encoded.rotation_key, encoded.original_length);
        assert_eq!(decoded, data);
    }

    #[test]
    fn test_case_insensitive_gc() {
        assert!((calculate_gc("GCgc") - 1.0).abs() < 0.001);
        assert!((calculate_gc("ATat") - 0.0).abs() < 0.001);
        assert!((calculate_gc("ACGTacgt") - 0.5).abs() < 0.001);
    }

    #[test]
    fn test_dna_to_bytes_case_insensitive() {
        let upper = dna_to_bytes("ACGT");
        let lower = dna_to_bytes("acgt");
        assert_eq!(upper, lower);
    }
}
