//! Oligo Quality Scorer & Structured Oligo Builder
//!
//! Production-grade oligo format:
//!   [Forward Primer (20bp)] [Index (16bp)] [Payload (variable)] [CRC-8 (4bp)] [Reverse Primer (20bp)]
//!
//! Each oligo is a self-contained, addressable, error-detectable unit.
//! The index enables random access to individual data segments.
//! CRC provides fast error detection before expensive RS decoding.
//! Primers enable PCR amplification and sequencing.

use serde::Serialize;
use std::collections::HashMap;

/// Standard primer sequences (20bp each) for PCR amplification
/// These are designed to have balanced GC, no self-complementarity,
/// and no cross-reactivity with common organisms.
pub const FORWARD_PRIMER: &str = "CTACACGACGCTCTTCCGAT";  // 20bp
pub const REVERSE_PRIMER: &str = "AGACGTGTGCTCTTCCGATC";  // 20bp
pub const PRIMER_LENGTH: usize = 20;

/// Index field: 16 bases = 8 bytes = can address 4 billion oligos
pub const INDEX_LENGTH: usize = 16;

/// CRC checksum: 16 DNA bases = 4 bytes = CRC-32
/// CRC-32 provides 2^32 possible values — collision probability < 0.00000024% per oligo.
pub const CRC_LENGTH: usize = 16;

/// Total overhead per oligo (primers + index + CRC-32)
pub const OVERHEAD_PER_OLIGO: usize = PRIMER_LENGTH * 2 + INDEX_LENGTH + CRC_LENGTH; // 72bp

