// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2025 Ved
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as published
// by the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// For commercial licensing, contact: vedcimit@gmail.com

//! DNA Constraints Engine: Biological safety & synthesis-readiness
//!
//! Enforces real-world DNA synthesis constraints:
//! - Max 3-base homopolymer runs (Twist Bioscience spec)
//! - GC content 40-60% per oligo window
//! - Restriction enzyme site screening (EcoRI, BamHI, HindIII, NotI, etc.)
//! - Forbidden biological sequences (promoters, origins)
//! - Balanced base distribution
//!
//! These constraints are CRITICAL for commercial viability — violating them
//! causes synthesis failures and increased costs.

use serde::Serialize;
use std::collections::HashMap;

/// Constraint checking results
#[derive(Debug, Clone, Serialize)]
pub struct ConstraintReport {
    pub passed: bool,
    pub total_oligos: usize,
    pub passing_oligos: usize,
    pub failing_oligos: usize,
    pub violations: Vec<Violation>,
    pub gc_stats: GCWindowStats,
    pub homopolymer_stats: HomopolymerStats,
    pub restriction_sites_found: Vec<RestrictionSiteHit>,
    pub synthesis_readiness_score: f64,  // 0.0 - 1.0
    pub estimated_synthesis_success_rate: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Violation {
    pub oligo_index: usize,
    pub position: usize,
    pub violation_type: String,
    pub detail: String,
    pub severity: String, // "critical", "warning", "info"
}

#[derive(Debug, Clone, Serialize)]
pub struct GCWindowStats {
    pub min_gc: f64,
    pub max_gc: f64,
    pub mean_gc: f64,
    pub std_dev: f64,
    pub windows_in_range: usize,
    pub windows_total: usize,
    pub target_range: (f64, f64),
}

#[derive(Debug, Clone, Serialize)]
pub struct HomopolymerStats {
    pub max_run_length: usize,
    pub total_violations: usize,
    pub run_distribution: HashMap<usize, usize>,
    pub safe: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct RestrictionSiteHit {
    pub enzyme: String,
    pub recognition_sequence: String,
    pub position: usize,
    pub oligo_index: usize,
}

/// Known restriction enzyme sites that MUST be avoided in synthesized oligos
const RESTRICTION_SITES: &[(&str, &str)] = &[
    ("EcoRI",    "GAATTC"),
    ("BamHI",    "GGATCC"),
    ("HindIII",  "AAGCTT"),
    ("NotI",     "GCGGCCGC"),
    ("XhoI",     "CTCGAG"),
    ("NdeI",     "CATATG"),
    ("BsaI",     "GGTCTC"),
    ("BbsI",     "GAAGAC"),
    ("SalI",     "GTCGAC"),
    ("PstI",     "CTGCAG"),
    ("SmaI",     "CCCGGG"),
    ("KpnI",     "GGTACC"),
    ("SacI",     "GAGCTC"),
    ("SpeI",     "ACTAGT"),
    ("NheI",     "GCTAGC"),
    ("XbaI",     "TCTAGA"),
    ("BglII",    "AGATCT"),
    ("ClaI",     "ATCGAT"),
    ("EcoRV",    "GATATC"),
    ("ApaI",     "GGGCCC"),
];

/// Dangerous biological sequences to screen against
const BIOHAZARD_PATTERNS: &[(&str, &str)] = &[
    ("T7_promoter",       "TAATACGACTCACTATAG"),
    ("SP6_promoter",      "ATTTAGGTGACACTATAG"),
    ("lac_operator",      "AATTGTGAGCGGATAACAATT"),
    ("ColE1_origin",      "CGATAAAACACGATGCGA"),
    ("pBR322_origin",     "GAGCTCGTCGAAAAAGGA"),
    ("Shine_Dalgarno",    "AAGGAG"),
    ("Kozak_consensus",   "GCCACCATG"),
    ("polyA_signal",      "AATAAA"),
];

pub struct DNAConstraints {
    pub max_homopolymer: usize,
    pub gc_min: f64,
    pub gc_max: f64,
    pub gc_window_size: usize,
    pub screen_restriction_sites: bool,
    pub screen_biohazards: bool,
}

impl Default for DNAConstraints {
    fn default() -> Self {
        Self {
            max_homopolymer: 3,       // Twist Bioscience spec
            gc_min: 0.40,             // 40% minimum
            gc_max: 0.60,             // 60% maximum
            gc_window_size: 50,       // Check GC in 50bp windows
            screen_restriction_sites: true,
            screen_biohazards: true,
        }
    }
}

impl DNAConstraints {
    pub fn new() -> Self {
        Self::default()
    }

