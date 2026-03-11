// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2025 Ved
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as published
// by the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// For commercial licensing, contact: vedcimit@gmail.com

//! FASTA I/O: Generate standard FASTA format oligos from DNA sequence.
//! Now includes FASTA parsing for standalone decode-from-FASTA feature.

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct FastaRecord {
    pub id: String,
    pub sequence: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct FastaStats {
    pub num_oligos: usize,
    pub oligo_length: usize,
    pub total_bases: usize,
    pub avg_gc: f64,
}

/// Metadata embedded in FASTA header comment for standalone decode
#[derive(Debug, Clone, Serialize)]
pub struct FastaMetadata {
    pub rotation_key: u8,
    pub original_length: usize,
    pub rs_enabled: bool,
    pub use_interleaved_rs: bool,
    pub compression_enabled: bool,
    pub use_hypercompress: bool,
    pub original_filename: String,
    pub original_checksum: String,
    pub block_size: usize,
    pub redundancy: f64,
    pub seed: u64,
}

pub struct FastaIO;

impl FastaIO {
    pub fn new() -> Self {
        FastaIO
    }

    /// Split DNA sequence into oligos of given length
    pub fn create_oligos(
        &self,
        sequence: &str,
        oligo_length: usize,
    ) -> Vec<FastaRecord> {
        if oligo_length == 0 {
            return Vec::new();
        }
        let chars: Vec<char> = sequence.chars().collect();
        let mut oligos = Vec::new();

        for (i, chunk) in chars.chunks(oligo_length).enumerate() {
            let seq: String = chunk.iter().collect();
            oligos.push(FastaRecord {
                id: format!("HELIX_{:06}|len={}|pos={}", i + 1, seq.len(), i * oligo_length),
                sequence: seq,
            });
        }

        oligos
    }

    /// Generate standard FASTA format string with Helix metadata header
    pub fn generate_fasta_with_metadata(
        &self,
        oligos: &[FastaRecord],
        metadata: &FastaMetadata,
    ) -> String {
        let mut output = String::new();
        // Metadata header as FASTA comments (lines starting with ;)
        output.push_str(";HELIX-CORE-v5.0 DNA Storage Archive\n");
        output.push_str(&format!(";META:rotation_key={}\n", metadata.rotation_key));
        output.push_str(&format!(";META:original_length={}\n", metadata.original_length));
        output.push_str(&format!(";META:rs_enabled={}\n", metadata.rs_enabled));
        output.push_str(&format!(";META:use_interleaved_rs={}\n", metadata.use_interleaved_rs));
        output.push_str(&format!(";META:compression_enabled={}\n", metadata.compression_enabled));
        output.push_str(&format!(";META:use_hypercompress={}\n", metadata.use_hypercompress));
        output.push_str(&format!(";META:original_filename={}\n", metadata.original_filename));
        output.push_str(&format!(";META:original_checksum={}\n", metadata.original_checksum));
        output.push_str(&format!(";META:block_size={}\n", metadata.block_size));
        output.push_str(&format!(";META:redundancy={}\n", metadata.redundancy));
        output.push_str(&format!(";META:seed={}\n", metadata.seed));
        output.push_str(&format!(";META:num_oligos={}\n", oligos.len()));
        output.push_str(";END-META\n");

        // Standard FASTA records
        for rec in oligos {
            output.push('>');
            output.push_str(&rec.id);
            output.push('\n');
            for line in rec.sequence.as_bytes().chunks(80) {
                output.push_str(&String::from_utf8_lossy(line));
                output.push('\n');
            }
        }
        output
    }

    /// Generate standard FASTA format string (legacy, no metadata)
    pub fn generate_fasta_string(&self, oligos: &[FastaRecord]) -> String {
        let mut output = String::new();
        for rec in oligos {
            output.push('>');
            output.push_str(&rec.id);
            output.push('\n');
            // Wrap at 80 chars
            for line in rec.sequence.as_bytes().chunks(80) {
                output.push_str(&String::from_utf8_lossy(line));
                output.push('\n');
            }
        }
        output
    }

    /// Parse FASTA content into records and optional metadata
    pub fn parse_fasta(content: &str) -> (Vec<FastaRecord>, Option<FastaMetadata>) {
        let mut records = Vec::new();
        let mut metadata: Option<FastaMetadata> = None;

        // Parse metadata from comment lines
        let mut meta_rotation_key: Option<u8> = None;
        let mut meta_original_length: Option<usize> = None;
        let mut meta_rs_enabled = true;
        let mut meta_interleaved_rs = true;
        let mut meta_compression = true;
        let mut meta_hypercompress = true;
        let mut meta_filename = String::new();
        let mut meta_checksum = String::new();
        let mut meta_block_size = 64usize;
        let mut meta_redundancy = 2.0f64; // BUG FIX: Match PipelineConfig::default() which is 2.0
        let mut meta_seed = 42u64;
        let mut has_meta = false;

        let mut current_id = String::new();
        let mut current_seq = String::new();
        let mut in_record = false;

        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }

            // Comment / metadata lines
            if trimmed.starts_with(';') {
                if trimmed.contains("HELIX-CORE") {
                    has_meta = true;
                }
                if let Some(meta_part) = trimmed.strip_prefix(";META:") {
                    if let Some((key, val)) = meta_part.split_once('=') {
                        match key {
                            "rotation_key" => meta_rotation_key = val.parse().ok(),
                            "original_length" => meta_original_length = val.parse().ok(),
                            "rs_enabled" => meta_rs_enabled = val.parse().unwrap_or(true),
                            "use_interleaved_rs" => meta_interleaved_rs = val.parse().unwrap_or(true),
                            "compression_enabled" => meta_compression = val.parse().unwrap_or(true),
                            "use_hypercompress" => meta_hypercompress = val.parse().unwrap_or(true),
                            "original_filename" => meta_filename = val.to_string(),
                            "original_checksum" => meta_checksum = val.to_string(),
                            "block_size" => meta_block_size = val.parse().unwrap_or(64),
                            "redundancy" => meta_redundancy = val.parse().unwrap_or(1.5),
                            "seed" => meta_seed = val.parse().unwrap_or(42),
                            _ => {}
                        }
                    }
                }
                continue;
            }

            // FASTA header line
            if trimmed.starts_with('>') {
                if in_record && !current_seq.is_empty() {
                    records.push(FastaRecord {
                        id: current_id.clone(),
                        sequence: current_seq.clone(),
                    });
                }
                current_id = trimmed[1..].to_string();
                current_seq.clear();
                in_record = true;
                continue;
            }

            // Sequence line — only keep valid DNA bases
            if in_record {
                for c in trimmed.chars() {
                    if matches!(c, 'A' | 'C' | 'G' | 'T' | 'a' | 'c' | 'g' | 't') {
                        current_seq.push(c.to_ascii_uppercase());
                    }
                }
            }
        }

        // Push last record
        if in_record && !current_seq.is_empty() {
            records.push(FastaRecord {
                id: current_id,
                sequence: current_seq,
            });
        }

        if has_meta && meta_rotation_key.is_some() && meta_original_length.is_some() {
            metadata = Some(FastaMetadata {
                rotation_key: meta_rotation_key.unwrap(),
                original_length: meta_original_length.unwrap(),
                rs_enabled: meta_rs_enabled,
                use_interleaved_rs: meta_interleaved_rs,
                compression_enabled: meta_compression,
                use_hypercompress: meta_hypercompress,
                original_filename: meta_filename,
                original_checksum: meta_checksum,
                block_size: meta_block_size,
                redundancy: meta_redundancy,
                seed: meta_seed,
            });
        }

        (records, metadata)
    }

    pub fn get_stats(&self, oligos: &[FastaRecord]) -> FastaStats {
        let total_bases: usize = oligos.iter().map(|o| o.sequence.len()).sum();
        let oligo_len = oligos.first().map_or(0, |o| o.sequence.len());
        let gc_total: usize = oligos
            .iter()
            .flat_map(|o| o.sequence.chars())
            .filter(|&c| c == 'G' || c == 'C')
            .count();
        let avg_gc = if total_bases > 0 {
            gc_total as f64 / total_bases as f64
        } else {
            0.0
        };

        FastaStats {
            num_oligos: oligos.len(),
            oligo_length: oligo_len,
            total_bases,
            avg_gc,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_oligos() {
        let fasta = FastaIO::new();
        let seq = "ACGTACGTACGTACGTACGTACGT";
        let oligos = fasta.create_oligos(seq, 8);
        assert_eq!(oligos.len(), 3);
        assert_eq!(oligos[0].sequence, "ACGTACGT");
    }

    #[test]
    fn test_parse_fasta() {
        let content = ">record1\nACGTACGT\n>record2\nTTTTAAAA\n";
        let (records, meta) = FastaIO::parse_fasta(content);
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].sequence, "ACGTACGT");
        assert_eq!(records[1].sequence, "TTTTAAAA");
        assert!(meta.is_none());
    }

    #[test]
    fn test_parse_fasta_with_metadata() {
        let content = ";HELIX-CORE-v5.0 DNA Storage Archive\n\
                       ;META:rotation_key=42\n\
                       ;META:original_length=1024\n\
                       ;META:rs_enabled=true\n\
                       ;META:compression_enabled=true\n\
                       ;META:original_filename=test.txt\n\
                       ;END-META\n\
                       >HELIX_000001|len=200|idx=0\nACGTACGT\n";
        let (records, meta) = FastaIO::parse_fasta(content);
        assert_eq!(records.len(), 1);
        assert!(meta.is_some());
        let m = meta.unwrap();
        assert_eq!(m.rotation_key, 42);
        assert_eq!(m.original_length, 1024);
        assert_eq!(m.original_filename, "test.txt");
    }

    #[test]
    fn test_roundtrip_fasta_with_metadata() {
        let fasta = FastaIO::new();
        let oligos = vec![
            FastaRecord { id: "HELIX_000001|len=8|idx=0".into(), sequence: "ACGTACGT".into() },
            FastaRecord { id: "HELIX_000002|len=8|idx=1".into(), sequence: "TTTTAAAA".into() },
        ];
        let metadata = FastaMetadata {
            rotation_key: 100,
            original_length: 2048,
            rs_enabled: true,
            use_interleaved_rs: true,
            compression_enabled: true,
            use_hypercompress: true,
            original_filename: "data.bin".into(),
            original_checksum: "abc123".into(),
            block_size: 64,
            redundancy: 1.5,
            seed: 42,
        };
        let fasta_str = fasta.generate_fasta_with_metadata(&oligos, &metadata);
        let (parsed_records, parsed_meta) = FastaIO::parse_fasta(&fasta_str);
        assert_eq!(parsed_records.len(), 2);
        assert_eq!(parsed_records[0].sequence, "ACGTACGT");
        let m = parsed_meta.unwrap();
        assert_eq!(m.rotation_key, 100);
        assert_eq!(m.original_length, 2048);
        assert_eq!(m.original_filename, "data.bin");
    }
}
