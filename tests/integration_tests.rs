//! Comprehensive Integration Tests for Helix-Core DNA Data Storage System
//!
//! Tests cover the full pipeline (encode → chaos → decode), individual module
//! roundtrips, edge cases, and multi-format data integrity verification.
//!
//! Every roundtrip is verified with SHA-256 checksums for bit-perfect recovery.

use sha2::{Sha256, Digest};
use rand::prelude::*;
use rand::rngs::StdRng;

use helix_core::pipeline::{HelixPipeline, PipelineConfig};
use helix_core::reed_solomon::ReedSolomonCodec;
use helix_core::fountain::{FountainCodec, Droplet};
use helix_core::transcoder::{Transcoder, bytes_to_dna, dna_to_bytes, calculate_gc, check_homopolymer};
use helix_core::hypercompress::HyperCompressor;
use helix_core::interleaved_rs::InterleavedRS;
use helix_core::chaos::ChaosMatrix;
use helix_core::fasta::{FastaIO, FastaMetadata};
use helix_core::oligo_builder::OligoBuilder;
use helix_core::dna_constraints::DNAConstraints;
use helix_core::compressor::hex_sha256;

// ═══════════════════════════════════════════════════════════════════
//  Helper Utilities
// ═══════════════════════════════════════════════════════════════════

fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    format!("{:x}", hasher.finalize())
}

/// Generate reproducible pseudo-random bytes of given length
fn random_bytes(len: usize, seed: u64) -> Vec<u8> {
    let mut rng = StdRng::seed_from_u64(seed);
    let mut buf = vec![0u8; len];
    rng.fill_bytes(&mut buf);
    buf
}

/// Sample CSV data (~1KB)
fn sample_csv() -> Vec<u8> {
    let mut csv = String::from("id,name,value,category,timestamp\n");
    for i in 0..50 {
        csv.push_str(&format!(
            "{},sensor_{},{:.4},category_{},2026-03-11T{:02}:{:02}:00Z\n",
            i, i % 10, (i as f64) * 1.7321, i % 5, i % 24, i % 60
        ));
    }
    csv.into_bytes()
}

/// Sample JSON data (~1KB)
fn sample_json() -> Vec<u8> {
    let mut json = String::from("[\n");
    for i in 0..30 {
        let comma = if i < 29 { "," } else { "" };
        json.push_str(&format!(
            "  {{\"id\": {}, \"name\": \"item_{}\", \"value\": {:.2}, \"tags\": [\"tag_{}\", \"tag_{}\"]}}{}\\n",
            i, i, (i as f64) * 3.14159, i % 7, i % 3, comma
        ));
    }
    json.push_str("]\n");
    json.into_bytes()
}

/// Sample SQL dump (~1KB)
fn sample_sql() -> Vec<u8> {
    let mut sql = String::from(
        "CREATE TABLE measurements (\n  id INTEGER PRIMARY KEY,\n  sensor_name TEXT NOT NULL,\n  reading REAL,\n  timestamp TEXT\n);\n\n"
    );
    for i in 0..40 {
        sql.push_str(&format!(
            "INSERT INTO measurements VALUES ({}, 'sensor_{}', {:.6}, '2026-03-11T{:02}:{:02}:00Z');\n",
            i, i % 8, (i as f64) * 2.71828, i % 24, i % 60
        ));
    }
    sql.into_bytes()
}

/// Sample source code (Rust)
fn sample_source_code() -> Vec<u8> {
    r#"use std::collections::HashMap;

/// A simple DNA codon lookup table
pub struct CodonTable {
    table: HashMap<String, char>,
}

impl CodonTable {
    pub fn new() -> Self {
        let mut table = HashMap::new();
        table.insert("ATG".to_string(), 'M'); // Start codon
        table.insert("TAA".to_string(), '*'); // Stop codon
        table.insert("TAG".to_string(), '*'); // Stop codon
        table.insert("TGA".to_string(), '*'); // Stop codon
        table.insert("GCT".to_string(), 'A');
        table.insert("GCC".to_string(), 'A');
        table.insert("GCA".to_string(), 'A');
        table.insert("GCG".to_string(), 'A');
        Self { table }
    }

    pub fn translate(&self, codon: &str) -> Option<char> {
        self.table.get(codon).copied()
    }

    pub fn translate_sequence(&self, dna: &str) -> String {
        let mut protein = String::new();
        for i in (0..dna.len()).step_by(3) {
            if i + 3 <= dna.len() {
                let codon = &dna[i..i+3];
                if let Some(aa) = self.translate(codon) {
                    protein.push(aa);
                    if aa == '*' { break; }
                }
            }
        }
        protein
    }
}

fn main() {
    let table = CodonTable::new();
    let dna = "ATGGCTGCCGCATAA";
    let protein = table.translate_sequence(dna);
    println!("DNA: {} -> Protein: {}", dna, protein);
}
"#.as_bytes().to_vec()
}

/// Sample FASTA genomics data
fn sample_fasta_data() -> Vec<u8> {
    let mut fasta = String::new();
    let bases = ['A', 'C', 'G', 'T'];
    let mut rng = StdRng::seed_from_u64(42);
    for i in 0..5 {
        fasta.push_str(&format!(">sequence_{} length=200 organism=test\n", i));
        for _ in 0..200 {
            fasta.push(bases[rng.gen_range(0..4)]);
        }
        fasta.push('\n');
    }
    fasta.into_bytes()
}

// ═══════════════════════════════════════════════════════════════════
//  1. FULL PIPELINE ROUNDTRIP TESTS
// ═══════════════════════════════════════════════════════════════════

#[test]
fn pipeline_roundtrip_text_data() {
    let data = b"Hello, Helix-Core DNA Data Storage System! This is a basic roundtrip test.".to_vec();
    let checksum_before = sha256_hex(&data);

    let config = PipelineConfig::default();
    let mut pipeline = HelixPipeline::new(config);

    let _encode_output = pipeline.encode(&data, "test.txt", None);
    let decode_result = pipeline.decode(None).expect("decode should not error");

    assert!(decode_result.success, "Decode should succeed");
    assert!(decode_result.data_match, "Data should match checksum");
    let recovered = decode_result.recovered_data.expect("Should have recovered data");
    assert_eq!(sha256_hex(&recovered), checksum_before, "SHA-256 must match");
    assert_eq!(recovered, data, "Bit-perfect recovery required");
}

#[test]
fn pipeline_roundtrip_csv() {
    let data = sample_csv();
    let checksum_before = sha256_hex(&data);

    let mut pipeline = HelixPipeline::new(PipelineConfig::default());
    let _encode = pipeline.encode(&data, "data.csv", None);
    let decode = pipeline.decode(None).expect("decode error");

    assert!(decode.success);
    let recovered = decode.recovered_data.unwrap();
    assert_eq!(sha256_hex(&recovered), checksum_before);
}

#[test]
fn pipeline_roundtrip_json() {
    let data = sample_json();
    let checksum_before = sha256_hex(&data);

    let mut pipeline = HelixPipeline::new(PipelineConfig::default());
    pipeline.encode(&data, "data.json", None);
    let decode = pipeline.decode(None).unwrap();

    assert!(decode.success);
    assert_eq!(sha256_hex(&decode.recovered_data.unwrap()), checksum_before);
}

