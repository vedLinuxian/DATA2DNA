// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2025 Ved
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as published
// by the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// For commercial licensing, contact: vedcimit@gmail.com

//! Interleaved Reed-Solomon — Cross-Oligo Error Correction
//!
//! PROBLEM: Standard RS(255,223) operates on 255-byte blocks. Each block spans
//! multiple oligos. If one oligo is lost, an entire RS block's worth of symbols
//! is corrupted — that's a burst error potentially exceeding the correction capacity.
//!
//! SOLUTION: Interleaved RS. Instead of applying RS to consecutive bytes,
//! we spread each RS block's symbols across different oligos. Losing one oligo
//! now corrupts only 1 symbol per RS block — trivially correctable.
//!
//! ```text
//!  Standard (bad):     RS Block 1: [oligo1 oligo1 oligo1 ... oligo2 oligo2 ...]
//!                      One lost oligo = burst of N consecutive errors in one block
//!
//!  Interleaved (good): RS Block 1: [oligo1[0] oligo2[0] oligo3[0] ... oligoN[0]]
//!                      RS Block 2: [oligo1[1] oligo2[1] oligo3[1] ... oligoN[1]]
//!                      One lost oligo = 1 error per RS block = always recoverable
//! ```
//!
//! The interleaving depth D determines how many RS blocks are created.
//! With D interleave rows and K oligo columns:
//!   - Each oligo contributes D symbols (one per row)
//!   - Each row is an independent RS codeword
//!   - Row length = number of oligos (K ≤ 255 for GF(2^8))
//!
//! For large data (>255 oligos): we tile the interleave into groups of ≤255 oligos.

use crate::reed_solomon::{RSStats, ReedSolomonCodec};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct InterleavedRSStats {
    pub mode: String,
    pub interleave_depth: usize,
    pub group_size: usize,
    pub num_groups: usize,
    pub data_symbols_per_row: usize,
    pub parity_symbols_per_row: usize,
    pub total_symbols_per_row: usize,
    pub max_correctable_per_row: usize,
    pub max_oligos_recoverable: usize,
    pub total_parity_bytes: usize,
    pub overhead_percent: f64,
    pub blocks_encoded: usize,
    pub blocks_corrected: usize,
    pub total_errors_corrected: usize,
    // Backwards compat with RSStats
    pub data_symbols: usize,
    pub parity_symbols: usize,
    pub total_symbols: usize,
    pub max_correctable_errors: usize,
}

impl InterleavedRSStats {
    pub fn to_rs_stats(&self) -> RSStats {
        RSStats {
            data_symbols: self.data_symbols_per_row,
            parity_symbols: self.parity_symbols_per_row,
            total_symbols: self.total_symbols_per_row,
            max_correctable_errors: self.max_correctable_per_row,
            overhead_percent: self.overhead_percent,
            blocks_encoded: self.blocks_encoded,
            blocks_corrected: self.blocks_corrected,
            total_errors_corrected: self.total_errors_corrected,
        }
    }
}

pub struct InterleavedRS {
    /// How many RS parity symbols per row
    parity_per_row: usize,
    /// Maximum group size (≤ 255 - parity_per_row for GF(2^8))
    max_data_per_row: usize,
}

impl InterleavedRS {
    /// Create with a specified number of parity symbols per interleave row.
    /// Each row can correct up to parity_per_row/2 erased oligos in its group.
    pub fn new(parity_per_row: usize) -> Self {
        assert!(parity_per_row >= 2 && parity_per_row % 2 == 0);
        assert!(parity_per_row <= 128, "Too many parity symbols");
        Self {
            parity_per_row,
            max_data_per_row: 255 - parity_per_row,
        }
    }

    /// Default: 32 parity symbols/row → can correct 16 oligo losses per group
    pub fn default_commercial() -> Self {
        Self::new(32)
    }

    /// Lightweight: 16 parity symbols/row → 8 oligo losses correctable
    pub fn lightweight() -> Self {
        Self::new(16)
    }

