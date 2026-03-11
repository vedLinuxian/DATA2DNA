//! Consensus Decoder: Combines fountain decode + reverse transcode
//!
//! Uses the peeling decoder for fountain code recovery.

use crate::fountain::{Droplet, FountainCodec, FountainEncoded};
use serde::Serialize;

#[derive(Debug, Clone, Serialize, Default)]
pub struct DecodeStats {
    pub strategy: String,
    pub iterations: usize,
    pub blocks_recovered: usize,
    pub total_blocks: usize,
}

pub struct ConsensusDecoder {
    stats: DecodeStats,
}

impl ConsensusDecoder {
    pub fn new() -> Self {
        Self {
            stats: DecodeStats::default(),
        }
    }

    /// Full decode pipeline: fountain decode → reverse transcode
    pub fn decode_pipeline(
        &mut self,
        encoded: &FountainEncoded,
        surviving: &[Droplet],
        _rotation_key: u8,
        _original_length: usize,
    ) -> Option<Vec<u8>> {
        self.stats = DecodeStats {
            strategy: "Peeling BP + Reverse Transcode".to_string(),
            iterations: 0,
            blocks_recovered: 0,
            total_blocks: encoded.num_blocks,
        };

        // Fountain decode (peeling decoder)
        let codec = FountainCodec::new(
            encoded.block_size,
            1.0, // Not used for decoding
            0,
        );

        let raw_data = codec.decode(encoded, surviving)?;

        self.stats.blocks_recovered = encoded.num_blocks;

        // Reverse transcode: DNA → binary is not needed here
        // because fountain codes operate on the raw binary data, not DNA.
        // The data stored in droplets IS the post-transcode data (compressed bytes).
        // So we just return the raw recovered data directly.
        // The pipeline handles decompression separately.

        Some(raw_data)
    }

    pub fn get_stats(&self) -> DecodeStats {
        self.stats.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fountain::FountainCodec;

    #[test]
    fn test_consensus_decode() {
        let codec = FountainCodec::new(64, 2.5, 42);
        let data = b"Consensus decoder test data for Project Helix-Core!".to_vec();
        let encoded = codec.encode(&data);

        let mut decoder = ConsensusDecoder::new();
        let result = decoder.decode_pipeline(
            &encoded,
            &encoded.droplets,
            0,
            data.len(),
        );

        assert!(result.is_some());
        assert_eq!(result.unwrap(), data);
    }
}