#[test]
fn pipeline_roundtrip_sql_dump() {
    let data = sample_sql();
    let checksum_before = sha256_hex(&data);

    let mut pipeline = HelixPipeline::new(PipelineConfig::default());
    pipeline.encode(&data, "dump.sql", None);
    let decode = pipeline.decode(None).unwrap();

    assert!(decode.success);
    assert_eq!(sha256_hex(&decode.recovered_data.unwrap()), checksum_before);
}

#[test]
fn pipeline_roundtrip_source_code() {
    let data = sample_source_code();
    let checksum_before = sha256_hex(&data);

    let mut pipeline = HelixPipeline::new(PipelineConfig::default());
    pipeline.encode(&data, "main.rs", None);
    let decode = pipeline.decode(None).unwrap();

    assert!(decode.success);
    assert_eq!(sha256_hex(&decode.recovered_data.unwrap()), checksum_before);
}

#[test]
fn pipeline_roundtrip_fasta_genomics() {
    let data = sample_fasta_data();
    let checksum_before = sha256_hex(&data);

    let mut pipeline = HelixPipeline::new(PipelineConfig::default());
    pipeline.encode(&data, "sequences.fasta", None);
    let decode = pipeline.decode(None).unwrap();

    assert!(decode.success);
    assert_eq!(sha256_hex(&decode.recovered_data.unwrap()), checksum_before);
}

#[test]
fn pipeline_roundtrip_no_compression() {
    let data = b"Testing pipeline without compression enabled.".to_vec();
    let checksum_before = sha256_hex(&data);

    let config = PipelineConfig {
        compression: false,
        ..PipelineConfig::default()
    };
    let mut pipeline = HelixPipeline::new(config);
    let encode_out = pipeline.encode(&data, "raw.bin", None);
    assert!(!encode_out.compression_enabled);

    let decode = pipeline.decode(None).unwrap();
    assert!(decode.success);
    assert_eq!(sha256_hex(&decode.recovered_data.unwrap()), checksum_before);
}

// ═══════════════════════════════════════════════════════════════════
//  2. PIPELINE WITH CHAOS (DNA DEGRADATION SIMULATION)
// ═══════════════════════════════════════════════════════════════════

#[test]
fn pipeline_chaos_15_percent_loss() {
    let data = sample_csv();
    let checksum_before = sha256_hex(&data);

    let mut pipeline = HelixPipeline::new(PipelineConfig {
        redundancy: 2.5,
        ..PipelineConfig::default()
    });
    pipeline.encode(&data, "chaos_test.csv", None);

    let chaos = pipeline.apply_chaos(0.15, None, None, None, None)
        .expect("chaos should succeed");
    assert!(chaos.droplet_survival_rate > 0.50,
        "Most droplets should survive 15% loss, got {:.2}%",
        chaos.droplet_survival_rate * 100.0);

    let decode = pipeline.decode(None).unwrap();
    assert!(decode.success, "Should recover from 15% droplet loss");
    assert_eq!(sha256_hex(&decode.recovered_data.unwrap()), checksum_before);
}

#[test]
fn pipeline_chaos_30_percent_loss() {
    let data = sample_sql();
    let checksum_before = sha256_hex(&data);

    let mut pipeline = HelixPipeline::new(PipelineConfig {
        redundancy: 3.0, // Higher redundancy for severe loss
        ..PipelineConfig::default()
    });
    pipeline.encode(&data, "chaos30.sql", None);

    let chaos = pipeline.apply_chaos(0.30, None, None, None, None)
        .expect("chaos should succeed");
    assert!(chaos.droplet_survival_rate > 0.60);

    let decode = pipeline.decode(None).unwrap();
    assert!(decode.success, "Should recover from 30% droplet loss with 3x redundancy");
    assert_eq!(sha256_hex(&decode.recovered_data.unwrap()), checksum_before);
}

#[test]
fn pipeline_chaos_with_mutations() {
    let data = b"DNA storage must survive base substitutions, insertions, and deletions.".to_vec();
    let checksum_before = sha256_hex(&data);

    let mut pipeline = HelixPipeline::new(PipelineConfig {
        redundancy: 2.5,
        deletion_rate: 0.02,
        substitution_rate: 0.03,
        insertion_rate: 0.01,
        ..PipelineConfig::default()
    });
    pipeline.encode(&data, "mutation_test.txt", None);

    // Apply moderate chaos: 10% loss + sequence mutations configured above
    let chaos = pipeline.apply_chaos(0.10, Some(0.02), Some(0.03), Some(0.01), None)
        .expect("chaos should succeed");
    assert!(chaos.mutation_summary.total_mutations > 0, "Should have some mutations");

    // Decode should still work because fountain codes protect the data
    let decode = pipeline.decode(None).unwrap();
    assert!(decode.success, "Should recover despite mutations (fountain protects droplet data)");
    assert_eq!(sha256_hex(&decode.recovered_data.unwrap()), checksum_before);
}

#[test]
fn pipeline_chaos_zero_loss() {
    let data = b"Zero chaos should be lossless trivially.".to_vec();
    let checksum_before = sha256_hex(&data);

    let mut pipeline = HelixPipeline::new(PipelineConfig::default());
    pipeline.encode(&data, "noop.txt", None);
    pipeline.apply_chaos(0.0, Some(0.0), Some(0.0), Some(0.0), None).unwrap();

    let decode = pipeline.decode(None).unwrap();
    assert!(decode.success);
    assert_eq!(sha256_hex(&decode.recovered_data.unwrap()), checksum_before);
}

// ═══════════════════════════════════════════════════════════════════
//  3. EDGE CASES
// ═══════════════════════════════════════════════════════════════════

#[test]
fn pipeline_single_byte() {
    let data = vec![0x42u8]; // Single byte 'B'
    let checksum_before = sha256_hex(&data);

    let mut pipeline = HelixPipeline::new(PipelineConfig::default());
    pipeline.encode(&data, "one_byte.bin", None);
    let decode = pipeline.decode(None).unwrap();

    assert!(decode.success);
    assert_eq!(sha256_hex(&decode.recovered_data.unwrap()), checksum_before);
}

#[test]
fn pipeline_exactly_64_bytes() {
    // Exactly one RS block size (64 bytes)
    let data: Vec<u8> = (0..64).map(|i| i as u8).collect();
    let checksum_before = sha256_hex(&data);

    let mut pipeline = HelixPipeline::new(PipelineConfig {
        block_size: 64,
        ..PipelineConfig::default()
    });
    pipeline.encode(&data, "exact_block.bin", None);
    let decode = pipeline.decode(None).unwrap();

    assert!(decode.success);
    assert_eq!(sha256_hex(&decode.recovered_data.unwrap()), checksum_before);
}

#[test]
fn pipeline_block_boundary_minus_one() {
    // 63 bytes: just under one block
    let data: Vec<u8> = (0..63).map(|i| i as u8).collect();
    let checksum_before = sha256_hex(&data);

    let mut pipeline = HelixPipeline::new(PipelineConfig {
        block_size: 64,
        ..PipelineConfig::default()
    });
    pipeline.encode(&data, "under_block.bin", None);
    let decode = pipeline.decode(None).unwrap();

    assert!(decode.success);
    assert_eq!(sha256_hex(&decode.recovered_data.unwrap()), checksum_before);
}