    /// Comprehensive constraint check on a set of oligo sequences
    pub fn check_oligos(&self, oligos: &[&str]) -> ConstraintReport {
        let mut violations = Vec::new();
        let mut restriction_hits = Vec::new();
        let mut passing = 0usize;
        let mut failing_set = std::collections::HashSet::new();

        // Per-oligo checks
        for (idx, &oligo) in oligos.iter().enumerate() {
            let mut oligo_ok = true;

            // Homopolymer check
            let hp_violations = self.check_homopolymer(oligo, idx);
            if !hp_violations.is_empty() {
                oligo_ok = false;
                violations.extend(hp_violations);
            }

            // GC content per oligo
            let gc = calculate_gc_content(oligo);
            if gc < self.gc_min || gc > self.gc_max {
                oligo_ok = false;
                violations.push(Violation {
                    oligo_index: idx,
                    position: 0,
                    violation_type: "gc_content".to_string(),
                    detail: format!("GC={:.1}% (allowed: {:.0}%-{:.0}%)", gc * 100.0, self.gc_min * 100.0, self.gc_max * 100.0),
                    severity: if gc < 0.30 || gc > 0.70 { "critical" } else { "warning" }.to_string(),
                });
            }

            // Restriction sites
            if self.screen_restriction_sites {
                let hits = self.find_restriction_sites(oligo, idx);
                if !hits.is_empty() {
                    oligo_ok = false;
                    for hit in &hits {
                        violations.push(Violation {
                            oligo_index: idx,
                            position: hit.position,
                            violation_type: "restriction_site".to_string(),
                            detail: format!("{} site ({}) at pos {}", hit.enzyme, hit.recognition_sequence, hit.position),
                            severity: "critical".to_string(),
                        });
                    }
                    restriction_hits.extend(hits);
                }
            }

            // Biohazard screening
            if self.screen_biohazards {
                let bio_violations = self.screen_biohazard_sequences(oligo, idx);
                if !bio_violations.is_empty() {
                    // Biohazards are warnings, not failures
                    violations.extend(bio_violations);
                }
            }

            if oligo_ok {
                passing += 1;
            } else {
                failing_set.insert(idx);
            }
        }

        // Global GC window analysis
        let gc_stats = self.analyze_gc_windows(oligos);

        // Global homopolymer stats
        let hp_stats = self.analyze_homopolymers(oligos);

        // Synthesis readiness score
        let score = self.compute_synthesis_score(oligos, &violations, &gc_stats, &hp_stats);

        ConstraintReport {
            passed: failing_set.is_empty(),
            total_oligos: oligos.len(),
            passing_oligos: passing,
            failing_oligos: failing_set.len(),
            violations,
            gc_stats,
            homopolymer_stats: hp_stats,
            restriction_sites_found: restriction_hits,
            synthesis_readiness_score: score,
            estimated_synthesis_success_rate: (score * 100.0).round() / 100.0,
        }
    }

    /// PERF: Operates directly on bytes (valid DNA is ASCII), avoiding Vec<char> allocation.
    fn check_homopolymer(&self, seq: &str, oligo_idx: usize) -> Vec<Violation> {
        let mut violations = Vec::new();
        let bytes = seq.as_bytes();
        if bytes.is_empty() { return violations; }

        let mut run = 1usize;
        let mut run_start = 0usize;
        for i in 1..bytes.len() {
            if bytes[i] == bytes[i - 1] {
                run += 1;
            } else {
                if run > self.max_homopolymer {
                    violations.push(Violation {
                        oligo_index: oligo_idx,
                        position: run_start,
                        violation_type: "homopolymer".to_string(),
                        detail: format!("{}×{} at pos {} (max allowed: {})",
                            bytes[run_start] as char, run, run_start, self.max_homopolymer),
                        severity: if run > 5 { "critical" } else { "warning" }.to_string(),
                    });
                }
                run = 1;
                run_start = i;
            }
        }
        if run > self.max_homopolymer {
            violations.push(Violation {
                oligo_index: oligo_idx,
                position: run_start,
                violation_type: "homopolymer".to_string(),
                detail: format!("{}×{} at pos {} (max allowed: {})",
                    bytes[run_start] as char, run, run_start, self.max_homopolymer),
                severity: if run > 5 { "critical" } else { "warning" }.to_string(),
            });
        }
        violations
    }

