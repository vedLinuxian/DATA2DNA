// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2025 Ved
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as published
// by the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// For commercial licensing, contact: vedcimit@gmail.com

//! Fountain Codes: LT (Luby Transform) codes for redundancy
//!
//! Splits data into blocks and creates XOR-encoded droplets.
//! Supports both Ideal Soliton and **Robust Soliton** distributions.
//! The Robust Soliton Distribution (Luby 2002) provides dramatically
//! better decoding performance than Ideal Soliton — this is the same
//! distribution used in the DNA Fountain paper (Erlich & Zielinski, Science 2017).
//!
//! Surviving droplets allow full recovery via the peeling (belief propagation) decoder.

use rand::prelude::*;
use rand::rngs::StdRng;
use rayon::prelude::*;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct Droplet {
    pub id: usize,
    pub seed: u64,
    pub degree: usize,
    pub block_indices: Vec<usize>,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct FountainEncoded {
    pub droplets: Vec<Droplet>,
    pub num_blocks: usize,
    pub block_size: usize,
    pub original_length: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct FountainStats {
    pub num_blocks: usize,
    pub num_droplets: usize,
    pub redundancy_ratio: f64,
    pub overhead_percent: f64,
    pub total_encoded_size: usize,
    pub distribution: String,
    pub decode_failure_probability: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SolitonDistribution {
    Ideal,
    Robust { c: f64, delta: f64 },
}

impl Default for SolitonDistribution {
    fn default() -> Self {
        // DNA Fountain paper (Erlich & Zielinski, Science 2017): c=0.025, δ=0.001
        SolitonDistribution::Robust { c: 0.025, delta: 0.001 }
    }
}

pub struct FountainCodec {
    pub block_size: usize,
    pub redundancy: f64,
    pub seed: u64,
    pub distribution: SolitonDistribution,
}

impl FountainCodec {
    pub fn new(block_size: usize, redundancy: f64, seed: u64) -> Self {
        Self {
            block_size,
            redundancy,
            seed,
            distribution: SolitonDistribution::default(),
        }
    }

    pub fn with_distribution(block_size: usize, redundancy: f64, seed: u64, dist: SolitonDistribution) -> Self {
        Self {
            block_size,
            redundancy,
            seed,
            distribution: dist,
        }
    }

    /// Encode data into fountain droplets.
    /// Hybrid approach: systematic degree-1 droplets (for baseline coverage)
    /// count against the total droplet budget, with remaining budget used for
    /// fountain-coded XOR droplets from the Robust Soliton distribution.
    pub fn encode(&self, data: &[u8]) -> FountainEncoded {
        let block_size = self.block_size;
        if block_size == 0 {
            return FountainEncoded {
                droplets: Vec::new(),
                num_blocks: 0,
                block_size: 0,
                original_length: data.len(),
            };
        }

        // Pad data to multiple of block_size
        let mut padded = data.to_vec();
        while padded.len() % block_size != 0 {
            padded.push(0);
        }

        let num_blocks = padded.len() / block_size;
        let blocks: Vec<&[u8]> = padded.chunks(block_size).collect();
        let num_droplets =
            ((num_blocks as f64) * self.redundancy).ceil() as usize;

        // Pre-compute CDF for the chosen distribution
        let cdf = match self.distribution {
            SolitonDistribution::Ideal => build_ideal_soliton_cdf(num_blocks),
            SolitonDistribution::Robust { c, delta } => build_robust_soliton_cdf(num_blocks, c, delta),
        };

        let mut rng = StdRng::seed_from_u64(self.seed);
        let mut droplets = Vec::with_capacity(num_droplets);

        // Phase 1: Systematic degree-1 droplets for baseline coverage
        // These count against the total budget (not extra)
        let systematic_count = num_blocks.min(num_droplets);
        for i in 0..systematic_count {
            let seed = rng.gen();
            droplets.push(Droplet {
                id: i,
                seed,
                degree: 1,
                block_indices: vec![i],
                data: blocks[i].to_vec(),
            });
        }

        // Phase 2: Fill remaining budget with fountain-coded XOR droplets
        // PERF: Pre-generate seeds sequentially (RNG is sequential), then
        // parallelize the compute-heavy XOR work with Rayon.
        let xor_count = num_droplets - systematic_count;
        let seeds: Vec<u64> = (0..xor_count).map(|_| rng.gen()).collect();

        // Parallel XOR droplet generation for large batches
        if xor_count > 64 {
            let xor_droplets: Vec<Droplet> = seeds.par_iter().enumerate().map(|(j, &droplet_seed)| {
                let mut d_rng = StdRng::seed_from_u64(droplet_seed);
                let degree = sample_from_cdf(&cdf, &mut d_rng);
                let indices = sample_indices(num_blocks, degree, &mut d_rng);

                let mut xored = vec![0u8; block_size];
                for &idx in &indices {
                    xor_in_place(&mut xored, blocks[idx]);
                }

                Droplet {
                    id: systematic_count + j,
                    seed: droplet_seed,
                    degree,
                    block_indices: indices,
                    data: xored,
                }
            }).collect();
            droplets.extend(xor_droplets);
        } else {
            // Sequential path for small batches (avoids Rayon overhead)
            for (j, &droplet_seed) in seeds.iter().enumerate() {
                let mut d_rng = StdRng::seed_from_u64(droplet_seed);
                let degree = sample_from_cdf(&cdf, &mut d_rng);
                let indices = sample_indices(num_blocks, degree, &mut d_rng);

                let mut xored = vec![0u8; block_size];
                for &idx in &indices {
                    xor_in_place(&mut xored, blocks[idx]);
                }

                droplets.push(Droplet {
                    id: systematic_count + j,
                    seed: droplet_seed,
                    degree,
                    block_indices: indices,
                    data: xored,
                });
            }
        }

        FountainEncoded {
            droplets,
            num_blocks,
            block_size,
            original_length: data.len(),
        }
    }

    /// Decode surviving droplets back to original data (peeling decoder)
    pub fn decode(
        &self,
        encoded: &FountainEncoded,
        surviving: &[Droplet],
    ) -> Option<Vec<u8>> {
        let num_blocks = encoded.num_blocks;
        let block_size = encoded.block_size;
        if num_blocks == 0 {
            return if encoded.original_length == 0 {
                Some(Vec::new())
            } else {
                None
            };
        }
        if block_size == 0 {
            return None;
        }

        // Initialize decoder state
        let mut decoded_blocks: Vec<Option<Vec<u8>>> =
            vec![None; num_blocks];
        let mut active_droplets: Vec<(Vec<usize>, Vec<u8>)> = surviving
            .iter()
            .map(|d| (d.block_indices.clone(), d.data.clone()))
            .collect();

        // Peeling decoder: iterative belief propagation
        // Peeling converges in at most k iterations by construction.
        // Add 10% margin for edge cases with duplicate droplets.
        let max_iterations = num_blocks + (num_blocks / 10).max(10);
        let mut changed = true;
        let mut iterations = 0;
        while changed && iterations < max_iterations {
            changed = false;
            iterations += 1;

            // Find degree-1 droplets
            let mut new_solved = Vec::new();
            for (indices, data) in &active_droplets {
                if indices.len() == 1 {
                    let block_idx = indices[0];
                    // Corrupted droplets from sequencing errors may reference block
                    // indices beyond num_blocks — clamp silently and let the
                    // peeling decoder recover from remaining valid droplets
                    if block_idx < num_blocks && decoded_blocks[block_idx].is_none() {
                        decoded_blocks[block_idx] =
                            Some(data[..block_size.min(data.len())].to_vec());
                        new_solved.push(block_idx);
                        changed = true;
                    }
                }
            }

            // XOR solved blocks out of remaining droplets
            for block_idx in &new_solved {
                if let Some(ref solved_data) = decoded_blocks[*block_idx] {
                    for (indices, data) in &mut active_droplets {
                        if let Some(pos) =
                            indices.iter().position(|&i| i == *block_idx)
                        {
                            xor_in_place(data, solved_data);
                            indices.remove(pos);
                        }
                    }
                }
            }
        }

        // Check if all blocks decoded
        let all_decoded = decoded_blocks.iter().all(|b| b.is_some());
        if !all_decoded {
            return None;
        }

        // Reassemble
        let mut output = Vec::with_capacity(encoded.original_length);
        for block in &decoded_blocks {
            if let Some(ref b) = block {
                output.extend_from_slice(b);
            }
        }
        output.truncate(encoded.original_length);
        Some(output)
    }

    pub fn get_stats(&self, encoded: &FountainEncoded) -> FountainStats {
        let total_size: usize =
            encoded.droplets.iter().map(|d| d.data.len()).sum();
        let redundancy_ratio = if encoded.num_blocks == 0 {
            0.0
        } else {
            (encoded.droplets.len() as f64 / encoded.num_blocks as f64 * 100.0).round() / 100.0
        };
        let overhead_percent = if encoded.num_blocks == 0 {
            0.0
        } else {
            ((encoded.droplets.len() as f64 / encoded.num_blocks as f64 - 1.0) * 100.0 * 10.0)
                .round()
                / 10.0
        };
        let dist_name = match self.distribution {
            SolitonDistribution::Ideal => "Ideal Soliton".to_string(),
            SolitonDistribution::Robust { c, delta } =>
                format!("Robust Soliton (c={}, δ={})", c, delta),
        };
        let failure_prob = match self.distribution {
            SolitonDistribution::Ideal => 0.5, // Ideal has ~50% failure rate
            SolitonDistribution::Robust { delta, .. } => delta,
        };
        FountainStats {
            num_blocks: encoded.num_blocks,
            num_droplets: encoded.droplets.len(),
            redundancy_ratio,
            overhead_percent,
            total_encoded_size: total_size,
            distribution: dist_name,
            decode_failure_probability: failure_prob,
        }
    }
}

/// Build CDF for Ideal Soliton Distribution ρ(d)
///
/// ρ(1) = 1/k, ρ(d) = 1/(d*(d-1)) for d = 2..k
fn build_ideal_soliton_cdf(k: usize) -> Vec<f64> {
    if k == 0 {
        return vec![1.0];
    }
    let mut pmf = vec![0.0; k + 1]; // index 0 unused, 1..=k
    pmf[1] = 1.0 / k as f64;
    for d in 2..=k {
        pmf[d] = 1.0 / (d as f64 * (d as f64 - 1.0));
    }
    // Build CDF
    let mut cdf = vec![0.0; k + 1];
    let mut cumulative = 0.0;
    for d in 1..=k {
        cumulative += pmf[d];
        cdf[d] = cumulative;
    }
    // Normalize to exactly 1.0 to avoid floating point drift
    cdf[k] = 1.0;
    cdf
}

/// Build CDF for Robust Soliton Distribution μ(d) = (ρ(d) + τ(d)) / β
///
/// From Luby (2002): τ(i) defines the "spike" that dramatically improves
/// decoding success probability from ~50% (Ideal) to 1 - δ (Robust).
///
/// Parameters:
/// - k: number of source blocks
/// - c: free parameter controlling spike height (typical: 0.01 - 0.2)
/// - delta: target decode failure probability (typical: 0.01 - 0.1)
fn build_robust_soliton_cdf(k: usize, c: f64, delta: f64) -> Vec<f64> {
    if k == 0 {
        return vec![1.0];
    }

    let k_f = k as f64;
    // R = c * ln(k/delta) * sqrt(k)
    let r = (c * (k_f / delta).ln() * k_f.sqrt()).max(1.0);
    let threshold = (k_f / r).floor() as usize;

    // Build ideal soliton PMF
    let mut pmf = vec![0.0; k + 1];
    pmf[1] = 1.0 / k_f;
    for d in 2..=k {
        pmf[d] = 1.0 / (d as f64 * (d as f64 - 1.0));
    }

    // Add τ(d) — the robust "spike"
    // τ(i) = R / (i * k) for i = 1..threshold-1
    // τ(threshold) = R * ln(R/delta) / k
    // τ(i) = 0 for i > threshold
    for d in 1..=k.min(threshold.saturating_sub(1).max(1)) {
        if d <= k && d < threshold {
            pmf[d] += r / (d as f64 * k_f);
        }
    }
    if threshold >= 1 && threshold <= k {
        pmf[threshold] += r * (r / delta).ln() / k_f;
    }

    // Normalize: β = Σ (ρ(d) + τ(d))
    let beta: f64 = pmf[1..=k].iter().sum();
    if beta > 0.0 {
        for d in 1..=k {
            pmf[d] /= beta;
        }
    }

    // Build CDF
    let mut cdf = vec![0.0; k + 1];
    let mut cumulative = 0.0;
    for d in 1..=k {
        cumulative += pmf[d];
        cdf[d] = cumulative;
    }
    cdf[k] = 1.0;
    cdf
}

/// Sample a degree from a precomputed CDF using binary search
fn sample_from_cdf(cdf: &[f64], rng: &mut StdRng) -> usize {
    let p: f64 = rng.gen();
    // Binary search for smallest d where cdf[d] >= p
    let k = cdf.len() - 1;
    if k == 0 {
        return 1;
    }
    // Linear scan for small k, binary search for large k
    if k <= 64 {
        for d in 1..=k {
            if cdf[d] >= p {
                return d;
            }
        }
        return k;
    }
    // Binary search: find first index in 1..=k where cdf[index] >= p
    let mut lo = 1usize;
    let mut hi = k;
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if cdf[mid] < p {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    lo.max(1).min(k)
}

/// Sample `count` unique indices from 0..n
/// PERF: Uses rejection sampling for small count/n ratios (avoids allocating n-element vec)
fn sample_indices(n: usize, count: usize, rng: &mut StdRng) -> Vec<usize> {
    let count = count.min(n);
    if count == 0 {
        return Vec::new();
    }
    if count == n {
        return (0..n).collect();
    }
    if count == 1 {
        return vec![rng.gen_range(0..n)];
    }

    // For small count relative to n, use rejection sampling (no allocation of n-element vec)
    if count * 4 < n {
        let mut set = std::collections::HashSet::with_capacity(count);
        while set.len() < count {
            set.insert(rng.gen_range(0..n));
        }
        let mut indices: Vec<usize> = set.into_iter().collect();
        indices.sort_unstable();
        return indices;
    }

    // For larger count, use Fisher-Yates partial shuffle
    let mut available: Vec<usize> = (0..n).collect();
    let mut indices = Vec::with_capacity(count);
    for i in 0..count {
        let idx = rng.gen_range(i..available.len());
        available.swap(i, idx);
        indices.push(available[i]);
    }
    indices.sort_unstable();
    indices
}

/// XOR b into a in-place.
/// PERF: Process 8 bytes at a time for auto-vectorization.
/// The compiler will emit SIMD (SSE2/AVX2/NEON) instructions for the u64 path.
#[inline]
fn xor_in_place(a: &mut [u8], b: &[u8]) {
    let len = a.len().min(b.len());
    // Process 8 bytes at a time (auto-vectorizable)
    let chunks = len / 8;
    let a_u64 = unsafe { std::slice::from_raw_parts_mut(a.as_mut_ptr() as *mut u64, chunks) };
    let b_u64 = unsafe { std::slice::from_raw_parts(b.as_ptr() as *const u64, chunks) };
    for i in 0..chunks {
        a_u64[i] ^= b_u64[i];
    }
    // Handle remaining bytes
    for i in (chunks * 8)..len {
        a[i] ^= b[i];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_decode_no_loss() {
        let codec = FountainCodec::new(64, 2.5, 42);
        let data = b"Fountain codes are amazing! They enable DNA data recovery.".to_vec();
        let encoded = codec.encode(&data);
        let decoded = codec.decode(&encoded, &encoded.droplets);
        assert_eq!(decoded.unwrap(), data);
    }

    #[test]
    fn test_encode_decode_with_loss() {
        let codec = FountainCodec::new(32, 3.0, 42);
        let data: Vec<u8> = (0..256).map(|i| (i % 256) as u8).collect();
        let encoded = codec.encode(&data);
        // Lose 30% of droplets
        let mut rng = StdRng::seed_from_u64(99);
        let surviving: Vec<Droplet> = encoded
            .droplets
            .iter()
            .filter(|_| rng.gen::<f64>() > 0.30)
            .cloned()
            .collect();
        let decoded = codec.decode(&encoded, &surviving);
        assert!(decoded.is_some());
        assert_eq!(decoded.unwrap(), data);
    }

    #[test]
    fn test_ideal_soliton_cdf() {
        let cdf = build_ideal_soliton_cdf(10);
        assert_eq!(cdf.len(), 11);
        assert!((cdf[1] - 0.1).abs() < 1e-10); // ρ(1) = 1/10
        assert!((cdf[10] - 1.0).abs() < 1e-10); // Final CDF = 1.0
        // CDF must be monotonically non-decreasing
        for i in 1..cdf.len() - 1 {
            assert!(cdf[i + 1] >= cdf[i]);
        }
    }

    #[test]
    fn test_robust_soliton_cdf() {
        let cdf = build_robust_soliton_cdf(100, 0.1, 0.05);
        assert_eq!(cdf.len(), 101);
        assert!((cdf[100] - 1.0).abs() < 1e-10);
        // Robust has more mass on degree-1 than Ideal
        let ideal_cdf = build_ideal_soliton_cdf(100);
        assert!(cdf[1] > ideal_cdf[1]); // Robust pumps up low degrees
    }

    #[test]
    fn test_robust_soliton_decode_resilience() {
        // Robust Soliton should handle 40% loss with 3x redundancy
        let codec = FountainCodec::with_distribution(
            32, 3.0, 42,
            SolitonDistribution::Robust { c: 0.1, delta: 0.05 },
        );
        let data: Vec<u8> = (0..512).map(|i| (i % 256) as u8).collect();
        let encoded = codec.encode(&data);
        let mut rng = StdRng::seed_from_u64(77);
        let surviving: Vec<Droplet> = encoded
            .droplets
            .iter()
            .filter(|_| rng.gen::<f64>() > 0.40)
            .cloned()
            .collect();
        let decoded = codec.decode(&encoded, &surviving);
        assert!(decoded.is_some());
        assert_eq!(decoded.unwrap(), data);
    }

    #[test]
    fn test_stats_distribution_field() {
        let codec = FountainCodec::new(64, 2.0, 42);
        let data = vec![0u8; 128];
        let encoded = codec.encode(&data);
        let stats = codec.get_stats(&encoded);
        assert!(stats.distribution.contains("Robust"));
        assert!(stats.decode_failure_probability < 1.0);
    }

    #[test]
    fn test_sample_from_cdf_bounds() {
        let cdf = build_ideal_soliton_cdf(50);
        let mut rng = StdRng::seed_from_u64(12345);
        for _ in 0..1000 {
            let d = sample_from_cdf(&cdf, &mut rng);
            assert!(d >= 1 && d <= 50);
        }
    }
}