#[test]
fn pipeline_block_boundary_plus_one() {
    // 65 bytes: just over one block
    let data: Vec<u8> = (0..65).map(|i| i as u8).collect();
    let checksum_before = sha256_hex(&data);

    let mut pipeline = HelixPipeline::new(PipelineConfig {
        block_size: 64,
        ..PipelineConfig::default()
    });
    pipeline.encode(&data, "over_block.bin", None);
    let decode = pipeline.decode(None).unwrap();

    assert!(decode.success);
    assert_eq!(sha256_hex(&decode.recovered_data.unwrap()), checksum_before);
}

#[test]
fn pipeline_all_zeros() {
    let data = vec![0u8; 256];
    let checksum_before = sha256_hex(&data);

    let mut pipeline = HelixPipeline::new(PipelineConfig::default());
    pipeline.encode(&data, "zeros.bin", None);
    let decode = pipeline.decode(None).unwrap();

    assert!(decode.success);
    assert_eq!(sha256_hex(&decode.recovered_data.unwrap()), checksum_before);
}

#[test]
fn pipeline_all_0xff() {
    let data = vec![0xFFu8; 256];
    let checksum_before = sha256_hex(&data);

    let mut pipeline = HelixPipeline::new(PipelineConfig::default());
    pipeline.encode(&data, "ones.bin", None);
    let decode = pipeline.decode(None).unwrap();

    assert!(decode.success);
    assert_eq!(sha256_hex(&decode.recovered_data.unwrap()), checksum_before);
}

#[test]
fn pipeline_repeating_pattern() {
    // Highly compressible: repeating short pattern
    let pattern = b"ACGT";
    let data: Vec<u8> = pattern.iter().cycle().take(1024).copied().collect();
    let checksum_before = sha256_hex(&data);

    let mut pipeline = HelixPipeline::new(PipelineConfig::default());
    pipeline.encode(&data, "pattern.txt", None);
    let decode = pipeline.decode(None).unwrap();

    assert!(decode.success);
    assert_eq!(sha256_hex(&decode.recovered_data.unwrap()), checksum_before);
}

#[test]
fn pipeline_random_binary_data() {
    let data = random_bytes(2048, 12345);
    let checksum_before = sha256_hex(&data);

    let mut pipeline = HelixPipeline::new(PipelineConfig::default());
    pipeline.encode(&data, "random.bin", None);
    let decode = pipeline.decode(None).unwrap();

    assert!(decode.success);
    assert_eq!(sha256_hex(&decode.recovered_data.unwrap()), checksum_before);
}

// ═══════════════════════════════════════════════════════════════════
//  4. DIFFERENT PIPELINE CONFIGURATIONS
// ═══════════════════════════════════════════════════════════════════

#[test]
fn pipeline_small_block_size() {
    let data = sample_csv();
    let checksum_before = sha256_hex(&data);

    let config = PipelineConfig {
        block_size: 32,
        redundancy: 2.0,
        ..PipelineConfig::default()
    };
    let mut pipeline = HelixPipeline::new(config);
    pipeline.encode(&data, "small_block.csv", None);
    let decode = pipeline.decode(None).unwrap();

    assert!(decode.success);
    assert_eq!(sha256_hex(&decode.recovered_data.unwrap()), checksum_before);
}

#[test]
fn pipeline_large_block_size() {
    let data = sample_csv();
    let checksum_before = sha256_hex(&data);

    let config = PipelineConfig {
        block_size: 128,
        redundancy: 2.0,
        ..PipelineConfig::default()
    };
    let mut pipeline = HelixPipeline::new(config);
    pipeline.encode(&data, "large_block.csv", None);
    let decode = pipeline.decode(None).unwrap();

    assert!(decode.success);
    assert_eq!(sha256_hex(&decode.recovered_data.unwrap()), checksum_before);
}

#[test]
fn pipeline_high_redundancy() {
    let data = b"High redundancy test data for maximum fault tolerance.".to_vec();
    let checksum_before = sha256_hex(&data);

    let config = PipelineConfig {
        redundancy: 4.0,
        ..PipelineConfig::default()
    };
    let mut pipeline = HelixPipeline::new(config);
    let encode_out = pipeline.encode(&data, "high_red.txt", None);
    assert!(encode_out.fountain_stats.redundancy_ratio >= 3.5);

    let decode = pipeline.decode(None).unwrap();
    assert!(decode.success);
    assert_eq!(sha256_hex(&decode.recovered_data.unwrap()), checksum_before);
}

#[test]
fn pipeline_short_oligo_length() {
    let data = b"Short oligo test with 150bp oligos.".to_vec();
    let checksum_before = sha256_hex(&data);

    let config = PipelineConfig {
        oligo_length: 150,
        ..PipelineConfig::default()
    };
    let mut pipeline = HelixPipeline::new(config);
    pipeline.encode(&data, "short_oligo.txt", None);
    let decode = pipeline.decode(None).unwrap();

    assert!(decode.success);
    assert_eq!(sha256_hex(&decode.recovered_data.unwrap()), checksum_before);
}

// ═══════════════════════════════════════════════════════════════════
//  5. REED-SOLOMON STANDALONE TESTS
// ═══════════════════════════════════════════════════════════════════

#[test]
fn rs_encode_decode_clean() {
    let rs = ReedSolomonCodec::new(223, 32);
    let data: Vec<u8> = (0..223).map(|i| (i % 256) as u8).collect();
    let codeword = rs.encode(&data);

    assert_eq!(codeword.len(), 255);
    let (decoded, errors) = rs.decode(&codeword).expect("clean decode should succeed");
    assert_eq!(errors, 0);
    assert_eq!(decoded, data);
}

#[test]
fn rs_correct_up_to_16_errors() {
    // RS(255,223) corrects up to 16 symbol errors
    let rs = ReedSolomonCodec::new(223, 32);
    let data: Vec<u8> = (0..223).map(|i| ((i * 13) % 256) as u8).collect();
    let mut codeword = rs.encode(&data);

    // Inject exactly 16 errors at known positions
    let error_positions: Vec<usize> = vec![0, 10, 20, 30, 50, 70, 90, 110,
                                           130, 150, 170, 190, 200, 220, 240, 250];
    for &pos in &error_positions {
        if pos < codeword.len() {
            codeword[pos] ^= 0xAB;
        }
    }

    let (decoded, errors) = rs.decode(&codeword).expect("should correct 16 errors");
    assert_eq!(errors, 16);
    assert_eq!(decoded, data, "Must recover original data after 16-error correction");
}

#[test]
fn rs_fail_on_too_many_errors() {
    // RS(255,223) cannot correct 17+ errors
    let rs = ReedSolomonCodec::new(223, 32);
    let data: Vec<u8> = (0..223).map(|i| i as u8).collect();
    let mut codeword = rs.encode(&data);

    // Inject 17 errors — exceeds correction capacity
    for i in 0..17 {
        codeword[i * 15] ^= 0xFF;
    }

    let result = rs.decode(&codeword);
    assert!(result.is_none(), "17 errors should exceed RS(255,223) capacity");
}