    /// Encode: Takes raw data, returns interleaved RS-encoded data + stats.
    ///
    /// The encoded format:
    /// ```text
    /// [HEADER: 11 bytes]
    ///   orig_len: u32 (LE)
    ///   depth:    u16 (LE)  — interleave depth (bytes per symbol column)
    ///   parity:   u8        — parity symbols per row
    ///   groups:   u32 (LE) — number of groups (tiles)
    /// [GROUP 0: rows × total_symbols bytes]
    /// [GROUP 1: ...]
    /// ...
    /// ```
    ///
    /// Each group has `depth` rows, each row is an RS codeword of
    /// `data_symbols + parity_symbols` bytes.
    pub fn encode_buffer(&self, data: &[u8]) -> (Vec<u8>, InterleavedRSStats) {
        let orig_len = data.len();

        // Determine interleave parameters based on data size
        // Depth = how many bytes each "column" contributes
        // We want enough columns to fill RS blocks but not too many groups
        let depth = self.calculate_depth(orig_len);
        let bytes_per_column = depth;

        // Pad data to be divisible by depth
        let padded_len = ((orig_len + bytes_per_column - 1) / bytes_per_column) * bytes_per_column;
        let mut padded = data.to_vec();
        padded.resize(padded_len, 0);

        let num_columns = padded_len / bytes_per_column;

        // Group columns into tiles of max_data_per_row
        let num_groups = (num_columns + self.max_data_per_row - 1) / self.max_data_per_row;

        let codec = ReedSolomonCodec::new(
            self.max_data_per_row.min(num_columns),
            self.parity_per_row,
        );

        // Build header
        let mut output = Vec::new();
        output.extend_from_slice(&(orig_len as u32).to_le_bytes());
        output.extend_from_slice(&(depth as u16).to_le_bytes());
        output.push(self.parity_per_row as u8);
        // Use u32 for num_groups so files up to ~900 GB are supported
        // (u8 would overflow at ~3.6 MB for worst-case depth/parity settings)
        output.extend_from_slice(&(num_groups as u32).to_le_bytes());

        let mut total_blocks = 0usize;
        let mut col_start = 0;

        for _g in 0..num_groups {
            let col_end = (col_start + self.max_data_per_row).min(num_columns);
            let group_cols = col_end - col_start;

            // BUG FIX: Create optimal codec for each group's actual column count.
            // Previously, partial groups (last group with fewer columns) were padded
            // to the full codec size, wasting parity bandwidth on zero-padding.
            // Now each group uses a codec sized to its actual column count.
            let group_codec = if group_cols == codec.data_symbols {
                &codec
            } else {
                // Partial group: create a smaller codec inline
                // group_cols must be >= 1 (ensured by the loop logic)
                &ReedSolomonCodec::new(group_cols.max(2), self.parity_per_row)
            };

            // For each row (depth), collect one byte from each column in this group
            for row in 0..depth {
                let mut row_data = Vec::with_capacity(group_cols);
                for col in col_start..col_end {
                    row_data.push(padded[col * bytes_per_column + row]);
                }
                // Pad to codec's data_symbols size
                row_data.resize(group_codec.data_symbols, 0);

                let codeword = group_codec.encode(&row_data);
                output.extend_from_slice(&codeword);
                total_blocks += 1;
            }

            col_start = col_end;
        }

        let total_parity = total_blocks * self.parity_per_row;
        let data_bytes = total_blocks * codec.data_symbols;

        let stats = InterleavedRSStats {
            mode: format!("Interleaved RS({},{})", codec.total_symbols, codec.data_symbols),
            interleave_depth: depth,
            group_size: self.max_data_per_row.min(num_columns),
            num_groups,
            data_symbols_per_row: codec.data_symbols,
            parity_symbols_per_row: self.parity_per_row,
            total_symbols_per_row: codec.total_symbols,
            max_correctable_per_row: self.parity_per_row / 2,
            max_oligos_recoverable: self.parity_per_row / 2,
            total_parity_bytes: total_parity,
            overhead_percent: if data_bytes > 0 { (total_parity as f64 / data_bytes as f64 * 1000.0).round() / 10.0 } else { 0.0 },
            blocks_encoded: total_blocks,
            blocks_corrected: 0,
            total_errors_corrected: 0,
            data_symbols: codec.data_symbols,
            parity_symbols: self.parity_per_row,
            total_symbols: codec.total_symbols,
            max_correctable_errors: self.parity_per_row / 2,
        };

        (output, stats)
    }