    fn find_restriction_sites(&self, seq: &str, oligo_idx: usize) -> Vec<RestrictionSiteHit> {
        let mut hits = Vec::new();
        let seq_upper = seq.to_uppercase();

        for &(enzyme, site) in RESTRICTION_SITES {
            let mut pos = 0;
            while let Some(found) = seq_upper[pos..].find(site) {
                hits.push(RestrictionSiteHit {
                    enzyme: enzyme.to_string(),
                    recognition_sequence: site.to_string(),
                    position: pos + found,
                    oligo_index: oligo_idx,
                });
                pos += found + 1;
            }
            // Also check reverse complement
            let rc = reverse_complement(site);
            pos = 0;
            while let Some(found) = seq_upper[pos..].find(&rc) {
                hits.push(RestrictionSiteHit {
                    enzyme: format!("{}_rc", enzyme),
                    recognition_sequence: rc.clone(),
                    position: pos + found,
                    oligo_index: oligo_idx,
                });
                pos += found + 1;
            }
        }

        hits
    }

    fn screen_biohazard_sequences(&self, seq: &str, oligo_idx: usize) -> Vec<Violation> {
        let mut violations = Vec::new();
        let seq_upper = seq.to_uppercase();

        for &(name, pattern) in BIOHAZARD_PATTERNS {
            if seq_upper.contains(pattern) {
                violations.push(Violation {
                    oligo_index: oligo_idx,
                    position: seq_upper.find(pattern).unwrap_or(0),
                    violation_type: "biohazard".to_string(),
                    detail: format!("Contains {} sequence", name),
                    severity: "warning".to_string(),
                });
            }
        }

        violations
    }

    /// PERF: Uses byte-level GC counting instead of Vec<char> conversion.
    fn analyze_gc_windows(&self, oligos: &[&str]) -> GCWindowStats {
        if self.gc_window_size == 0 {
            return GCWindowStats {
                min_gc: 0.0,
                max_gc: 0.0,
                mean_gc: 0.0,
                std_dev: 0.0,
                windows_in_range: 0,
                windows_total: 0,
                target_range: (self.gc_min, self.gc_max),
            };
        }

        // Analyze GC windows within each oligo independently — sliding a window
        // across concatenated oligos would alias boundary regions between unrelated sequences
        let mut gc_values = Vec::new();
        let step = (self.gc_window_size / 2).max(1);

        for oligo in oligos {
            let bytes = oligo.as_bytes();
            let mut i = 0;
            while i + self.gc_window_size <= bytes.len() {
                let gc = bytes[i..i + self.gc_window_size].iter()
                    .filter(|&&b| b == b'G' || b == b'C')
                    .count() as f64 / self.gc_window_size as f64;
                gc_values.push(gc);
                i += step;
            }
        }

        if gc_values.is_empty() {
            return GCWindowStats {
                min_gc: 0.0, max_gc: 0.0, mean_gc: 0.0, std_dev: 0.0,
                windows_in_range: 0, windows_total: 0,
                target_range: (self.gc_min, self.gc_max),
            };
        }

        let mean = gc_values.iter().sum::<f64>() / gc_values.len() as f64;
        let variance = gc_values.iter().map(|&x| (x - mean).powi(2)).sum::<f64>() / gc_values.len() as f64;
        let in_range = gc_values.iter().filter(|&&g| g >= self.gc_min && g <= self.gc_max).count();

        GCWindowStats {
            min_gc: gc_values.iter().cloned().fold(f64::INFINITY, f64::min),
            max_gc: gc_values.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
            mean_gc: (mean * 10000.0).round() / 10000.0,
            std_dev: (variance.sqrt() * 10000.0).round() / 10000.0,
            windows_in_range: in_range,
            windows_total: gc_values.len(),
            target_range: (self.gc_min, self.gc_max),
        }
    }