#[test]
fn rs_buffer_roundtrip() {
    let rs = ReedSolomonCodec::new(223, 32);
    let data = b"Reed-Solomon buffer API roundtrip test for DNA storage.".to_vec();
    let checksum_before = sha256_hex(&data);

    let (encoded, enc_stats) = rs.encode_buffer(&data);
    assert!(enc_stats.blocks_encoded > 0);

    let (decoded, dec_stats) = rs.decode_buffer(&encoded).expect("buffer decode should work");
    assert_eq!(dec_stats.total_errors_corrected, 0);
    assert_eq!(sha256_hex(&decoded), checksum_before);
}

#[test]
fn rs_buffer_with_corruption() {
    let rs = ReedSolomonCodec::new(223, 32);
    let data: Vec<u8> = (0..500).map(|i| (i % 256) as u8).collect();
    let checksum_before = sha256_hex(&data);

    let (mut encoded, _) = rs.encode_buffer(&data);

    // Corrupt a few bytes per block (well under 16 per block)
    for i in (0..encoded.len()).step_by(80) {
        encoded[i] ^= 0xCC;
    }

    let (decoded, stats) = rs.decode_buffer(&encoded).expect("should correct moderate corruption");
    assert!(stats.total_errors_corrected > 0, "Should have corrected some errors");
    assert_eq!(sha256_hex(&decoded), checksum_before);
}

#[test]
fn rs_lightweight_mode() {
    let rs = ReedSolomonCodec::lightweight(); // RS(255,239) — 8-error correction
    let data: Vec<u8> = (0..239).map(|i| i as u8).collect();
    let codeword = rs.encode(&data);

    assert_eq!(codeword.len(), 255);

    // Inject 8 errors (max for RS(255,239))
    let mut corrupted = codeword.clone();
    for i in 0..8 {
        corrupted[i * 30] ^= 0xFF;
    }

    let (decoded, errors) = rs.decode(&corrupted).expect("should correct 8 errors");
    assert_eq!(errors, 8);
    assert_eq!(decoded, data);
}

#[test]
fn rs_default_commercial() {
    let rs = ReedSolomonCodec::default_commercial();
    assert_eq!(rs.data_symbols, 223);
    assert_eq!(rs.parity_symbols, 32);
    assert_eq!(rs.total_symbols, 255);
}

// ═══════════════════════════════════════════════════════════════════
//  6. FOUNTAIN CODEC STANDALONE TESTS
// ═══════════════════════════════════════════════════════════════════

#[test]
fn fountain_encode_decode_no_loss() {
    let codec = FountainCodec::new(64, 2.0, 42);
    let data = b"Fountain codes: rateless erasure codes for DNA data recovery.".to_vec();
    let encoded = codec.encode(&data);

    assert!(encoded.droplets.len() > 0);
    assert_eq!(encoded.original_length, data.len());

    let decoded = codec.decode(&encoded, &encoded.droplets);
    assert!(decoded.is_some());
    assert_eq!(decoded.unwrap(), data);
}

#[test]
fn fountain_survive_20_percent_loss() {
    let codec = FountainCodec::new(32, 2.5, 42);
    let data: Vec<u8> = (0..512).map(|i| (i % 256) as u8).collect();
    let checksum_before = sha256_hex(&data);
    let encoded = codec.encode(&data);

    // Drop 20% of droplets deterministically
    let mut rng = StdRng::seed_from_u64(99);
    let surviving: Vec<Droplet> = encoded.droplets.iter()
        .filter(|_| rng.gen::<f64>() > 0.20)
        .cloned()
        .collect();

    assert!(surviving.len() < encoded.droplets.len());

    let decoded = codec.decode(&encoded, &surviving);
    assert!(decoded.is_some(), "Should decode with 20% loss at 2.5x redundancy");
    assert_eq!(sha256_hex(&decoded.unwrap()), checksum_before);
}

#[test]
fn fountain_survive_40_percent_loss_high_redundancy() {
    let codec = FountainCodec::new(32, 3.0, 42);
    let data: Vec<u8> = (0..256).map(|i| (i % 256) as u8).collect();
    let checksum_before = sha256_hex(&data);
    let encoded = codec.encode(&data);

    let mut rng = StdRng::seed_from_u64(77);
    let surviving: Vec<Droplet> = encoded.droplets.iter()
        .filter(|_| rng.gen::<f64>() > 0.40)
        .cloned()
        .collect();

    let decoded = codec.decode(&encoded, &surviving);
    assert!(decoded.is_some(), "3x redundancy should survive 40% loss");
    assert_eq!(sha256_hex(&decoded.unwrap()), checksum_before);
}

#[test]
fn fountain_stats() {
    let codec = FountainCodec::new(64, 2.0, 42);
    let data = vec![0u8; 256]; // 4 blocks of 64
    let encoded = codec.encode(&data);
    let stats = codec.get_stats(&encoded);

    assert_eq!(stats.num_blocks, 4);
    assert!(stats.num_droplets >= 8); // 2.0x redundancy
    assert!(stats.redundancy_ratio >= 1.9);
    assert!(stats.distribution.contains("Robust"));
}

#[test]
fn fountain_empty_data() {
    let codec = FountainCodec::new(64, 2.0, 42);
    let data: Vec<u8> = Vec::new();
    let encoded = codec.encode(&data);
    assert_eq!(encoded.num_blocks, 0);
    assert_eq!(encoded.original_length, 0);

    let decoded = codec.decode(&encoded, &encoded.droplets);
    assert!(decoded.is_some());
    assert!(decoded.unwrap().is_empty());
}

// ═══════════════════════════════════════════════════════════════════
//  7. TRANSCODER ROUNDTRIP TESTS
// ═══════════════════════════════════════════════════════════════════

#[test]
fn transcoder_roundtrip_text() {
    let t = Transcoder::new();
    let data = b"Hello, DNA World! Testing transcoder roundtrip.";
    let encoded = t.encode(data);
    let decoded = t.decode(&encoded.sequence, encoded.rotation_key, encoded.original_length);

    assert_eq!(decoded, data);
    assert!(encoded.gc_content > 0.0 && encoded.gc_content <= 1.0);
}

#[test]
fn transcoder_roundtrip_all_bytes() {
    let t = Transcoder::new();
    let data: Vec<u8> = (0..=255).collect();
    let encoded = t.encode(&data);
    let decoded = t.decode(&encoded.sequence, encoded.rotation_key, encoded.original_length);

    assert_eq!(decoded, data, "All 256 byte values must roundtrip");
    assert_eq!(encoded.length, 1024, "256 bytes = 1024 DNA bases");
}

#[test]
fn transcoder_roundtrip_random() {
    let t = Transcoder::new();
    let data = random_bytes(4096, 54321);
    let checksum_before = sha256_hex(&data);

    let encoded = t.encode(&data);
    let decoded = t.decode(&encoded.sequence, encoded.rotation_key, encoded.original_length);

    assert_eq!(sha256_hex(&decoded), checksum_before);
}

#[test]
fn transcoder_gc_balance() {
    let t = Transcoder::new();
    // Test that rotation key selection achieves reasonable GC balance
    let data = b"AAAAAAAAAAAAAAAA"; // Worst-case ASCII input
    let encoded = t.encode(data);

    assert!(
        (encoded.gc_content - 0.5).abs() < 0.2,
        "GC content should be close to 50%: got {:.1}%",
        encoded.gc_content * 100.0
    );
}