#[derive(Debug, Clone, Serialize)]
pub struct StructuredOligo {
    pub index: u32,
    pub forward_primer: String,
    pub index_field: String,
    pub payload: String,
    pub crc_field: String,
    pub reverse_primer: String,
    pub full_sequence: String,
    pub quality_score: f64,
    pub payload_length: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct OligoQualityReport {
    pub total_oligos: usize,
    pub mean_quality: f64,
    pub min_quality: f64,
    pub max_quality: f64,
    pub quality_distribution: HashMap<String, usize>, // "excellent", "good", "fair", "poor"
    pub total_payload_bases: usize,
    pub payload_per_oligo: usize,
    pub overhead_per_oligo: usize,
    pub payload_efficiency: f64, // payload / total length
    pub total_data_capacity_bytes: usize,
    pub crc_pass_rate: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct OligoBuildStats {
    pub num_oligos: usize,
    pub oligo_total_length: usize,
    pub payload_length: usize,
    pub index_bits: usize,
    pub has_primers: bool,
    pub has_crc: bool,
    pub has_index: bool,
}

pub struct OligoBuilder {
    pub oligo_total_length: usize,
    pub include_primers: bool,
    pub include_index: bool,
    pub include_crc: bool,
}

impl Default for OligoBuilder {
    fn default() -> Self {
        Self {
            oligo_total_length: 200, // Twist Bioscience standard
            include_primers: true,
            include_index: true,
            include_crc: true,
        }
    }
}

impl OligoBuilder {
    pub fn new(total_length: usize) -> Self {
        Self {
            oligo_total_length: total_length,
            ..Default::default()
        }
    }

    /// Calculate available payload space per oligo
    pub fn payload_capacity(&self) -> usize {
        let overhead = if self.include_primers { PRIMER_LENGTH * 2 } else { 0 }
            + if self.include_index { INDEX_LENGTH } else { 0 }
            + if self.include_crc { CRC_LENGTH } else { 0 };
        self.oligo_total_length.saturating_sub(overhead)
    }

    /// Build structured oligos from a DNA payload sequence
    pub fn build_oligos(&self, dna_sequence: &str) -> (Vec<StructuredOligo>, OligoBuildStats) {
        let payload_cap = self.payload_capacity();
        if payload_cap == 0 {
            return (
                Vec::new(),
                OligoBuildStats {
                    num_oligos: 0,
                    oligo_total_length: self.oligo_total_length,
                    payload_length: 0,
                    index_bits: if self.include_index { 32 } else { 0 },
                    has_primers: self.include_primers,
                    has_crc: self.include_crc,
                    has_index: self.include_index,
                },
            );
        }
        let chars: Vec<char> = dna_sequence.chars().collect();
        let mut oligos = Vec::new();

        for (i, chunk) in chars.chunks(payload_cap).enumerate() {
            let payload: String = chunk.iter().collect();
            let index = i as u32;

            // Build index field (4 bytes → 16 DNA bases)
            // XOR with constant to avoid homopolymer runs at low index values
            let index_field = if self.include_index {
                let scrambled = index ^ 0xA5C3_3CA5; // break homopolymers
                bytes_to_dna_bases(&scrambled.to_le_bytes())
            } else {
                String::new()
            };

            // Compute CRC-32 over (index + payload) → 4 bytes → 16 DNA bases
            let crc_field = if self.include_crc {
                let mut crc_data = Vec::new();
                if self.include_index {
                    crc_data.extend_from_slice(&index.to_le_bytes());
                }
                crc_data.extend_from_slice(payload.as_bytes());
                let crc = compute_crc32(&crc_data);
                bytes_to_dna_bases(&crc.to_le_bytes())
            } else {
                String::new()
            };

            // Assemble full oligo
            let fwd = if self.include_primers { FORWARD_PRIMER } else { "" };
            let rev = if self.include_primers { REVERSE_PRIMER } else { "" };

            let full = format!("{}{}{}{}{}", fwd, index_field, payload, crc_field, rev);

            let quality = compute_oligo_quality(&full);

            oligos.push(StructuredOligo {
                index,
                forward_primer: fwd.to_string(),
                index_field: index_field.clone(),
                payload: payload.clone(),
                crc_field: crc_field.clone(),
                reverse_primer: rev.to_string(),
                full_sequence: full,
                quality_score: quality,
                payload_length: payload.len(),
            });
        }

        let stats = OligoBuildStats {
            num_oligos: oligos.len(),
            oligo_total_length: self.oligo_total_length,
            payload_length: payload_cap,
            index_bits: if self.include_index { 32 } else { 0 },
            has_primers: self.include_primers,
            has_crc: self.include_crc,
            has_index: self.include_index,
        };

        (oligos, stats)
    }

    /// Disassemble structured oligos from raw FASTA sequences.
    /// Strips primers, extracts index + payload + CRC, verifies CRC, sorts by index.
    /// Returns (sorted payloads, num_crc_pass, num_crc_fail).
    pub fn disassemble_oligos(&self, raw_sequences: &[String]) -> Result<(Vec<String>, usize, usize), String> {
        let payload_cap = self.payload_capacity();
        if payload_cap == 0 {
            return Err("Payload capacity is zero".to_string());
        }

        let fwd_len = if self.include_primers { PRIMER_LENGTH } else { 0 };
        let rev_len = if self.include_primers { PRIMER_LENGTH } else { 0 };
        let idx_len = if self.include_index { INDEX_LENGTH } else { 0 };
        let crc_len = if self.include_crc { CRC_LENGTH } else { 0 };
        let min_len = fwd_len + idx_len + crc_len + rev_len + 1; // at least 1bp payload

        let mut indexed_payloads: Vec<(u32, String)> = Vec::new();
        let mut crc_pass = 0usize;
        let mut crc_fail = 0usize;

        for seq in raw_sequences {
            if seq.len() < min_len {
                continue; // Skip truncated oligos
            }

            // Strip forward primer
            let after_fwd = &seq[fwd_len..];
            // Strip reverse primer from end
            let before_rev = if rev_len > 0 && after_fwd.len() > rev_len {
                &after_fwd[..after_fwd.len() - rev_len]
            } else {
                after_fwd
            };

            // Extract index field
            let (index, after_idx) = if self.include_index && before_rev.len() >= idx_len {
                let idx_dna = &before_rev[..idx_len];
                let idx_bytes = dna_bases_to_bytes(idx_dna);
                if idx_bytes.len() >= 4 {
                    let scrambled = u32::from_le_bytes([idx_bytes[0], idx_bytes[1], idx_bytes[2], idx_bytes[3]]);
                    let index = scrambled ^ 0xA5C3_3CA5;
                    (index, &before_rev[idx_len..])
                } else {
                    continue;
                }
            } else {
                (indexed_payloads.len() as u32, before_rev)
            };

            // Extract CRC field (at end of remaining)
            let (payload, crc_field) = if self.include_crc && after_idx.len() >= crc_len {
                let payload = &after_idx[..after_idx.len() - crc_len];
                let crc = &after_idx[after_idx.len() - crc_len..];
                (payload.to_string(), crc.to_string())
            } else {
                (after_idx.to_string(), String::new())
            };

            // Verify CRC
            if self.include_crc && !crc_field.is_empty() {
                let mut crc_data = Vec::new();
                if self.include_index {
                    crc_data.extend_from_slice(&index.to_le_bytes());
                }
                crc_data.extend_from_slice(payload.as_bytes());
                let expected = compute_crc32(&crc_data);
                let expected_field = bytes_to_dna_bases(&expected.to_le_bytes());
                if crc_field == expected_field {
                    crc_pass += 1;
                } else {
                    crc_fail += 1;
                }
            } else {
                crc_pass += 1;
            }

            indexed_payloads.push((index, payload));
        }

        if indexed_payloads.is_empty() {
            return Err("No valid oligos found in FASTA data".to_string());
        }

        // Sort by index
        indexed_payloads.sort_by_key(|(idx, _)| *idx);

        let payloads: Vec<String> = indexed_payloads.into_iter().map(|(_, p)| p).collect();
        Ok((payloads, crc_pass, crc_fail))
    }

    /// Verify CRC-32 integrity of a structured oligo
    pub fn verify_crc(&self, oligo: &StructuredOligo) -> bool {
        if !self.include_crc { return true; }

        let mut crc_data = Vec::new();
        if self.include_index {
            crc_data.extend_from_slice(&oligo.index.to_le_bytes());
        }
        crc_data.extend_from_slice(oligo.payload.as_bytes());
        let expected = compute_crc32(&crc_data);
        let expected_field = bytes_to_dna_bases(&expected.to_le_bytes());

        oligo.crc_field == expected_field
    }

    /// Generate quality report for a set of oligos
    pub fn quality_report(&self, oligos: &[StructuredOligo]) -> OligoQualityReport {
        if oligos.is_empty() {
            return OligoQualityReport {
                total_oligos: 0, mean_quality: 0.0, min_quality: 0.0,
                max_quality: 0.0, quality_distribution: HashMap::new(),
                total_payload_bases: 0, payload_per_oligo: 0,
                overhead_per_oligo: 0, payload_efficiency: 0.0,
                total_data_capacity_bytes: 0, crc_pass_rate: 0.0,
            };
        }

        let qualities: Vec<f64> = oligos.iter().map(|o| o.quality_score).collect();
        let mean = qualities.iter().sum::<f64>() / qualities.len() as f64;
        let min = qualities.iter().cloned().fold(f64::INFINITY, f64::min);
        let max = qualities.iter().cloned().fold(f64::NEG_INFINITY, f64::max);

        let mut dist = HashMap::new();
        for &q in &qualities {
            let category = if q >= 0.9 { "excellent" }
                else if q >= 0.7 { "good" }
                else if q >= 0.5 { "fair" }
                else { "poor" };
            *dist.entry(category.to_string()).or_insert(0) += 1;
        }

        let total_payload: usize = oligos.iter().map(|o| o.payload_length).sum();
        let overhead = self.oligo_total_length.saturating_sub(self.payload_capacity());
        let efficiency = if self.oligo_total_length > 0 {
            self.payload_capacity() as f64 / self.oligo_total_length as f64
        } else { 0.0 };

        let crc_pass = oligos.iter().filter(|o| self.verify_crc(o)).count();

        OligoQualityReport {
            total_oligos: oligos.len(),
            mean_quality: (mean * 10000.0).round() / 10000.0,
            min_quality: (min * 10000.0).round() / 10000.0,
            max_quality: (max * 10000.0).round() / 10000.0,
            quality_distribution: dist,
            total_payload_bases: total_payload,
            payload_per_oligo: self.payload_capacity(),
            overhead_per_oligo: overhead,
            payload_efficiency: (efficiency * 10000.0).round() / 10000.0,
            total_data_capacity_bytes: total_payload / 4, // 4 bases per byte
            crc_pass_rate: if oligos.is_empty() { 0.0 } else { crc_pass as f64 / oligos.len() as f64 },
        }
    }
}

// ═══════════ Quality Scoring ═══════════

fn compute_oligo_quality(seq: &str) -> f64 {
    if seq.is_empty() { return 0.0; }

    let mut score = 1.0;

    // 1. GC balance (target 40-60%)
    let gc = seq.chars().filter(|&c| c == 'G' || c == 'C').count() as f64 / seq.len() as f64;
    let gc_dev = (gc - 0.5).abs();
    score -= gc_dev * 1.5; // Penalize GC deviation

    // 2. Homopolymer runs
    let max_run = max_homopolymer_run(seq);
    if max_run > 3 {
        score -= (max_run - 3) as f64 * 0.1;
    }

    // 3. Local GC uniformity (check 20bp windows)
    let chars: Vec<char> = seq.chars().collect();
    let window = 20;
    let mut gc_variance = 0.0;
    let mut n_windows = 0;
    let mut i = 0;
    while i + window <= chars.len() {
        let local_gc = chars[i..i + window].iter().filter(|&&c| c == 'G' || c == 'C').count() as f64 / window as f64;
        gc_variance += (local_gc - 0.5).powi(2);
        n_windows += 1;
        i += window;
    }
    if n_windows > 0 {
        gc_variance /= n_windows as f64;
        score -= gc_variance * 2.0; // Penalize non-uniform GC
    }

    // 4. Self-complementarity check (simplified)
    if has_strong_secondary_structure(seq) {
        score -= 0.15;
    }

    score.max(0.0).min(1.0)
}

fn max_homopolymer_run(seq: &str) -> usize {
    if seq.is_empty() { return 0; }
    let mut max_run = 1; // FIX: minimum run is 1 for non-empty sequences
    let mut run = 1;
    let chars: Vec<char> = seq.chars().collect();
    for i in 1..chars.len() {
        if chars[i] == chars[i - 1] {
            run += 1;
            max_run = max_run.max(run);
        } else {
            run = 1;
        }
    }
    max_run
}

/// Simple check for strong secondary structure (hairpins)
fn has_strong_secondary_structure(seq: &str) -> bool {
    let chars: Vec<char> = seq.chars().collect();
    let len = chars.len();
    if len < 20 { return false; }

    // Check for palindromic stretches >= 8bp
    for i in 0..len.saturating_sub(20) {
        for window in [8, 10, 12] {
            if i + window * 2 > len { continue; }
            let fwd: String = chars[i..i + window].iter().collect();
            let rev: String = chars[i + window..i + window * 2].iter().rev().map(|&c| match c {
                'A' => 'T', 'T' => 'A', 'G' => 'C', 'C' => 'G', _ => c
            }).collect();
            if fwd == rev {
                return true;
            }
        }
    }
    false
}

// ═══════════ Utility ═══════════

/// CRC-32 computation (IEEE 802.3 polynomial)
fn compute_crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ 0xEDB8_8320; // Reversed polynomial
            } else {
                crc >>= 1;
            }
        }
    }
    crc ^ 0xFFFF_FFFF
}