    /// Decode interleaved RS data, correcting errors
    pub fn decode_buffer(&self, encoded: &[u8]) -> Option<(Vec<u8>, InterleavedRSStats)> {
        // Header is 11 bytes: orig_len(4) + depth(2) + parity(1) + num_groups(4)
        if encoded.len() < 11 { return None; }

        let orig_len = u32::from_le_bytes([encoded[0], encoded[1], encoded[2], encoded[3]]) as usize;
        let depth = u16::from_le_bytes([encoded[4], encoded[5]]) as usize;
        let parity = encoded[6] as usize;
        let num_groups = u32::from_le_bytes([encoded[7], encoded[8], encoded[9], encoded[10]]) as usize;

        if depth == 0 || parity == 0 { return None; }

        let max_data = 255 - parity;
        let bytes_per_column = depth;
        let padded_len = ((orig_len + bytes_per_column - 1) / bytes_per_column) * bytes_per_column;
        let num_columns = padded_len / bytes_per_column;

        let _total_per_row = max_data.min(num_columns) + parity;
        let codec = ReedSolomonCodec::new(max_data.min(num_columns), parity);

        let mut pos = 11; // skip 11-byte header
        let mut decoded_matrix: Vec<Vec<u8>> = vec![vec![0u8; depth]; num_columns];
        let mut total_errors = 0usize;
        let mut blocks_corrected = 0usize;
        let mut total_blocks = 0usize;
        let mut col_start = 0;

        for _g in 0..num_groups {
            let col_end = (col_start + max_data).min(num_columns);
            let group_cols = col_end - col_start;

            // BUG FIX: Match the encode-side fix — use optimal codec for partial groups
            let partial_codec;
            let group_codec = if group_cols == codec.data_symbols {
                &codec
            } else {
                partial_codec = ReedSolomonCodec::new(group_cols.max(2), parity);
                &partial_codec
            };

            for row in 0..depth {
                if pos + group_codec.total_symbols > encoded.len() {
                    return None; // truncated
                }

                let codeword = &encoded[pos..pos + group_codec.total_symbols];
                pos += group_codec.total_symbols;
                total_blocks += 1;

                match group_codec.decode(codeword) {
                    Some((data, ne)) => {
                        if ne > 0 { blocks_corrected += 1; total_errors += ne; }
                        for (ci, col) in (col_start..col_end).enumerate() {
                            decoded_matrix[col][row] = data[ci];
                        }
                    }
                    None => return None, // uncorrectable
                }
            }

            col_start = col_end;
        }

        // Reconstruct data from columns
        let mut result = Vec::with_capacity(padded_len);
        for col in 0..num_columns {
            result.extend_from_slice(&decoded_matrix[col]);
        }
        result.truncate(orig_len);

        let stats = InterleavedRSStats {
            mode: format!("Interleaved RS({},{})", codec.total_symbols, codec.data_symbols),
            interleave_depth: depth,
            group_size: max_data.min(num_columns),
            num_groups,
            data_symbols_per_row: codec.data_symbols,
            parity_symbols_per_row: parity,
            total_symbols_per_row: codec.total_symbols,
            max_correctable_per_row: parity / 2,
            max_oligos_recoverable: parity / 2,
            total_parity_bytes: total_blocks * parity,
            overhead_percent: if total_blocks > 0 {
                (total_blocks as f64 * parity as f64 / (total_blocks as f64 * codec.data_symbols as f64) * 1000.0).round() / 10.0
            } else { 0.0 },
            blocks_encoded: total_blocks,
            blocks_corrected,
            total_errors_corrected: total_errors,
            data_symbols: codec.data_symbols,
            parity_symbols: parity,
            total_symbols: codec.total_symbols,
            max_correctable_errors: parity / 2,
        };

        Some((result, stats))
    }

    /// Calculate optimal interleave depth based on data size
    fn calculate_depth(&self, data_len: usize) -> usize {
        if data_len <= 1024 {
            // Small data: shallow interleave
            4
        } else if data_len <= 65536 {
            // Medium: moderate depth
            16
        } else {
            // Large data: deeper interleave for better correction spread
            64
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_interleaved_roundtrip_clean() {
        let irs = InterleavedRS::default_commercial();
        let data = b"Interleaved Reed-Solomon for DNA data storage!".to_vec();
        let (encoded, stats) = irs.encode_buffer(&data);
        assert!(stats.blocks_encoded > 0);
        assert!(stats.overhead_percent > 0.0);
        let (decoded, dec_stats) = irs.decode_buffer(&encoded).unwrap();
        assert_eq!(decoded, data);
        assert_eq!(dec_stats.total_errors_corrected, 0);
    }

    #[test]
    fn test_interleaved_with_corruption() {
        let irs = InterleavedRS::default_commercial();
        let data: Vec<u8> = (0..2000).map(|i| (i % 256) as u8).collect();
        let (mut encoded, stats) = irs.encode_buffer(&data);

        // Corrupt some bytes (simulating partial oligo loss)
        // The interleaving should spread errors across RS blocks
        let header_size = 11; // FIX: header is orig_len(4) + depth(2) + parity(1) + num_groups(4) = 11
        let row_size = stats.total_symbols_per_row;
        // Corrupt 1 byte per each of the first 5 rows (within correction capability)
        for r in 0..5.min(stats.blocks_encoded) {
            let offset = header_size + r * row_size + 3; // position 3 in each row
            if offset < encoded.len() {
                encoded[offset] ^= 0xFF;
            }
        }

        let result = irs.decode_buffer(&encoded);
        assert!(result.is_some(), "Should correct scattered errors");
        let (decoded, dec_stats) = result.unwrap();
        assert_eq!(decoded, data);
        assert!(dec_stats.total_errors_corrected > 0);
    }

    #[test]
    fn test_interleaved_large_data() {
        let irs = InterleavedRS::default_commercial();
        let data: Vec<u8> = (0..10000).map(|i| (i % 256) as u8).collect();
        let (encoded, stats) = irs.encode_buffer(&data);
        println!("10KB: {} groups, {} depth, {} blocks, {:.1}% overhead",
            stats.num_groups, stats.interleave_depth, stats.blocks_encoded, stats.overhead_percent);
        let (decoded, _) = irs.decode_buffer(&encoded).unwrap();
        assert_eq!(decoded, data);
    }

    #[test]
    fn test_lightweight_mode() {
        let irs = InterleavedRS::lightweight();
        let data = b"Short test data for lightweight RS".to_vec();
        let (encoded, stats) = irs.encode_buffer(&data);
        assert_eq!(stats.parity_symbols_per_row, 16);
        assert_eq!(stats.max_correctable_per_row, 8);
        let (decoded, _) = irs.decode_buffer(&encoded).unwrap();
        assert_eq!(decoded, data);
    }
}