#[test]
fn transcoder_empty_input() {
    let t = Transcoder::new();
    let encoded = t.encode(b"");
    assert_eq!(encoded.length, 0);
    assert!(encoded.homopolymer_safe);
    let decoded = t.decode(&encoded.sequence, encoded.rotation_key, 0);
    assert!(decoded.is_empty());
}

#[test]
fn transcoder_free_functions() {
    // Test the standalone bytes_to_dna / dna_to_bytes
    let data = vec![0b00011011u8]; // A=00, C=01, G=10, T=11
    let dna = bytes_to_dna(&data);
    assert_eq!(dna, "ACGT");

    let back = dna_to_bytes(&dna);
    assert_eq!(back, data);
}

#[test]
fn transcoder_gc_calculation() {
    assert!((calculate_gc("GCGC") - 1.0).abs() < 0.001);
    assert!((calculate_gc("ATAT") - 0.0).abs() < 0.001);
    assert!((calculate_gc("ACGT") - 0.5).abs() < 0.001);
    assert!((calculate_gc("") - 0.0).abs() < 0.001);
}

#[test]
fn transcoder_homopolymer_check() {
    assert!(check_homopolymer("ACGT"));       // No repeats
    assert!(check_homopolymer("AAACGT"));     // 3×A = at threshold, safe
    assert!(!check_homopolymer("AAAACGT"));   // 4×A = violation
    assert!(!check_homopolymer("ACGTTTTACG")); // 4×T
    assert!(check_homopolymer("ACGTTTACG"));  // 3×T = safe
}

// ═══════════════════════════════════════════════════════════════════
//  8. HYPERCOMPRESS ROUNDTRIP TESTS
// ═══════════════════════════════════════════════════════════════════

#[test]
fn hypercompress_roundtrip_csv() {
    let hc = HyperCompressor::new();
    let data = sample_csv();
    let checksum_before = sha256_hex(&data);

    let (compressed, stats) = hc.compress(&data, None);
    assert!(stats.compressed_size > 0);
    assert!(stats.compression_ratio >= 1.0, "CSV should compress well");

    let decompressed = hc.decompress(&compressed).expect("decompression should succeed");
    assert_eq!(sha256_hex(&decompressed), checksum_before);
}

#[test]
fn hypercompress_roundtrip_json() {
    let hc = HyperCompressor::new();
    let data = sample_json();
    let checksum_before = sha256_hex(&data);

    let (compressed, _) = hc.compress(&data, None);
    let decompressed = hc.decompress(&compressed).unwrap();
    assert_eq!(sha256_hex(&decompressed), checksum_before);
}

#[test]
fn hypercompress_roundtrip_source_code() {
    let hc = HyperCompressor::new();
    let data = sample_source_code();
    let checksum_before = sha256_hex(&data);

    let (compressed, stats) = hc.compress(&data, None);
    assert!(stats.original_size == data.len());

    let decompressed = hc.decompress(&compressed).unwrap();
    assert_eq!(sha256_hex(&decompressed), checksum_before);
}

#[test]
fn hypercompress_roundtrip_random_bytes() {
    let hc = HyperCompressor::new();
    let data = random_bytes(4096, 11111);
    let checksum_before = sha256_hex(&data);

    let (compressed, _stats) = hc.compress(&data, None);
    // Random data is near-incompressible, but roundtrip must still work
    let decompressed = hc.decompress(&compressed).unwrap();
    assert_eq!(sha256_hex(&decompressed), checksum_before);
}

#[test]
fn hypercompress_small_data() {
    let hc = HyperCompressor::new();
    let data = b"tiny".to_vec();
    let checksum_before = sha256_hex(&data);

    let (compressed, _) = hc.compress(&data, None);
    let decompressed = hc.decompress(&compressed).unwrap();
    assert_eq!(sha256_hex(&decompressed), checksum_before);
}

#[test]
fn hypercompress_highly_repetitive() {
    let hc = HyperCompressor::new();
    // Highly repetitive data should compress very well
    let line = "INSERT INTO table VALUES (1, 'test', 42.0);\n";
    let data: Vec<u8> = line.repeat(200).into_bytes();
    let checksum_before = sha256_hex(&data);

    let (compressed, stats) = hc.compress(&data, None);
    assert!(
        stats.compression_ratio > 2.0,
        "Highly repetitive data should have >2x compression ratio, got {:.2}",
        stats.compression_ratio
    );

    let decompressed = hc.decompress(&compressed).unwrap();
    assert_eq!(sha256_hex(&decompressed), checksum_before);
}

#[test]
fn hypercompress_empty_data() {
    let hc = HyperCompressor::new();
    let data: Vec<u8> = Vec::new();
    let (compressed, _) = hc.compress(&data, None);
    assert!(compressed.is_empty());
}

// ═══════════════════════════════════════════════════════════════════
//  9. INTERLEAVED REED-SOLOMON TESTS
// ═══════════════════════════════════════════════════════════════════

#[test]
fn interleaved_rs_roundtrip_clean() {
    let irs = InterleavedRS::default_commercial();
    let data = b"Interleaved RS provides cross-oligo error protection.".to_vec();
    let checksum_before = sha256_hex(&data);

    let (encoded, stats) = irs.encode_buffer(&data);
    assert!(stats.blocks_encoded > 0);
    assert!(stats.overhead_percent > 0.0);

    let (decoded, dec_stats) = irs.decode_buffer(&encoded).expect("clean decode should work");
    assert_eq!(dec_stats.total_errors_corrected, 0);
    assert_eq!(sha256_hex(&decoded), checksum_before);
}

#[test]
fn interleaved_rs_with_corruption() {
    let irs = InterleavedRS::default_commercial();
    let data: Vec<u8> = (0..2000).map(|i| (i % 256) as u8).collect();
    let checksum_before = sha256_hex(&data);

    let (mut encoded, stats) = irs.encode_buffer(&data);

    // Corrupt 1 byte per row for the first few rows (within correction capability)
    let header_size = 11;
    let row_size = stats.total_symbols_per_row;
    for r in 0..5.min(stats.blocks_encoded) {
        let offset = header_size + r * row_size + 3;
        if offset < encoded.len() {
            encoded[offset] ^= 0xFF;
        }
    }

    let (decoded, dec_stats) = irs.decode_buffer(&encoded).expect("should correct scattered errors");
    assert!(dec_stats.total_errors_corrected > 0);
    assert_eq!(sha256_hex(&decoded), checksum_before);
}

#[test]
fn interleaved_rs_lightweight() {
    let irs = InterleavedRS::lightweight();
    let data = b"Lightweight interleaved RS with 16 parity symbols.".to_vec();
    let checksum_before = sha256_hex(&data);

    let (encoded, stats) = irs.encode_buffer(&data);
    assert_eq!(stats.parity_symbols_per_row, 16);
    assert_eq!(stats.max_correctable_per_row, 8);

    let (decoded, _) = irs.decode_buffer(&encoded).unwrap();
    assert_eq!(sha256_hex(&decoded), checksum_before);
}

#[test]
fn interleaved_rs_large_data() {
    let irs = InterleavedRS::default_commercial();
    let data: Vec<u8> = (0..10000).map(|i| (i % 256) as u8).collect();
    let checksum_before = sha256_hex(&data);

    let (encoded, stats) = irs.encode_buffer(&data);
    assert!(stats.num_groups >= 1);

    let (decoded, _) = irs.decode_buffer(&encoded).unwrap();
    assert_eq!(sha256_hex(&decoded), checksum_before);
}

