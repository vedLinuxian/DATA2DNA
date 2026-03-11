// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2025 Ved
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as published
// by the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// For commercial licensing, contact: vedcimit@gmail.com

//! Pipeline Orchestrator: Connects all modules into a unified API.
//!
//! ENCODE: Data → Compress → RS(encode) → Fountain → Transcode → OligoBuilder → Constraints → FASTA → CostEstimate
//! CHAOS:  Mutate droplets + DNA sequence
//! DECODE: Surviving droplets → Fountain decode → RS(decode+correct) → Decompress → Original

use crate::chaos::{ChaosMatrix, ChaosStats, MutationSummary};
use crate::compressor::{hex_sha256, CompressionStats, HelixCompressor};
use crate::consensus::{ConsensusDecoder, DecodeStats};
use crate::cost_estimator::{CostEstimate, CostEstimator};
use crate::dna_constraints::{ConstraintReport, DNAConstraints};
use crate::fasta::{FastaIO, FastaMetadata, FastaStats};
use crate::fountain::{Droplet, FountainCodec, FountainEncoded, FountainStats};
use crate::hypercompress::HyperCompressor;
use crate::interleaved_rs::InterleavedRS;
use crate::oligo_builder::{OligoBuildStats, OligoBuilder, OligoQualityReport};
use crate::reed_solomon::{RSStats, ReedSolomonCodec};
use crate::transcoder::{Transcoder, TranscodeResult};
use serde::Serialize;
use std::time::Instant;

#[derive(Debug, Clone)]
pub struct PipelineConfig {
    pub block_size: usize,
    pub redundancy: f64,
    pub seed: u64,
    pub compression: bool,
    pub compression_level: String,
    pub deletion_rate: f64,
    pub substitution_rate: f64,
    pub insertion_rate: f64,
    pub oligo_length: usize,
}

impl Default for PipelineConfig {
    fn default() -> Self {
        Self {
            block_size: 64,
            redundancy: 2.0,
            seed: 42,
            compression: true,
            compression_level: "ultra".to_string(),
            deletion_rate: 0.15,
            substitution_rate: 0.05,
            insertion_rate: 0.02,
            oligo_length: 300,
        }
    }
}

/// Adaptive Redundancy Calculator
///
/// Novel algorithm: Calculates the minimum redundancy ratio needed to guarantee
/// data recovery with probability ≥ (1 - target_failure_prob) given expected
/// channel erasure/error rates per the DNA storage degradation model.
///
/// Based on information-theoretic bounds:
/// - Shannon channel capacity for erasure channel: C = 1 - ε
/// - Fountain code overhead: ~5% above capacity for Robust Soliton
/// - RS correction budget: t = parity/2 errors per block
///
/// Returns optimal redundancy as a multiplier (e.g., 2.0 = 2x data).
pub fn calculate_adaptive_redundancy(
    data_size_bytes: usize,
    expected_oligo_loss_rate: f64,
    expected_substitution_rate: f64,
    target_failure_prob: f64,
) -> f64 {
    // Channel capacity for erasure channel
    let erasure_rate = expected_oligo_loss_rate.clamp(0.0, 0.99);
    let channel_capacity = 1.0 - erasure_rate;
    
    // Minimum redundancy from Shannon bound (must be > 1/C)
    let shannon_min = if channel_capacity > 0.01 {
        1.0 / channel_capacity
    } else {
        100.0 // Channel is essentially dead
    };
    
    // Fountain code overhead factor (Robust Soliton with given failure prob)
    // From Luby 2002: overhead ≈ c * sqrt(k) * ln(k/δ) / k
    // For practical k (data blocks), this is typically 5-15%
    let k = (data_size_bytes as f64 / 64.0).max(1.0); // number of blocks
    let fountain_overhead = 1.0 + 0.025 * (k / target_failure_prob).ln() / k.sqrt();
    
    // RS correction budget overhead
    // With RS(255,223), we can correct 16 errors per 255-byte block
    // If substitution rate is high, we need more redundancy so RS can handle it
    let rs_overhead = if expected_substitution_rate > 0.01 {
        1.0 + expected_substitution_rate * 2.0 // Additional margin for RS
    } else {
        1.0
    };
    
    // Safety margin based on target failure probability
    let safety_margin = if target_failure_prob < 0.001 {
        1.3 // 30% safety margin for 99.9% recovery
    } else if target_failure_prob < 0.01 {
        1.2 // 20% safety margin for 99% recovery
    } else {
        1.1 // 10% safety margin
    };
    
    let optimal = shannon_min * fountain_overhead * rs_overhead * safety_margin;
    
    // Clamp to reasonable range
    optimal.clamp(1.2, 10.0)
}

/// Shannon Entropy Estimator
///
/// Computes the first-order Shannon entropy H(X) = -Σ p(x) log2(p(x))
/// of arbitrary byte data. Returns bits per byte (0.0 = perfectly uniform,
/// 8.0 = maximum entropy / incompressible).
///
/// This is used for:
/// - Compression strategy selection (low entropy → aggressive compression)
/// - Data classification (text ~4-5 bits, compressed ~7.9+ bits)
/// - Storage cost estimation (entropy determines minimum encoding cost)
pub fn estimate_entropy(data: &[u8]) -> f64 {
    if data.is_empty() { return 0.0; }
    
    let mut freq = [0u64; 256];
    for &b in data {
        freq[b as usize] += 1;
    }
    
    let len = data.len() as f64;
    let mut entropy = 0.0f64;
    for &count in &freq {
        if count > 0 {
            let p = count as f64 / len;
            entropy -= p * p.log2();
        }
    }
    
    // Round to 4 decimal places
    (entropy * 10000.0).round() / 10000.0
}

/// Data Classification for Compression Strategy
///
/// Classifies input data to select optimal compression parameters.
/// Returns (class_name, estimated_compressibility) where compressibility
/// is a ratio estimate (e.g., 3.0 means ~3x compression expected).
pub fn classify_data(data: &[u8]) -> (&'static str, f64) {
    if data.is_empty() { return ("empty", 1.0); }
    
    let entropy = estimate_entropy(data);
    let is_utf8 = std::str::from_utf8(data).is_ok();
    
    // Check for high repetition (useful for BWT/RLE)
    let unique_bytes = {
        let mut seen = [false; 256];
        for &b in data {
            seen[b as usize] = true;
        }
        seen.iter().filter(|&&s| s).count()
    };
    
    let byte_ratio = unique_bytes as f64 / 256.0;
    
    if entropy < 1.0 {
        ("highly_repetitive", 8.0 / entropy.max(0.1))
    } else if entropy < 3.0 && is_utf8 {
        ("structured_text", 8.0 / entropy)
    } else if entropy < 4.5 && is_utf8 {
        ("natural_text", 8.0 / entropy)
    } else if entropy < 5.5 && is_utf8 {
        ("code_or_markup", 8.0 / entropy)
    } else if entropy > 7.8 && byte_ratio > 0.9 {
        ("pre_compressed", 1.02) // Already near Shannon limit
    } else if entropy > 7.0 {
        ("high_entropy_binary", 8.0 / entropy)
    } else if is_utf8 {
        ("mixed_text", 8.0 / entropy)
    } else {
        ("binary_data", 8.0 / entropy)
    }
}