    /// PERF: Byte-level analysis, no char conversion.
    fn analyze_homopolymers(&self, oligos: &[&str]) -> HomopolymerStats {
        let mut max_run = 0usize;
        let mut total_violations = 0usize;
        let mut dist: HashMap<usize, usize> = HashMap::new();

        for oligo in oligos {
            let bytes = oligo.as_bytes();
            if bytes.is_empty() { continue; }

            let mut run = 1usize;
            for i in 1..bytes.len() {
                if bytes[i] == bytes[i - 1] {
                    run += 1;
                } else {
                    if run >= 2 {
                        *dist.entry(run).or_insert(0) += 1;
                        max_run = max_run.max(run);
                        if run > self.max_homopolymer {
                            total_violations += 1;
                        }
                    }
                    run = 1;
                }
            }
            if run >= 2 {
                *dist.entry(run).or_insert(0) += 1;
                max_run = max_run.max(run);
                if run > self.max_homopolymer {
                    total_violations += 1;
                }
            }
        }

        HomopolymerStats {
            max_run_length: max_run,
            total_violations,
            run_distribution: dist,
            safe: total_violations == 0 && max_run <= self.max_homopolymer,
        }
    }

    fn compute_synthesis_score(
        &self,
        oligos: &[&str],
        violations: &[Violation],
        gc_stats: &GCWindowStats,
        hp_stats: &HomopolymerStats,
    ) -> f64 {
        if oligos.is_empty() { return 0.0; }

        let mut score = 1.0;

        // GC balance penalty
        let gc_deviation = (gc_stats.mean_gc - 0.5).abs();
        score -= gc_deviation * 0.5; // Up to -25% for extreme GC

        // GC window compliance
        if gc_stats.windows_total > 0 {
            let compliance = gc_stats.windows_in_range as f64 / gc_stats.windows_total as f64;
            score *= 0.7 + 0.3 * compliance; // 30% weight on window compliance
        }

        // Homopolymer penalty
        if hp_stats.max_run_length > self.max_homopolymer {
            let excess = hp_stats.max_run_length - self.max_homopolymer;
            score -= excess as f64 * 0.05;
        }

        // Restriction site penalty (critical)
        let critical_count = violations.iter()
            .filter(|v| v.severity == "critical")
            .count();
        score -= critical_count as f64 * 0.02;

        // Biohazard penalty (mild)
        let bio_count = violations.iter()
            .filter(|v| v.violation_type == "biohazard")
            .count();
        score -= bio_count as f64 * 0.005;

        score.max(0.0).min(1.0)
    }
}

// ═══════════ Utility Functions ═══════════

pub fn calculate_gc_content(seq: &str) -> f64 {
    if seq.is_empty() { return 0.0; }
    let gc = seq.chars().filter(|&c| c == 'G' || c == 'C' || c == 'g' || c == 'c').count();
    gc as f64 / seq.len() as f64
}

pub fn reverse_complement(seq: &str) -> String {
    seq.chars().rev().map(|c| match c {
        'A' => 'T', 'T' => 'A', 'G' => 'C', 'C' => 'G',
        'a' => 't', 't' => 'a', 'g' => 'c', 'c' => 'g',
        _ => c,
    }).collect()
}

/// Calculate melting temperature (nearest-neighbor method, simplified)
/// PERF: Uses compile-time lookup array instead of runtime HashMap allocations.
pub fn melting_temperature(seq: &str) -> f64 {
    let len = seq.len();
    if len < 8 { return 0.0; }

    // Encode base pair → index: AA=0, AT=1, ..., TT=15
    // Row = first base (A=0,C=1,G=2,T=3), Col = second base
    // Nearest-neighbor ΔH (kcal/mol) — row-major [first][second]
    const NN_DH: [[f64; 4]; 4] = [
        // A       C       G       T       (second base)
        [-7.9,  -8.4,  -7.8,  -7.2],  // A (first)
        [-8.5,  -8.0,  -10.6, -7.8],  // C
        [-8.2,  -9.8,  -8.0,  -8.4],  // G
        [-7.2,  -8.5,  -8.2,  -7.9],  // T
    ];
    // Nearest-neighbor ΔS (cal/mol·K)
    const NN_DS: [[f64; 4]; 4] = [
        [-22.2, -22.4, -21.0, -20.4],  // A
        [-22.7, -19.9, -27.2, -21.0],  // C
        [-22.2, -24.4, -19.9, -22.4],  // G
        [-21.3, -22.7, -22.2, -22.2],  // T
    ];

    #[inline]
    fn base_idx(b: u8) -> Option<usize> {
        match b {
            b'A' | b'a' => Some(0),
            b'C' | b'c' => Some(1),
            b'G' | b'g' => Some(2),
            b'T' | b't' => Some(3),
            _ => None,
        }
    }

    let bytes = seq.as_bytes();
    let mut total_dh = 0.0f64;
    let mut total_ds = 0.0f64;

    for i in 0..bytes.len() - 1 {
        if let (Some(a), Some(b)) = (base_idx(bytes[i]), base_idx(bytes[i + 1])) {
            total_dh += NN_DH[a][b];
            total_ds += NN_DS[a][b];
        }
    }

    // Initiation parameters
    total_dh += 0.1;
    total_ds += -2.8;

    // Tm = ΔH / (ΔS + R * ln(Ct/4)) - 273.15
    let r = 1.987; // cal/(mol·K)
    let ct: f64 = 250e-9; // 250 nM
    let tm = (total_dh * 1000.0) / (total_ds + r * (ct / 4.0_f64).ln()) - 273.15;

    (tm * 10.0).round() / 10.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_perfect_oligo() {
        let constraints = DNAConstraints::new();
        let oligo = "ACGTACGTACGTACGTACGTACGTACGTACGTACGTACGTACGTACGTACGT"; // Balanced
        let report = constraints.check_oligos(&[oligo]);
        assert!(report.gc_stats.mean_gc > 0.45);
        assert!(report.gc_stats.mean_gc < 0.55);
    }

    #[test]
    fn test_homopolymer_detection() {
        let constraints = DNAConstraints::new();
        let bad_oligo = "ACGTAAAACCCC"; // 4×A and 4×C violate max_homopolymer=3
        let report = constraints.check_oligos(&[bad_oligo]);
        assert!(!report.homopolymer_stats.safe);
        assert!(report.homopolymer_stats.total_violations > 0);
    }

    #[test]
    fn test_restriction_site_detection() {
        let constraints = DNAConstraints::new();
        // Contains EcoRI site GAATTC
        let oligo = "ACGTGAATTCACGT";
        let report = constraints.check_oligos(&[oligo]);
        assert!(!report.restriction_sites_found.is_empty());
        assert_eq!(report.restriction_sites_found[0].enzyme, "EcoRI");
    }

    #[test]
    fn test_gc_out_of_range() {
        let constraints = DNAConstraints::new();
        let gc_high = "GCGCGCGCGCGCGCGCGCGC"; // 100% GC
        let gc_low = "ATATATATATATATATATATATATAT"; // 0% GC
        let report_h = constraints.check_oligos(&[gc_high]);
        let report_l = constraints.check_oligos(&[gc_low]);
        assert!(report_h.violations.iter().any(|v| v.violation_type == "gc_content"));
        assert!(report_l.violations.iter().any(|v| v.violation_type == "gc_content"));
    }

    #[test]
    fn test_reverse_complement() {
        assert_eq!(reverse_complement("GAATTC"), "GAATTC"); // EcoRI is palindromic
        assert_eq!(reverse_complement("ACGT"), "ACGT");
        assert_eq!(reverse_complement("AAAA"), "TTTT");
    }

    #[test]
    fn test_melting_temp() {
        let tm = melting_temperature("ACGTACGTACGTACGT");
        assert!(tm > 20.0 && tm < 80.0, "Tm should be reasonable: {}", tm);
    }
}