// ═══════════════════════════════════════════════════════════════════
//  10. CHAOS ENGINE STANDALONE TESTS
// ═══════════════════════════════════════════════════════════════════

#[test]
fn chaos_droplet_loss() {
    let chaos = ChaosMatrix::new(0.0, 0.0, 0.0, 42);
    let codec = FountainCodec::new(64, 2.0, 42);
    let data = vec![0u8; 256];
    let encoded = codec.encode(&data);
    let total = encoded.droplets.len();

    // 50% loss
    let (survived, stats) = chaos.mutate_droplets(&encoded.droplets, 0.50);
    assert!(stats.lost_droplets > 0);
    assert!(stats.surviving_droplets > 0);
    assert_eq!(stats.total_droplets, total);
    assert!(survived.len() < total);
    // With seeded RNG, ~50% should survive (allow large margin for small sample)
    let ratio = survived.len() as f64 / total as f64;
    assert!(ratio > 0.25 && ratio < 0.75, "Survival ratio {:.2} seems extreme", ratio);
}

#[test]
fn chaos_zero_loss() {
    let chaos = ChaosMatrix::new(0.0, 0.0, 0.0, 42);
    let codec = FountainCodec::new(64, 2.0, 42);
    let data = vec![1u8; 128];
    let encoded = codec.encode(&data);

    let (survived, stats) = chaos.mutate_droplets(&encoded.droplets, 0.0);
    assert_eq!(stats.lost_droplets, 0);
    assert_eq!(survived.len(), encoded.droplets.len());
}

#[test]
fn chaos_sequence_mutations() {
    let chaos = ChaosMatrix::new(0.02, 0.05, 0.01, 42);
    let seq = "ACGTACGTACGTACGTACGTACGTACGTACGTACGTACGTACGTACGTACGT";
    let (mutated, summary) = chaos.mutate_sequence(seq);

    assert!(summary.total_mutations > 0, "Should have some mutations");
    assert!(summary.substitutions + summary.deletions + summary.insertions == summary.total_mutations);
    // Mutated sequence should differ from original
    assert_ne!(mutated, seq);
}

#[test]
fn chaos_no_mutations() {
    let chaos = ChaosMatrix::new(0.0, 0.0, 0.0, 42);
    let seq = "ACGTACGT";
    let (mutated, summary) = chaos.mutate_sequence(seq);

    assert_eq!(summary.total_mutations, 0);
    assert_eq!(mutated, seq);
}

// ═══════════════════════════════════════════════════════════════════
//  11. FASTA I/O TESTS
// ═══════════════════════════════════════════════════════════════════

#[test]
fn fasta_create_and_parse_oligos() {
    let fasta = FastaIO::new();
    let sequence = "ACGTACGTACGTACGTACGTACGTACGTACGTACGTACGT"; // 40 bases

    // Split into oligos of 16 bases each
    let oligos = fasta.create_oligos(sequence, 16);
    assert_eq!(oligos.len(), 3); // 16 + 16 + 8

    let fasta_str = fasta.generate_fasta_string(&oligos);
    assert!(fasta_str.contains(">HELIX_"));
    assert!(fasta_str.contains("ACGTACGTACGTACGT"));

    // Parse back
    let (parsed, _meta) = FastaIO::parse_fasta(&fasta_str);
    assert_eq!(parsed.len(), oligos.len());
    for (orig, parsed_rec) in oligos.iter().zip(parsed.iter()) {
        assert_eq!(orig.sequence, parsed_rec.sequence);
    }
}

#[test]
fn fasta_metadata_roundtrip() {
    let fasta = FastaIO::new();
    let oligos = fasta.create_oligos("ACGTACGT", 8);

    let metadata = FastaMetadata {
        rotation_key: 42,
        original_length: 1024,
        rs_enabled: true,
        use_interleaved_rs: true,
        compression_enabled: true,
        use_hypercompress: true,
        original_filename: "test_data.csv".to_string(),
        original_checksum: "abc123def456".to_string(),
        block_size: 64,
        redundancy: 2.0,
        seed: 42,
    };

    let fasta_str = fasta.generate_fasta_with_metadata(&oligos, &metadata);
    assert!(fasta_str.contains(";HELIX-CORE"));
    assert!(fasta_str.contains(";META:rotation_key=42"));
    assert!(fasta_str.contains(";META:original_filename=test_data.csv"));

    let (parsed_oligos, parsed_meta) = FastaIO::parse_fasta(&fasta_str);
    assert_eq!(parsed_oligos.len(), oligos.len());

    let meta = parsed_meta.expect("Should parse metadata");
    assert_eq!(meta.rotation_key, 42);
    assert_eq!(meta.original_length, 1024);
    assert!(meta.rs_enabled);
    assert!(meta.compression_enabled);
    assert_eq!(meta.original_filename, "test_data.csv");
    assert_eq!(meta.block_size, 64);
}

#[test]
fn fasta_stats() {
    let fasta = FastaIO::new();
    let sequence = "ACGTACGTACGTACGT"; // 16 bases
    let oligos = fasta.create_oligos(sequence, 8);
    let stats = fasta.get_stats(&oligos);

    assert_eq!(stats.num_oligos, 2);
    assert_eq!(stats.total_bases, 16);
}

// ═══════════════════════════════════════════════════════════════════
//  12. OLIGO BUILDER TESTS
// ═══════════════════════════════════════════════════════════════════

#[test]
fn oligo_builder_build_and_disassemble() {
    let builder = OligoBuilder::new(300);

    // Create a DNA sequence long enough to produce at least one oligo
    let t = Transcoder::new();
    let data = random_bytes(512, 99999);
    let encoded = t.encode(&data);

    let (oligos, build_stats) = builder.build_oligos(&encoded.sequence);
    assert!(build_stats.num_oligos > 0);
    assert_eq!(build_stats.oligo_total_length, 300);
    assert!(build_stats.has_primers);
    assert!(build_stats.has_crc);
    assert!(build_stats.has_index);

    // Each oligo should have forward primer, index, payload, CRC, reverse primer
    // Note: the last oligo may be shorter if the payload doesn't fill its capacity
    for (i, oligo) in oligos.iter().enumerate() {
        if i < oligos.len() - 1 {
            assert_eq!(oligo.full_sequence.len(), 300,
                "Non-last oligo should be full length");
        }
        assert!(oligo.payload_length > 0);
        assert!(oligo.full_sequence.starts_with("CTACACGACGCTCTTCCGAT")); // forward primer
    }

    // Disassemble should recover the payloads
    let raw_seqs: Vec<String> = oligos.iter().map(|o| o.full_sequence.clone()).collect();
    let (payloads, crc_pass, crc_fail) = builder.disassemble_oligos(&raw_seqs)
        .expect("disassembly should succeed");

    assert!(crc_pass > 0);
    assert_eq!(crc_fail, 0, "All CRCs should pass on clean oligos");
    assert_eq!(payloads.len(), oligos.len());
}