fn bytes_to_dna_bases(data: &[u8]) -> String {
    const BASES: [char; 4] = ['A', 'C', 'G', 'T'];
    let mut seq = String::with_capacity(data.len() * 4);
    for &byte in data {
        seq.push(BASES[((byte >> 6) & 0x03) as usize]);
        seq.push(BASES[((byte >> 4) & 0x03) as usize]);
        seq.push(BASES[((byte >> 2) & 0x03) as usize]);
        seq.push(BASES[(byte & 0x03) as usize]);
    }
    seq
}

fn dna_bases_to_bytes(dna: &str) -> Vec<u8> {
    let chars: Vec<char> = dna.chars().collect();
    let mut result = Vec::with_capacity(chars.len() / 4 + 1);
    for chunk in chars.chunks(4) {
        let mut byte: u8 = 0;
        for (i, &c) in chunk.iter().enumerate() {
            let val = match c {
                'A' | 'a' => 0u8,
                'C' | 'c' => 1u8,
                'G' | 'g' => 2u8,
                'T' | 't' => 3u8,
                _ => 0u8,
            };
            byte |= val << (6 - i * 2);
        }
        result.push(byte);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_oligo_builder_basic() {
        let builder = OligoBuilder::new(200);
        assert_eq!(builder.payload_capacity(), 200 - 72); // 128bp payload (CRC-32 = 16bp)

        let dna = "ACGT".repeat(100); // 400 bases = 4 oligos at 128bp payload
        let (oligos, stats) = builder.build_oligos(&dna);
        assert_eq!(stats.num_oligos, 4); // ceil(400/128) = 4
        assert!(oligos[0].full_sequence.starts_with(FORWARD_PRIMER));
        assert!(oligos[0].full_sequence.ends_with(REVERSE_PRIMER));
    }

    #[test]
    fn test_crc_integrity() {
        let builder = OligoBuilder::new(200);
        let dna = "ACGTACGTACGTACGT".repeat(10);
        let (oligos, _) = builder.build_oligos(&dna);

        for oligo in &oligos {
            assert!(builder.verify_crc(oligo), "CRC should pass for intact oligo");
        }
    }

    #[test]
    fn test_quality_scoring() {
        let balanced = "ACGTACGTACGTACGTACGTACGTACGTACGTACGTACGTACGTACGTACGT";
        let q = compute_oligo_quality(balanced);
        assert!(q > 0.7, "Balanced sequence should score well: {}", q);

        let gc_heavy = "GCGCGCGCGCGCGCGCGCGCGCGCGCGCGCGCGCGCGCGCGCGCGCGCGC";
        let q2 = compute_oligo_quality(gc_heavy);
        assert!(q2 < q, "High-GC should score worse than balanced");
    }

    #[test]
    fn test_crc32() {
        let data = b"Hello";
        let crc = compute_crc32(data);
        assert_ne!(crc, 0); // Should produce a non-zero checksum

        // Same data should produce same CRC
        let crc2 = compute_crc32(data);
        assert_eq!(crc, crc2);

        // Different data should produce different CRC
        let crc3 = compute_crc32(b"World");
        assert_ne!(crc, crc3);
    }

    #[test]
    fn test_quality_report() {
        let builder = OligoBuilder::new(200);
        let dna = "ACGT".repeat(200);
        let (oligos, _) = builder.build_oligos(&dna);
        let report = builder.quality_report(&oligos);

        assert_eq!(report.total_oligos, oligos.len());
        assert!(report.mean_quality > 0.0);
        assert_eq!(report.crc_pass_rate, 1.0);
        assert!(report.payload_efficiency > 0.5);
    }
}