// ========== Result types ==========

#[derive(Debug, Clone, Serialize)]
pub struct EncodeOutput {
    pub filename: String,
    pub original_size: usize,
    pub pre_compress_size: usize,
    pub post_compress_size: usize,
    pub original_checksum: String,
    pub original_data_checksum: String,
    pub compression_enabled: bool,
    pub compression_stats: Option<CompressionStats>,
    pub transcode: TranscodeInfo,
    pub fountain_stats: FountainStats,
    pub fasta_content: String,
    pub fasta_stats: FastaStats,
    pub num_oligos: usize,
    pub dna_sequence_preview: String,
    pub encode_time: f64,
    // New: integrated module outputs
    pub rs_stats: Option<RSStats>,
    pub constraint_report: Option<ConstraintReport>,
    pub oligo_quality: Option<OligoQualityReport>,
    pub oligo_build_stats: Option<OligoBuildStats>,
    pub cost_estimate: Option<CostEstimate>,
    // Entropy analysis
    pub entropy_bits_per_byte: f64,
    pub data_class: String,
    pub estimated_compressibility: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct TranscodeInfo {
    pub sequence_preview: String,
    pub sequence_length: usize,
    pub rotation_key: u8,
    pub original_length: usize,
    pub gc_content: f64,
    pub homopolymer_safe: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChaosOutput {
    pub chaos_stats: ChaosStats,
    pub mutation_summary: MutationSummary,
    pub dna_mutation_affects_decode: bool,
    pub original_sequence_preview: String,
    pub mutated_sequence_preview: String,
    pub droplet_survival_rate: f64,
    pub chaos_time: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct DecodeOutput {
    pub success: bool,
    pub data_match: bool,
    pub recovered_size: usize,
    pub recovered_data: Option<Vec<u8>>,
    pub recovered_preview: String,
    pub decode_stats: DecodeStats,
    pub decode_time: f64,
    pub decompression_stats: Option<DecompressionInfo>,
    pub rs_correction_stats: Option<RSStats>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DecompressionInfo {
    pub compressed_size: usize,
    pub decompressed_size: usize,
    pub expansion_ratio: f64,
}

/// Output from FASTA-based decode (standalone decode from uploaded FASTA)
#[derive(Debug, Clone, Serialize)]
pub struct FastaDecodeOutput {
    pub success: bool,
    pub data_match: bool,
    pub recovered_size: usize,
    pub recovered_data: Option<Vec<u8>>,
    pub recovered_preview: String,
    pub original_filename: String,
    pub original_checksum: String,
    pub actual_checksum: String,
    pub num_oligos_parsed: usize,
    pub crc_pass: usize,
    pub crc_fail: usize,
    pub decode_time: f64,
    pub decompression_stats: Option<DecompressionInfo>,
    pub rs_correction_stats: Option<RSStats>,
}

// ========== Pipeline ==========

pub struct HelixPipeline {
    pub config: PipelineConfig,
    transcoder: Transcoder,
    fountain: FountainCodec,
    chaos: ChaosMatrix,
    consensus: ConsensusDecoder,
    fasta: FastaIO,
    compressor: HelixCompressor,
    hypercompressor: HyperCompressor,
    rs_codec: ReedSolomonCodec,
    interleaved_rs: InterleavedRS,
    oligo_builder: OligoBuilder,
    constraints: DNAConstraints,

    // State
    pub last_encode: Option<EncodeState>,
    pub last_chaos: Option<ChaosState>,
    pub last_decode: Option<DecodeOutput>,
    dna_sequence_full: String,
    rs_enabled: bool,
    use_interleaved_rs: bool,
    use_hypercompress: bool,
}

/// Internal state kept for cross-step references
pub struct EncodeState {
    pub output: EncodeOutput,
    pub fountain_encoded: FountainEncoded,
    pub transcode_result: TranscodeResult,
    pub compression_enabled: bool,
    pub original_data_checksum: String,
    pub full_fasta_content: String,
    pub rs_enabled: bool,
    pub use_interleaved_rs: bool,
    pub use_hypercompress: bool,
}

pub struct ChaosState {
    pub output: ChaosOutput,
    pub surviving_droplets: Vec<Droplet>,
}

macro_rules! progress {
    ($cb:expr, $phase:expr, $pct:expr) => {
        if let Some(ref f) = $cb {
            f($phase, $pct);
        }
    };
}

impl HelixPipeline {
    pub fn new(config: PipelineConfig) -> Self {
        let fountain = FountainCodec::new(
            config.block_size,
            config.redundancy,
            config.seed,
        );
        let chaos = ChaosMatrix::new(
            config.deletion_rate,
            config.substitution_rate,
            config.insertion_rate,
            config.seed,
        );
        let compressor = HelixCompressor::new(&config.compression_level);

        let oligo_builder = OligoBuilder::new(config.oligo_length);

        Self {
            config,
            transcoder: Transcoder::new(),
            fountain,
            chaos,
            consensus: ConsensusDecoder::new(),
            fasta: FastaIO::new(),
            compressor,
            hypercompressor: HyperCompressor::new(),
            rs_codec: ReedSolomonCodec::default_commercial(), // RS(255,223)
            interleaved_rs: InterleavedRS::default_commercial(),
            oligo_builder,
            constraints: DNAConstraints::new(),
            last_encode: None,
            last_chaos: None,
            last_decode: None,
            dna_sequence_full: String::new(),
            rs_enabled: true,
            use_interleaved_rs: true,
            use_hypercompress: true,
        }
    }

    pub fn encode(
        &mut self,
        data: &[u8],
        filename: &str,
        progress_cb: Option<&dyn Fn(&str, u32)>,
    ) -> EncodeOutput {
        let start = Instant::now();
        progress!(progress_cb, &format!("Starting encode ({} bytes)...", data.len()), 2);

        let pre_compress = data.len();
        let orig_data_checksum = hex_sha256(data);

        // Stage 0: Entropy analysis & data classification
        let entropy = estimate_entropy(data);
        let (data_class_name, est_compressibility) = classify_data(data);
        progress!(progress_cb, &format!("Entropy: {:.2} bits/byte, class={}, est. {:.1}x compressible",
            entropy, data_class_name, est_compressibility), 3);

        // Stage 1: Compress (HyperCompress v2 or legacy)
        let (compressed_data, compression_stats) = if self.config.compression {
            progress!(progress_cb, "HyperCompress: Entropy analysis + parallel compression...", 5);
            if self.use_hypercompress {
                let sub_cb = |phase: &str, pct: u32| {
                    progress!(progress_cb, &format!("HyperCompress: {}", phase), 5 + (pct * 15 / 100));
                };
                let (compressed, hstats) = self.hypercompressor.compress(data, Some(&sub_cb));
                let ratio_msg = if hstats.compression_ratio > 1.01 {
                    format!("HyperCompressed: {}B → {}B ({:.1}% saved, {} won, {} class)",
                        pre_compress, compressed.len(), hstats.space_saving_percent,
                        hstats.method, hstats.data_class)
                } else {
                    format!("HyperCompress: {}B → {}B (already optimal)", pre_compress, compressed.len())
                };
                progress!(progress_cb, &ratio_msg, 20);
                // Convert HyperCompressStats → CompressionStats for backwards compat
                let compat_stats = CompressionStats {
                    original_size: hstats.original_size,
                    compressed_size: hstats.compressed_size,
                    method: hstats.method.clone(),
                    compression_ratio: hstats.compression_ratio,
                    space_saving_percent: hstats.space_saving_percent,
                    throughput_mbps: hstats.throughput_mbps,
                    time_seconds: hstats.time_seconds,
                    checksum: hstats.checksum.clone(),
                    saved_bytes: hstats.saved_bytes,
                    content_type_detected: hstats.content_type_detected.clone(),
                    compression_note: hstats.compression_note.clone(),
                    dedup_savings: hstats.dedup_savings,
                    dedup_unique_blocks: hstats.dedup_unique_blocks,
                    dedup_total_blocks: hstats.dedup_total_blocks,
                    stages: hstats.stages.iter().map(|s| crate::compressor::StageInfo {
                        name: s.name.clone(), output_size: s.output_size,
                    }).collect(),
                    all_methods_tried: hstats.all_methods_tried.iter().map(|m| crate::compressor::MethodResult {
                        method: m.method.clone(), size: m.size,
                    }).collect(),
                };
                (compressed, Some(compat_stats))
            } else {
                let sub_cb = |phase: &str, pct: u32| {
                    progress!(progress_cb, &format!("Compress: {}", phase), 5 + (pct * 15 / 100));
                };
                let (compressed, stats) = self.compressor.compress(data, Some(&sub_cb));
                let ratio_msg = if stats.compression_ratio > 1.01 {
                    format!("Compressed: {}B → {}B ({:.1}% saved, {} won)",
                        pre_compress, compressed.len(), stats.space_saving_percent, stats.method)
                } else {
                    format!("Compressed: {}B → {}B (already optimal)", pre_compress, compressed.len())
                };
                progress!(progress_cb, &ratio_msg, 20);
                (compressed, Some(stats))
            }
        } else {
            (data.to_vec(), None)
        };

        let post_compress = compressed_data.len();

        // Stage 2: Reed-Solomon error correction encoding
        let (pipeline_data, rs_stats) = if self.rs_enabled {
            if self.use_interleaved_rs {
                progress!(progress_cb, &format!("Interleaved RS encoding {} bytes (cross-oligo protection)...", compressed_data.len()), 22);
                let (rs_encoded, istats) = self.interleaved_rs.encode_buffer(&compressed_data);
                progress!(progress_cb, &format!("Interleaved RS: {} → {} bytes ({} groups, {}×{} blocks, {:.1}% overhead, corrects {} oligo losses/group)",
                    compressed_data.len(), rs_encoded.len(), istats.num_groups,
                    istats.interleave_depth, istats.num_groups,
                    istats.overhead_percent, istats.max_oligos_recoverable), 28);
                (rs_encoded, Some(istats.to_rs_stats()))
            } else {
                progress!(progress_cb, &format!("Reed-Solomon RS(255,223) encoding {} bytes...", compressed_data.len()), 22);
                let (rs_encoded, stats) = self.rs_codec.encode_buffer(&compressed_data);
                progress!(progress_cb, &format!("RS encoded: {} → {} bytes ({} blocks, {:.1}% overhead)",
                    compressed_data.len(), rs_encoded.len(), stats.blocks_encoded, stats.overhead_percent), 28);
                (rs_encoded, Some(stats))
            }
        } else {
            (compressed_data.clone(), None)
        };

        let pipeline_checksum = hex_sha256(&pipeline_data);

        // Stage 3: Fountain codes (operates on binary RS-protected data)
        progress!(progress_cb, "Generating hybrid systematic/LT Fountain codes (Robust Soliton)...", 30);
        let fountain_encoded = self.fountain.encode(&pipeline_data);
        let fountain_stats = self.fountain.get_stats(&fountain_encoded);
        progress!(progress_cb, &format!("Fountain: {} droplets from {} blocks ({:.1}× redundancy)",
            fountain_stats.num_droplets, fountain_stats.num_blocks, fountain_stats.redundancy_ratio), 40);

        // Stage 4: Transcode to DNA
        progress!(progress_cb, &format!("Transcoding {} bytes to DNA bases...", pipeline_data.len()), 42);
        let transcode = self.transcoder.encode(&pipeline_data);
        self.dna_sequence_full = transcode.sequence.clone();
        progress!(progress_cb, &format!("Transcoded: {} bases, GC={:.1}%, key={}",
            transcode.length, transcode.gc_content * 100.0, transcode.rotation_key), 50);

        // Stage 5: Build structured oligos with primers, index, CRC
        progress!(progress_cb, "Building structured oligos (primers + index + CRC)...", 52);
        let (structured_oligos, oligo_build_stats) = self.oligo_builder.build_oligos(&transcode.sequence);
        let oligo_quality = self.oligo_builder.quality_report(&structured_oligos);
        progress!(progress_cb, &format!("Built {} structured oligos ({}bp each, {:.0}% payload efficiency)",
            oligo_build_stats.num_oligos, oligo_build_stats.oligo_total_length,
            oligo_quality.payload_efficiency * 100.0), 58);

        // Stage 6: DNA constraint validation
        progress!(progress_cb, "Validating DNA constraints (GC, homopolymers, restriction sites)...", 60);
        let oligo_seqs: Vec<&str> = structured_oligos.iter()
            .map(|o| o.full_sequence.as_str())
            .collect();
        let constraint_report = self.constraints.check_oligos(&oligo_seqs);
        progress!(progress_cb, &format!("Constraints: {}/{} oligos pass, synthesis readiness {:.0}%",
            constraint_report.passing_oligos, constraint_report.total_oligos,
            constraint_report.synthesis_readiness_score * 100.0), 68);

        // Stage 7: FASTA output (from structured oligos — single consistent oligo set)
        progress!(progress_cb, "Generating FASTA output from structured oligos...", 70);
        let fasta_records: Vec<crate::fasta::FastaRecord> = structured_oligos.iter().map(|o| {
            crate::fasta::FastaRecord {
                id: format!("HELIX_{:06}|len={}|idx={}", o.index + 1, o.full_sequence.len(), o.index),
                sequence: o.full_sequence.clone(),
            }
        }).collect();
        // Generate FASTA with embedded metadata for standalone decode
        let fasta_metadata = FastaMetadata {
            rotation_key: transcode.rotation_key,
            original_length: transcode.original_length,
            rs_enabled: self.rs_enabled,
            use_interleaved_rs: self.use_interleaved_rs,
            compression_enabled: self.config.compression,
            use_hypercompress: self.use_hypercompress,
            original_filename: filename.to_string(),
            original_checksum: orig_data_checksum.clone(),
            block_size: self.config.block_size,
            redundancy: self.config.redundancy,
            seed: self.config.seed,
        };
        let fasta_content = self.fasta.generate_fasta_with_metadata(&fasta_records, &fasta_metadata);
        let fasta_stats = self.fasta.get_stats(&fasta_records);
        progress!(progress_cb, &format!("FASTA: {} oligos, {} total bases",
            fasta_records.len(), fasta_stats.total_bases), 78);

        // Stage 8: Cost estimation (uses structured oligo counts for consistency)
        progress!(progress_cb, "Estimating synthesis and sequencing costs...", 80);
        let cost_estimate = CostEstimator::estimate(
            pre_compress,
            structured_oligos.len(),
            fasta_stats.total_bases,
            self.config.oligo_length,
            fountain_stats.redundancy_ratio,
        );
        progress!(progress_cb, &format!("Cost: ${:.2} total ({}, ${:.2}/MB)",
            cost_estimate.total_cost_usd, cost_estimate.recommended_vendor,
            cost_estimate.cost_per_mb_stored), 88);

        progress!(progress_cb, "Computing analytics and finalizing...", 92);

        let elapsed = start.elapsed().as_secs_f64();
        let seq_preview = if transcode.sequence.len() > 500 {
            transcode.sequence[..500].to_string()
        } else {
            transcode.sequence.clone()
        };

        let full_fasta = fasta_content.clone();
        let fasta_truncated = if fasta_content.len() > 8000 {
            fasta_content[..8000].to_string()
        } else {
            fasta_content
        };

        let output = EncodeOutput {
            filename: filename.to_string(),
            original_size: pre_compress,
            pre_compress_size: pre_compress,
            post_compress_size: post_compress,
            original_checksum: pipeline_checksum.clone(),
            original_data_checksum: orig_data_checksum.clone(),
            compression_enabled: self.config.compression,
            compression_stats,
            transcode: TranscodeInfo {
                sequence_preview: seq_preview.clone(),
                sequence_length: transcode.length,
                rotation_key: transcode.rotation_key,
                original_length: transcode.original_length,
                gc_content: transcode.gc_content,
                homopolymer_safe: transcode.homopolymer_safe,
            },
            fountain_stats,
            fasta_content: fasta_truncated,
            fasta_stats,
            num_oligos: structured_oligos.len(),
            dna_sequence_preview: seq_preview,
            encode_time: (elapsed * 10000.0).round() / 10000.0,
            rs_stats,
            constraint_report: Some(constraint_report),
            oligo_quality: Some(oligo_quality),
            oligo_build_stats: Some(oligo_build_stats),
            cost_estimate: Some(cost_estimate),
            entropy_bits_per_byte: entropy,
            data_class: data_class_name.to_string(),
            estimated_compressibility: est_compressibility,
        };

        self.last_encode = Some(EncodeState {
            output: output.clone(),
            fountain_encoded,
            transcode_result: transcode,
            compression_enabled: self.config.compression,
            original_data_checksum: orig_data_checksum,
            full_fasta_content: full_fasta,
            rs_enabled: self.rs_enabled,
            use_interleaved_rs: self.use_interleaved_rs,
            use_hypercompress: self.use_hypercompress,
        });
        self.last_chaos = None;
        self.last_decode = None;

        progress!(progress_cb, "Complete", 100);
        output
    }

    pub fn apply_chaos(
        &mut self,
        loss_rate: f64,
        deletion_rate: Option<f64>,
        substitution_rate: Option<f64>,
        insertion_rate: Option<f64>,
        progress_cb: Option<&dyn Fn(&str, u32)>,
    ) -> Result<ChaosOutput, String> {
        let encode_state = self
            .last_encode
            .as_ref()
            .ok_or("No encoded data. Run encode first.")?;

        let start = Instant::now();
        let total_drops = encode_state.fountain_encoded.droplets.len();
        progress!(progress_cb, &format!("Chaos: targeting {} droplets with {:.0}% loss...",
            total_drops, loss_rate * 100.0), 10);

        self.chaos.set_rates(deletion_rate, substitution_rate, insertion_rate);

        // Droplet loss
        progress!(progress_cb, &format!("Destroying droplets ({:.0}% loss probability)...", loss_rate * 100.0), 30);
        let (survived, chaos_stats) = self.chaos.mutate_droplets(
            &encode_state.fountain_encoded.droplets,
            loss_rate,
        );
        progress!(progress_cb, &format!("Destroyed {} of {} droplets ({} surviving)",
            chaos_stats.lost_droplets, chaos_stats.total_droplets, chaos_stats.surviving_droplets), 50);

        // Sequence mutations
        progress!(progress_cb, &format!("Mutating DNA: del={:.0}%, sub={:.0}%, ins={:.0}%...",
            self.chaos.deletion_rate * 100.0, self.chaos.substitution_rate * 100.0,
            self.chaos.insertion_rate * 100.0), 60);
        let orig_preview = if self.dna_sequence_full.len() > 500 {
            self.dna_sequence_full[..500].to_string()
        } else {
            self.dna_sequence_full.clone()
        };
        // Apply chaos mutations to the full DNA sequence so degradation
        // statistics accurately reflect the complete oligo pool
        let (mutated, mutation_summary) =
            self.chaos.mutate_sequence(&self.dna_sequence_full);
        let mutated_preview = if mutated.len() > 500 {
            mutated[..500].to_string()
        } else {
            mutated.clone()
        };

        progress!(progress_cb, &format!("Simulated {} sequence mutations for preview ({} subs, {} dels, {} ins)",
            mutation_summary.total_mutations, mutation_summary.substitutions,
            mutation_summary.deletions, mutation_summary.insertions), 90);
        progress!(progress_cb, "Complete", 100);
        let elapsed = start.elapsed().as_secs_f64();

        let droplet_survival_rate = if encode_state.fountain_encoded.droplets.is_empty() {
            0.0
        } else {
            survived.len() as f64 / encode_state.fountain_encoded.droplets.len() as f64
        };

        let output = ChaosOutput {
            chaos_stats,
            mutation_summary,
            dna_mutation_affects_decode: false,
            original_sequence_preview: orig_preview,
            mutated_sequence_preview: mutated_preview,
            droplet_survival_rate: (droplet_survival_rate * 10000.0).round() / 10000.0,
            chaos_time: (elapsed * 10000.0).round() / 10000.0,
        };

        self.last_chaos = Some(ChaosState {
            output: output.clone(),
            surviving_droplets: survived,
        });

        Ok(output)
    }

    pub fn decode(
        &mut self,
        progress_cb: Option<&dyn Fn(&str, u32)>,
    ) -> Result<DecodeOutput, String> {
        let encode_state = self
            .last_encode
            .as_ref()
            .ok_or("No encoded data. Run encode first.")?;

        let start = Instant::now();
        let surviving = if let Some(ref chaos) = self.last_chaos {
            &chaos.surviving_droplets
        } else {
            &encode_state.fountain_encoded.droplets
        };
        progress!(progress_cb, &format!("Starting decode ({} surviving droplets, {} blocks)...",
            surviving.len(), encode_state.fountain_encoded.num_blocks), 5);

        // Stage 1: Fountain decode
        progress!(progress_cb, &format!("Peeling decoder: solving {} droplets → {} blocks...",
            surviving.len(), encode_state.fountain_encoded.num_blocks), 15);
        let recovered_opt = self.consensus.decode_pipeline(
            &encode_state.fountain_encoded,
            surviving,
            encode_state.transcode_result.rotation_key,
            encode_state.transcode_result.original_length,
        );

        let mut recovered_data = match recovered_opt {
            Some(d) => {
                progress!(progress_cb, &format!("Fountain decode OK: recovered {} bytes from {} droplets",
                    d.len(), surviving.len()), 35);
                d
            }
            None => {
                let output = DecodeOutput {
                    success: false,
                    data_match: false,
                    recovered_size: 0,
                    recovered_data: None,
                    recovered_preview: String::new(),
                    decode_stats: self.consensus.get_stats(),
                    decode_time: start.elapsed().as_secs_f64(),
                    decompression_stats: None,
                    rs_correction_stats: None,
                };
                self.last_decode = Some(output.clone());
                return Ok(output);
            }
        };

        // Stage 2: Reed-Solomon error correction (if enabled during encode)
        let mut rs_correction_stats = None;
        if encode_state.rs_enabled {
            progress!(progress_cb, &format!("Reed-Solomon decoding {} bytes (correcting errors)...", recovered_data.len()), 42);

            // BUG FIX: Use encode_state flags, not self flags.
            // If config changes between encode and decode, self.use_interleaved_rs
            // could differ from what was used during encoding, causing decode failure.
            let rs_result = if encode_state.use_interleaved_rs {
                self.interleaved_rs.decode_buffer(&recovered_data)
                    .map(|(d, s)| (d, s.to_rs_stats()))
            } else {
                self.rs_codec.decode_buffer(&recovered_data)
            };

            match rs_result {
                Some((decoded, stats)) => {
                    if stats.total_errors_corrected > 0 {
                        progress!(progress_cb, &format!("RS corrected {} errors across {} blocks",
                            stats.total_errors_corrected, stats.blocks_corrected), 50);
                    } else {
                        progress!(progress_cb, "RS decode clean (no errors to correct)", 50);
                    }
                    rs_correction_stats = Some(stats);
                    recovered_data = decoded;
                }
                None => {
                    progress!(progress_cb, "RS decode FAILED — too many errors to correct", 50);
                    let output = DecodeOutput {
                        success: false,
                        data_match: false,
                        recovered_size: 0,
                        recovered_data: None,
                        recovered_preview: String::new(),
                        decode_stats: self.consensus.get_stats(),
                        decode_time: start.elapsed().as_secs_f64(),
                        decompression_stats: None,
                        rs_correction_stats: None,
                    };
                    self.last_decode = Some(output.clone());
                    return Ok(output);
                }
            }
        }

        // Stage 3: Decompress (HyperCompress v2 or legacy)
        progress!(progress_cb, &format!("Decompressing {} bytes...", recovered_data.len()), 55);
        let mut decompression_stats = None;
        if encode_state.compression_enabled {
            // BUG FIX: Use encode_state flags to match the compressor used during encode.
            let decompress_result = if encode_state.use_hypercompress {
                self.hypercompressor.decompress(&recovered_data)
            } else {
                self.compressor.decompress(&recovered_data)
            };
            match decompress_result {
                Ok(decompressed) => {
                    progress!(progress_cb, &format!("Decompressed: {} → {} bytes ({:.1}× expansion)",
                        recovered_data.len(), decompressed.len(),
                        decompressed.len() as f64 / recovered_data.len().max(1) as f64), 70);
                    decompression_stats = Some(DecompressionInfo {
                        compressed_size: recovered_data.len(),
                        decompressed_size: decompressed.len(),
                        expansion_ratio: (decompressed.len() as f64
                            / recovered_data.len().max(1) as f64
                            * 100.0)
                            .round()
                            / 100.0,
                    });
                    recovered_data = decompressed;
                }
                Err(e) => {
                    progress!(progress_cb, &format!("Decompression FAILED: {}", e), 70);
                    let output = DecodeOutput {
                        success: false,
                        data_match: false,
                        recovered_size: 0,
                        recovered_data: None,
                        recovered_preview: String::new(),
                        decode_stats: self.consensus.get_stats(),
                        decode_time: start.elapsed().as_secs_f64(),
                        decompression_stats: None,
                        rs_correction_stats: None,
                    };
                    self.last_decode = Some(output.clone());
                    return Ok(output);
                }
            }
        }

        // Stage 4: Checksum verify
        progress!(progress_cb, &format!("Verifying SHA-256 checksum ({} bytes)...", recovered_data.len()), 80);
        let actual_hash = hex_sha256(&recovered_data);
        let expected_hash = &encode_state.original_data_checksum;
        let data_match = actual_hash == *expected_hash;
        if data_match {
            progress!(progress_cb, &format!("✓ Checksum MATCH: {}", &actual_hash[..16]), 95);
        } else {
            progress!(progress_cb, &format!("✗ Checksum MISMATCH: expected {}... got {}...",
                &expected_hash[..16], &actual_hash[..16]), 95);
        }

        progress!(progress_cb, "Complete", 100);
        let elapsed = start.elapsed().as_secs_f64();

        let preview = String::from_utf8_lossy(
            &recovered_data[..recovered_data.len().min(200)],
        )
        .to_string();

        let output = DecodeOutput {
            success: data_match,
            data_match,
            recovered_size: recovered_data.len(),
            recovered_data: Some(recovered_data),
            recovered_preview: preview,
            decode_stats: self.consensus.get_stats(),
            decode_time: (elapsed * 10000.0).round() / 10000.0,
            decompression_stats,
            rs_correction_stats,
        };

        self.last_decode = Some(output.clone());
        Ok(output)
    }

    /// Decode data from a standalone FASTA file (uploaded by user).
    /// This reverses the full pipeline: FASTA → strip oligos → reassemble DNA → reverse transcode → RS decode → decompress
    pub fn decode_from_fasta(
        &mut self,
        fasta_content: &str,
        progress_cb: Option<&dyn Fn(&str, u32)>,
    ) -> Result<FastaDecodeOutput, String> {
        let start = Instant::now();
        progress!(progress_cb, "Parsing FASTA file...", 5);

        // Parse FASTA content
        let (records, metadata) = FastaIO::parse_fasta(fasta_content);
        if records.is_empty() {
            return Err("No valid FASTA records found".to_string());
        }

        let meta = metadata.ok_or_else(|| {
            "Missing Helix-Core metadata in FASTA file. This FASTA was not generated by Helix-Core.".to_string()
        })?;

        progress!(progress_cb, &format!("Found {} oligos, metadata: rotation_key={}, file={}",
            records.len(), meta.rotation_key, meta.original_filename), 10);

        // Disassemble oligos: strip primers, extract index+payload, verify CRC
        progress!(progress_cb, "Disassembling oligos (stripping primers, verifying CRC)...", 15);
        let raw_seqs: Vec<String> = records.iter().map(|r| r.sequence.clone()).collect();
        let (payloads, crc_pass, crc_fail) = self.oligo_builder.disassemble_oligos(&raw_seqs)
            .map_err(|e| format!("Oligo disassembly failed: {}", e))?;

        progress!(progress_cb, &format!("Disassembled {} oligos: {} CRC pass, {} CRC fail",
            payloads.len(), crc_pass, crc_fail), 25);

        // Reassemble full DNA sequence from payloads (sorted by index)
        progress!(progress_cb, "Reassembling DNA sequence from oligo payloads...", 30);
        let full_dna: String = payloads.concat();
        progress!(progress_cb, &format!("Reassembled {} DNA bases", full_dna.len()), 35);

        // Reverse transcode: DNA → bytes
        progress!(progress_cb, &format!("Reverse transcoding ({} bases, rotation_key={})",
            full_dna.len(), meta.rotation_key), 40);
        let binary_data = self.transcoder.decode(&full_dna, meta.rotation_key, meta.original_length);
        progress!(progress_cb, &format!("Reverse transcoded: {} bytes", binary_data.len()), 50);

        // RS decode (if enabled)
        let mut recovered_data = binary_data;
        let mut rs_stats = None;
        if meta.rs_enabled {
            progress!(progress_cb, &format!("Reed-Solomon decoding {} bytes...", recovered_data.len()), 55);
            let rs_result = if meta.use_interleaved_rs {
                self.interleaved_rs.decode_buffer(&recovered_data)
                    .map(|(d, s)| (d, s.to_rs_stats()))
            } else {
                self.rs_codec.decode_buffer(&recovered_data)
            };

            match rs_result {
                Some((decoded, stats)) => {
                    if stats.total_errors_corrected > 0 {
                        progress!(progress_cb, &format!("RS corrected {} errors", stats.total_errors_corrected), 65);
                    } else {
                        progress!(progress_cb, "RS decode clean (no errors)", 65);
                    }
                    rs_stats = Some(stats);
                    recovered_data = decoded;
                }
                None => {
                    return Err("Reed-Solomon decode failed — too many errors".to_string());
                }
            }
        }

        // Decompress
        let mut decompression_info = None;
        if meta.compression_enabled {
            progress!(progress_cb, &format!("Decompressing {} bytes...", recovered_data.len()), 70);
            let decompress_result = if meta.use_hypercompress {
                self.hypercompressor.decompress(&recovered_data)
            } else {
                self.compressor.decompress(&recovered_data)
            };
            match decompress_result {
                Ok(decompressed) => {
                    progress!(progress_cb, &format!("Decompressed: {} → {} bytes",
                        recovered_data.len(), decompressed.len()), 80);
                    decompression_info = Some(DecompressionInfo {
                        compressed_size: recovered_data.len(),
                        decompressed_size: decompressed.len(),
                        expansion_ratio: (decompressed.len() as f64 / recovered_data.len().max(1) as f64 * 100.0).round() / 100.0,
                    });
                    recovered_data = decompressed;
                }
                Err(e) => {
                    return Err(format!("Decompression failed: {}", e));
                }
            }
        }

        // Checksum verify
        progress!(progress_cb, "Verifying SHA-256 checksum...", 85);
        let actual_checksum = hex_sha256(&recovered_data);
        let data_match = actual_checksum == meta.original_checksum;
        if data_match {
            progress!(progress_cb, &format!("✓ Checksum MATCH: {}", &actual_checksum[..16.min(actual_checksum.len())]), 95);
        } else {
            progress!(progress_cb, &format!("✗ Checksum MISMATCH: expected {}... got {}...",
                &meta.original_checksum[..16.min(meta.original_checksum.len())],
                &actual_checksum[..16.min(actual_checksum.len())]), 95);
        }

        progress!(progress_cb, "Complete", 100);
        let elapsed = start.elapsed().as_secs_f64();

        let preview = String::from_utf8_lossy(
            &recovered_data[..recovered_data.len().min(500)],
        ).to_string();

        Ok(FastaDecodeOutput {
            success: data_match,
            data_match,
            recovered_size: recovered_data.len(),
            recovered_data: Some(recovered_data),
            recovered_preview: preview,
            original_filename: meta.original_filename,
            original_checksum: meta.original_checksum,
            actual_checksum,
            num_oligos_parsed: records.len(),
            crc_pass,
            crc_fail,
            decode_time: (elapsed * 10000.0).round() / 10000.0,
            decompression_stats: decompression_info,
            rs_correction_stats: rs_stats,
        })
    }

    pub fn get_config_json(&self) -> serde_json::Value {
        serde_json::json!({
            "block_size": self.config.block_size,
            "redundancy": self.config.redundancy,
            "seed": self.config.seed,
            "deletion_rate": self.config.deletion_rate,
            "substitution_rate": self.config.substitution_rate,
            "insertion_rate": self.config.insertion_rate,
        })
    }

    pub fn update_config(&mut self, updates: &serde_json::Value) {
        if let Some(r) = updates.get("redundancy").and_then(|v| v.as_f64()) {
            if r.is_finite() && r > 0.0 {
                self.config.redundancy = r;
                self.fountain = FountainCodec::new(
                    self.config.block_size,
                    r,
                    self.config.seed,
                );
            }
        }
        if let Some(d) = updates.get("deletion_rate").and_then(|v| v.as_f64())
        {
            if (0.0..=1.0).contains(&d) {
                self.config.deletion_rate = d;
                self.chaos.deletion_rate = d;
            }
        }
        if let Some(s) =
            updates.get("substitution_rate").and_then(|v| v.as_f64())
        {
            if (0.0..=1.0).contains(&s) {
                self.config.substitution_rate = s;
                self.chaos.substitution_rate = s;
            }
        }
        if let Some(i) =
            updates.get("insertion_rate").and_then(|v| v.as_f64())
        {
            if (0.0..=1.0).contains(&i) {
                self.config.insertion_rate = i;
                self.chaos.insertion_rate = i;
            }
        }
    }

    /// Full sequence for analytics computation
    pub fn get_full_dna_sequence(&self) -> &str {
        &self.dna_sequence_full
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_full_pipeline() {
        let config = PipelineConfig::default();
        let mut pipeline = HelixPipeline::new(config);

        let data =
            b"Project Helix-Core Rust Edition: DNA data storage works!";
        let enc = pipeline.encode(data, "test.txt", None);
        assert!(enc.encode_time > 0.0);
        assert!(enc.compression_enabled);

        let chaos = pipeline.apply_chaos(0.20, None, None, None, None);
        assert!(chaos.is_ok());

        let dec = pipeline.decode(None);
        assert!(dec.is_ok());
        let dec = dec.unwrap();
        assert!(dec.success);
        assert!(dec.data_match);
        assert_eq!(dec.recovered_data.unwrap(), data);
    }

    #[test]
    fn test_roundtrip_no_chaos() {
        let config = PipelineConfig::default();
        let mut pipeline = HelixPipeline::new(config);

        let data: Vec<u8> = (0..1024).map(|i| (i % 256) as u8).collect();
        pipeline.encode(&data, "binary.bin", None);

        let dec = pipeline.decode(None).unwrap();
        assert!(dec.data_match, "Data mismatch without chaos!");
    }

    #[test]
    fn test_large_text_compression() {
        let config = PipelineConfig::default();
        let mut pipeline = HelixPipeline::new(config);

        let data = "Rust Helix-Core compression engine! ".repeat(5000);
        let enc = pipeline.encode(data.as_bytes(), "big.txt", None);

        if let Some(ref cs) = enc.compression_stats {
            println!(
                "Compression: {} -> {} ({:.0}x, {:.1}% saved)",
                cs.original_size,
                cs.compressed_size,
                cs.compression_ratio,
                cs.space_saving_percent
            );
            assert!(
                cs.compression_ratio > 10.0,
                "Expected >10x compression on repetitive text"
            );
        }

        let dec = pipeline.decode(None).unwrap();
        assert!(dec.data_match);
    }

    // ========== FASTA Decode Roundtrip Tests ==========

    #[test]
    fn test_fasta_roundtrip_text() {
        let config = PipelineConfig::default();
        let mut pipeline = HelixPipeline::new(config);

        let data = b"Hello, DNA storage world! Testing FASTA roundtrip with text data.";
        pipeline.encode(data, "hello.txt", None);

        // Get the FASTA content
        let fasta_content = pipeline.last_encode.as_ref().unwrap().full_fasta_content.clone();

        // Decode from FASTA on a fresh pipeline
        let mut decoder = HelixPipeline::new(PipelineConfig::default());
        let dec = decoder.decode_from_fasta(&fasta_content, None).unwrap();
        assert!(dec.success, "FASTA roundtrip should succeed");
        assert!(dec.data_match, "FASTA roundtrip data should match");
        assert_eq!(dec.recovered_data.unwrap(), data);
    }

    #[test]
    fn test_fasta_roundtrip_binary() {
        let config = PipelineConfig::default();
        let mut pipeline = HelixPipeline::new(config);

        let data: Vec<u8> = (0..2048).map(|i| ((i * 7 + 13) % 256) as u8).collect();
        pipeline.encode(&data, "binary.bin", None);

        let fasta = pipeline.last_encode.as_ref().unwrap().full_fasta_content.clone();
        let mut decoder = HelixPipeline::new(PipelineConfig::default());
        let dec = decoder.decode_from_fasta(&fasta, None).unwrap();
        assert!(dec.data_match, "FASTA binary roundtrip should match");
        assert_eq!(dec.recovered_data.unwrap(), data);
    }

    #[test]
    fn test_fasta_roundtrip_csv() {
        let config = PipelineConfig::default();
        let mut pipeline = HelixPipeline::new(config);

        let data = "id,name,value\n1,alpha,42.5\n2,beta,13.7\n3,gamma,99.1\n".repeat(100);
        pipeline.encode(data.as_bytes(), "data.csv", None);

        let fasta = pipeline.last_encode.as_ref().unwrap().full_fasta_content.clone();
        let mut decoder = HelixPipeline::new(PipelineConfig::default());
        let dec = decoder.decode_from_fasta(&fasta, None).unwrap();
        assert!(dec.data_match, "FASTA CSV roundtrip should match");
        assert_eq!(dec.original_filename, "data.csv");
    }

    #[test]
    fn test_fasta_roundtrip_sql() {
        let config = PipelineConfig::default();
        let mut pipeline = HelixPipeline::new(config);

        let data = "CREATE TABLE users (id INT PRIMARY KEY, name VARCHAR(255), email VARCHAR(255));\n\
                    INSERT INTO users VALUES (1, 'Alice', 'alice@example.com');\n\
                    INSERT INTO users VALUES (2, 'Bob', 'bob@example.com');\n\
                    SELECT * FROM users WHERE id > 0 ORDER BY name;\n".repeat(50);
        pipeline.encode(data.as_bytes(), "schema.sql", None);

        let fasta = pipeline.last_encode.as_ref().unwrap().full_fasta_content.clone();
        let mut decoder = HelixPipeline::new(PipelineConfig::default());
        let dec = decoder.decode_from_fasta(&fasta, None).unwrap();
        assert!(dec.data_match, "FASTA SQL roundtrip should match");
    }

    #[test]
    fn test_fasta_roundtrip_json() {
        let config = PipelineConfig::default();
        let mut pipeline = HelixPipeline::new(config);

        let data = r#"{"users":[{"id":1,"name":"Alice","roles":["admin","user"]},{"id":2,"name":"Bob","roles":["user"]}],"meta":{"total":2,"page":1}}"#.repeat(50);
        pipeline.encode(data.as_bytes(), "api_response.json", None);

        let fasta = pipeline.last_encode.as_ref().unwrap().full_fasta_content.clone();
        let mut decoder = HelixPipeline::new(PipelineConfig::default());
        let dec = decoder.decode_from_fasta(&fasta, None).unwrap();
        assert!(dec.data_match, "FASTA JSON roundtrip should match");
    }

    #[test]
    fn test_fasta_roundtrip_all_bytes() {
        // Test that every possible byte value roundtrips correctly
        let config = PipelineConfig::default();
        let mut pipeline = HelixPipeline::new(config);

        let data: Vec<u8> = (0..=255).collect();
        pipeline.encode(&data, "all_bytes.bin", None);

        let fasta = pipeline.last_encode.as_ref().unwrap().full_fasta_content.clone();
        let mut decoder = HelixPipeline::new(PipelineConfig::default());
        let dec = decoder.decode_from_fasta(&fasta, None).unwrap();
        assert!(dec.data_match, "All 256 byte values should roundtrip");
        assert_eq!(dec.recovered_data.unwrap(), data);
    }

    #[test]
    fn test_fasta_roundtrip_large_repetitive() {
        let config = PipelineConfig::default();
        let mut pipeline = HelixPipeline::new(config);

        let data = vec![0xAA_u8; 50_000];
        pipeline.encode(&data, "repetitive.bin", None);

        let fasta = pipeline.last_encode.as_ref().unwrap().full_fasta_content.clone();
        let mut decoder = HelixPipeline::new(PipelineConfig::default());
        let dec = decoder.decode_from_fasta(&fasta, None).unwrap();
        assert!(dec.data_match, "Large repetitive data should roundtrip");
        assert_eq!(dec.recovered_size, 50_000);
    }

    // ========== Benchmark-style tests with timing ==========

    #[test]
    fn test_benchmark_multiple_types() {
        let test_cases: Vec<(&str, &str, Vec<u8>)> = vec![
            ("Text 1KB", "text.txt", "The quick brown fox. ".repeat(50).into_bytes()),
            ("CSV 5KB", "data.csv", "id,name,val\n1,a,0.5\n2,b,0.7\n".repeat(180).into_bytes()),
            ("SQL", "queries.sql", "SELECT * FROM t WHERE x > 0;\n".repeat(100).into_bytes()),
            ("JSON", "data.json", r#"{"k":"v","n":42}"#.repeat(200).into_bytes()),
            ("Binary", "data.bin", (0..5000).map(|i| ((i * 13 + 7) % 256) as u8).collect()),
        ];

        for (name, filename, data) in &test_cases {
            let config = PipelineConfig::default();
            let mut pipeline = HelixPipeline::new(config);

            let start = std::time::Instant::now();
            let enc = pipeline.encode(data, filename, None);
            let encode_time = start.elapsed();

            let start = std::time::Instant::now();
            let dec = pipeline.decode(None).unwrap();
            let decode_time = start.elapsed();

            println!("{}: encode={:.1}ms, decode={:.1}ms, ratio={:.1}x, oligos={}, match={}",
                name,
                encode_time.as_secs_f64() * 1000.0,
                decode_time.as_secs_f64() * 1000.0,
                enc.compression_stats.as_ref().map(|s| s.compression_ratio).unwrap_or(1.0),
                enc.num_oligos,
                dec.data_match);

            assert!(dec.data_match, "{} should roundtrip correctly", name);
        }
    }

    // ========== Entropy & Data Classification Tests ==========

    #[test]
    fn test_entropy_uniform() {
        // All same bytes → zero entropy
        let data = vec![0u8; 1000];
        let e = estimate_entropy(&data);
        assert!(e < 0.01, "Uniform data should have ~0 entropy, got {}", e);
    }

    #[test]
    fn test_entropy_max() {
        // All 256 byte values appearing equally → 8.0 bits
        let data: Vec<u8> = (0..=255).cycle().take(256 * 100).collect();
        let e = estimate_entropy(&data);
        assert!((e - 8.0).abs() < 0.01, "Max entropy data should be ~8.0, got {}", e);
    }

    #[test]
    fn test_entropy_text() {
        let data = "The quick brown fox jumps over the lazy dog. ".repeat(100);
        let e = estimate_entropy(data.as_bytes());
        assert!(e > 3.5 && e < 5.5, "English text entropy should be ~4-5 bits, got {}", e);
    }

    #[test]
    fn test_classify_data() {
        let repetitive = vec![0xAA_u8; 10000];
        let (class, ratio) = classify_data(&repetitive);
        assert_eq!(class, "highly_repetitive");
        assert!(ratio > 5.0, "Repetitive data should be highly compressible");

        let text = "Hello world, this is a test of classification. ".repeat(200);
        let (class, _ratio) = classify_data(text.as_bytes());
        assert!(class == "natural_text" || class == "structured_text" || class == "mixed_text",
            "Text should be classified as text variant, got {}", class);

        let binary: Vec<u8> = (0..=255).cycle().take(10000).collect();
        let (class, ratio) = classify_data(&binary);
        assert!(class == "pre_compressed" || class == "high_entropy_binary",
            "High entropy data class should reflect that, got {}", class);
        assert!(ratio < 1.5, "High entropy should not be very compressible");
    }

    #[test]
    fn test_adaptive_redundancy() {
        // Low loss → low redundancy
        let r1 = calculate_adaptive_redundancy(10000, 0.05, 0.01, 0.01);
        assert!(r1 < 2.5, "Low loss should need < 2.5x, got {}", r1);

        // High loss → high redundancy
        let r2 = calculate_adaptive_redundancy(10000, 0.50, 0.05, 0.001);
        assert!(r2 > 2.5, "High loss should need > 2.5x, got {}", r2);

        // Monotonic: higher loss → higher redundancy
        assert!(r2 > r1, "More loss should require more redundancy");

        // Stricter failure prob → higher redundancy
        let r3 = calculate_adaptive_redundancy(10000, 0.15, 0.02, 0.0001);
        let r4 = calculate_adaptive_redundancy(10000, 0.15, 0.02, 0.1);
        assert!(r3 > r4, "Stricter failure prob should need more redundancy");
    }
}