#[test]
fn oligo_builder_quality_report() {
    let builder = OligoBuilder::new(200);
    let t = Transcoder::new();
    let data = random_bytes(256, 77777);
    let encoded = t.encode(&data);

    let (oligos, _) = builder.build_oligos(&encoded.sequence);
    let report = builder.quality_report(&oligos);

    assert_eq!(report.total_oligos, oligos.len());
    assert!(report.mean_quality > 0.0);
    assert!(report.payload_efficiency > 0.0 && report.payload_efficiency < 1.0);
    assert!(report.crc_pass_rate > 0.0);
}

// ═══════════════════════════════════════════════════════════════════
//  13. DNA CONSTRAINTS TESTS
// ═══════════════════════════════════════════════════════════════════

#[test]
fn dna_constraints_valid_oligos() {
    let constraints = DNAConstraints::new();

    // Construct some relatively well-behaved sequences
    let good_oligos: Vec<String> = (0..5).map(|i| {
        let mut rng = StdRng::seed_from_u64(i + 100);
        let bases = ['A', 'C', 'G', 'T'];
        // Generate with alternating bases to avoid homopolymers
        (0..100).map(|j| {
            if j % 4 == 0 { bases[rng.gen_range(0..4)] }
            else { bases[j % 4] }
        }).collect::<String>()
    }).collect();

    let oligo_refs: Vec<&str> = good_oligos.iter().map(|s| s.as_str()).collect();
    let report = constraints.check_oligos(&oligo_refs);

    assert_eq!(report.total_oligos, 5);
    assert!(report.synthesis_readiness_score >= 0.0);
    assert!(report.synthesis_readiness_score <= 1.0);
}

#[test]
fn dna_constraints_detect_homopolymer() {
    let constraints = DNAConstraints::new();

    // Sequence with a long homopolymer run (violation)
    let bad_seq = "ACGTAAAAAAAACGT"; // 8×A = violation
    let report = constraints.check_oligos(&[bad_seq]);

    assert!(report.homopolymer_stats.total_violations > 0);
    assert!(!report.homopolymer_stats.safe);
}

// ═══════════════════════════════════════════════════════════════════
//  14. CROSS-MODULE INTEGRATION TESTS
// ═══════════════════════════════════════════════════════════════════

#[test]
fn cross_module_rs_then_fountain() {
    // RS encode → Fountain encode → Fountain decode → RS decode
    let rs = ReedSolomonCodec::default_commercial();
    let fountain = FountainCodec::new(64, 2.0, 42);
    let data = b"Cross-module test: RS protection around fountain codes.".to_vec();
    let checksum_before = sha256_hex(&data);

    // RS encode
    let (rs_encoded, _) = rs.encode_buffer(&data);

    // Fountain encode
    let fountain_encoded = fountain.encode(&rs_encoded);

    // Fountain decode (no loss)
    let fountain_decoded = fountain.decode(&fountain_encoded, &fountain_encoded.droplets)
        .expect("fountain decode should work");

    // RS decode
    let (final_data, _) = rs.decode_buffer(&fountain_decoded)
        .expect("RS decode should work after fountain recovery");

    assert_eq!(sha256_hex(&final_data), checksum_before);
}

#[test]
fn cross_module_compress_transcode_roundtrip() {
    // Compress → Transcode → Reverse-transcode → Decompress
    let hc = HyperCompressor::new();
    let t = Transcoder::new();
    let data = sample_csv();
    let checksum_before = sha256_hex(&data);

    let (compressed, _) = hc.compress(&data, None);
    let transcode_result = t.encode(&compressed);
    let reverse_transcoded = t.decode(
        &transcode_result.sequence,
        transcode_result.rotation_key,
        transcode_result.original_length,
    );
    let decompressed = hc.decompress(&reverse_transcoded).unwrap();

    assert_eq!(sha256_hex(&decompressed), checksum_before);
}

#[test]
fn cross_module_interleaved_rs_with_fountain() {
    // InterleavedRS → Fountain → drop 15% → Fountain decode → InterleavedRS decode
    let irs = InterleavedRS::default_commercial();
    let fountain = FountainCodec::new(64, 2.5, 42);

    let data: Vec<u8> = (0..1000).map(|i| (i % 256) as u8).collect();
    let checksum_before = sha256_hex(&data);

    let (irs_encoded, _) = irs.encode_buffer(&data);
    let fountain_encoded = fountain.encode(&irs_encoded);

    // Drop 15% of droplets
    let mut rng = StdRng::seed_from_u64(555);
    let surviving: Vec<Droplet> = fountain_encoded.droplets.iter()
        .filter(|_| rng.gen::<f64>() > 0.15)
        .cloned()
        .collect();

    let fountain_decoded = fountain.decode(&fountain_encoded, &surviving)
        .expect("fountain should decode with 15% loss and 2.5x redundancy");

    let (final_data, _) = irs.decode_buffer(&fountain_decoded)
        .expect("interleaved RS should decode recovered data");

    assert_eq!(sha256_hex(&final_data), checksum_before);
}

// ═══════════════════════════════════════════════════════════════════
//  15. PIPELINE ENCODE OUTPUT VALIDATION
// ═══════════════════════════════════════════════════════════════════

#[test]
fn pipeline_encode_output_fields() {
    let data = sample_csv();
    let mut pipeline = HelixPipeline::new(PipelineConfig::default());
    let output = pipeline.encode(&data, "validate.csv", None);

    // Verify encode output fields are populated
    assert_eq!(output.filename, "validate.csv");
    assert_eq!(output.original_size, data.len());
    assert!(output.pre_compress_size > 0);
    assert!(output.post_compress_size > 0);
    assert!(!output.original_checksum.is_empty());
    assert!(!output.original_data_checksum.is_empty());
    assert!(output.compression_enabled);
    assert!(output.compression_stats.is_some());

    // Transcode info
    assert!(output.transcode.sequence_length > 0);
    assert!(output.transcode.gc_content > 0.0);
    assert!(output.transcode.gc_content <= 1.0);

    // Fountain stats
    assert!(output.fountain_stats.num_blocks > 0);
    assert!(output.fountain_stats.num_droplets > 0);

    // RS should be enabled by default
    assert!(output.rs_stats.is_some());

    // FASTA output
    assert!(!output.fasta_content.is_empty());
    assert!(output.fasta_stats.num_oligos > 0);
    assert!(output.num_oligos > 0);

    // Constraint report
    assert!(output.constraint_report.is_some());

    // Oligo quality
    assert!(output.oligo_quality.is_some());

    // Cost estimate
    assert!(output.cost_estimate.is_some());

    // Timing
    assert!(output.encode_time > 0.0);
}

#[test]
fn pipeline_decode_output_fields() {
    let data = sample_csv();
    let mut pipeline = HelixPipeline::new(PipelineConfig::default());
    pipeline.encode(&data, "decode_fields.csv", None);
    let decode = pipeline.decode(None).unwrap();

    assert!(decode.success);
    assert!(decode.data_match);
    assert_eq!(decode.recovered_size, data.len());
    assert!(decode.recovered_data.is_some());
    assert!(!decode.recovered_preview.is_empty());
    assert!(decode.decode_time >= 0.0);
    assert!(decode.decompression_stats.is_some());
}

// ═══════════════════════════════════════════════════════════════════
//  16. DETERMINISM / REPRODUCIBILITY TESTS
// ═══════════════════════════════════════════════════════════════════

#[test]
fn pipeline_deterministic_encode() {
    let data = b"Determinism test: same input, same config, same output.".to_vec();

    let config1 = PipelineConfig { seed: 42, ..PipelineConfig::default() };
    let config2 = PipelineConfig { seed: 42, ..PipelineConfig::default() };

    let mut p1 = HelixPipeline::new(config1);
    let mut p2 = HelixPipeline::new(config2);

    let out1 = p1.encode(&data, "det.txt", None);
    let out2 = p2.encode(&data, "det.txt", None);

    assert_eq!(out1.original_checksum, out2.original_checksum);
    assert_eq!(out1.fountain_stats.num_droplets, out2.fountain_stats.num_droplets);
    assert_eq!(out1.transcode.rotation_key, out2.transcode.rotation_key);
}

#[test]
fn rs_deterministic() {
    let rs = ReedSolomonCodec::new(223, 32);
    let data: Vec<u8> = (0..223).map(|i| i as u8).collect();

    let cw1 = rs.encode(&data);
    let cw2 = rs.encode(&data);
    assert_eq!(cw1, cw2, "RS encoding must be deterministic");
}

#[test]
fn fountain_deterministic_with_same_seed() {
    let codec1 = FountainCodec::new(64, 2.0, 42);
    let codec2 = FountainCodec::new(64, 2.0, 42);
    let data = vec![7u8; 256];

    let enc1 = codec1.encode(&data);
    let enc2 = codec2.encode(&data);

    assert_eq!(enc1.droplets.len(), enc2.droplets.len());
    for (d1, d2) in enc1.droplets.iter().zip(enc2.droplets.iter()) {
        assert_eq!(d1.data, d2.data, "Droplet data must be identical with same seed");
        assert_eq!(d1.block_indices, d2.block_indices);
    }
}

// ═══════════════════════════════════════════════════════════════════
//  17. STRESS & BOUNDARY TESTS
// ═══════════════════════════════════════════════════════════════════

#[test]
fn rs_all_data_symbol_sizes() {
    // Test various RS configurations
    for &(k, t) in &[(10, 4), (50, 10), (100, 20), (200, 32), (223, 32)] {
        let rs = ReedSolomonCodec::new(k, t);
        let data: Vec<u8> = (0..k).map(|i| (i % 256) as u8).collect();
        let cw = rs.encode(&data);
        assert_eq!(cw.len(), k + t);

        let (dec, ne) = rs.decode(&cw).expect(&format!("RS({},{}) clean decode failed", k + t, k));
        assert_eq!(ne, 0);
        assert_eq!(dec, data);
    }
}

#[test]
fn fountain_various_block_sizes() {
    for &bs in &[16, 32, 64, 128] {
        let codec = FountainCodec::new(bs, 2.0, 42);
        let data: Vec<u8> = (0..bs * 4).map(|i| (i % 256) as u8).collect();
        let checksum = sha256_hex(&data);

        let encoded = codec.encode(&data);
        let decoded = codec.decode(&encoded, &encoded.droplets)
            .expect(&format!("Fountain bs={} decode failed", bs));
        assert_eq!(sha256_hex(&decoded), checksum,
            "Fountain roundtrip failed for block_size={}", bs);
    }
}

#[test]
fn transcoder_large_input() {
    let t = Transcoder::new();
    let data = random_bytes(65536, 88888);
    let checksum = sha256_hex(&data);

    let encoded = t.encode(&data);
    assert_eq!(encoded.length, data.len() * 4);

    let decoded = t.decode(&encoded.sequence, encoded.rotation_key, encoded.original_length);
    assert_eq!(sha256_hex(&decoded), checksum);
}

#[test]
fn pipeline_larger_dataset() {
    // ~4KB of structured data through the full pipeline
    let mut data = sample_csv();
    data.extend(sample_json());
    data.extend(sample_sql());
    let checksum = sha256_hex(&data);

    let mut pipeline = HelixPipeline::new(PipelineConfig::default());
    pipeline.encode(&data, "combined.dat", None);
    let decode = pipeline.decode(None).unwrap();

    assert!(decode.success);
    assert_eq!(sha256_hex(&decode.recovered_data.unwrap()), checksum);
}

// ═══════════════════════════════════════════════════════════════════
//  18. COMPRESSOR hex_sha256 UTILITY TEST
// ═══════════════════════════════════════════════════════════════════

#[test]
fn hex_sha256_matches_independent_impl() {
    let data = b"SHA-256 cross-verification test data";
    let from_crate = hex_sha256(data);
    let independent = sha256_hex(data);
    assert_eq!(from_crate, independent,
        "hex_sha256 from compressor module must match independent SHA-256 computation");
}

#[test]
fn hex_sha256_empty() {
    let empty_hash = hex_sha256(b"");
    // Known SHA-256 of empty input
    assert_eq!(
        empty_hash,
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
}

// ═══════════════════════════════════════════════════════════════════
//  19. FASTA-BASED DECODE (STANDALONE)
// ═══════════════════════════════════════════════════════════════════

#[test]
fn pipeline_fasta_decode_roundtrip() {
    let data = b"FASTA-based standalone decode roundtrip test data.".to_vec();
    let checksum_before = sha256_hex(&data);

    let mut pipeline = HelixPipeline::new(PipelineConfig::default());
    let _encode_out = pipeline.encode(&data, "fasta_rt.txt", None);

    // Get the full FASTA from the pipeline state
    let full_fasta = pipeline.last_encode.as_ref()
        .expect("should have encode state")
        .full_fasta_content.clone();

    assert!(!full_fasta.is_empty());
    assert!(full_fasta.contains(";HELIX-CORE"));

    // Decode from FASTA
    let fasta_decode = pipeline.decode_from_fasta(&full_fasta, None)
        .expect("FASTA decode should succeed");

    assert!(fasta_decode.success, "FASTA decode should succeed");
    assert_eq!(fasta_decode.original_filename, "fasta_rt.txt");
    let recovered = fasta_decode.recovered_data.expect("should have data");
    assert_eq!(sha256_hex(&recovered), checksum_before,
        "FASTA-decoded data must match original");
}

// ═══════════════════════════════════════════════════════════════════
//  20. PIPELINE WITH PROGRESS CALLBACK
// ═══════════════════════════════════════════════════════════════════

#[test]
fn pipeline_with_progress_callback() {
    use std::sync::{Arc, Mutex};

    let data = sample_csv();
    let checksum_before = sha256_hex(&data);

    let progress_log: Arc<Mutex<Vec<(String, u32)>>> = Arc::new(Mutex::new(Vec::new()));
    let log_clone = progress_log.clone();

    let cb = move |phase: &str, pct: u32| {
        log_clone.lock().unwrap().push((phase.to_string(), pct));
    };

    let mut pipeline = HelixPipeline::new(PipelineConfig::default());
    pipeline.encode(&data, "progress.csv", Some(&cb));

    let log = progress_log.lock().unwrap();
    assert!(!log.is_empty(), "Progress callback should have been called");
    // Should reach 100%
    assert!(log.iter().any(|(_, pct)| *pct == 100), "Should reach 100% progress");

    // Decode should also work
    let decode = pipeline.decode(None).unwrap();
    assert!(decode.success);
    assert_eq!(sha256_hex(&decode.recovered_data.unwrap()), checksum_before);
}
