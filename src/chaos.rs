// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2025 Ved
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as published
// by the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// For commercial licensing, contact: vedcimit@gmail.com

//! Chaos Matrix: DNA degradation simulation
//!
//! Simulates real-world storage damage: droplet loss, base substitutions,
//! deletions, and insertions.

use crate::fountain::Droplet;
use rand::prelude::*;
use rand::rngs::StdRng;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct ChaosStats {
    pub total_droplets: usize,
    pub lost_droplets: usize,
    pub surviving_droplets: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct MutationSummary {
    pub total_mutations: usize,
    pub substitutions: usize,
    pub deletions: usize,
    pub insertions: usize,
}

pub struct ChaosMatrix {
    pub deletion_rate: f64,
    pub substitution_rate: f64,
    pub insertion_rate: f64,
    pub seed: u64,
}

impl ChaosMatrix {
    pub fn new(
        deletion_rate: f64,
        substitution_rate: f64,
        insertion_rate: f64,
        seed: u64,
    ) -> Self {
        Self {
            deletion_rate,
            substitution_rate,
            insertion_rate,
            seed,
        }
    }

    pub fn set_rates(
        &mut self,
        del: Option<f64>,
        sub: Option<f64>,
        ins: Option<f64>,
    ) {
        if let Some(d) = del {
            if (0.0..=1.0).contains(&d) {
                self.deletion_rate = d;
            }
        }
        if let Some(s) = sub {
            if (0.0..=1.0).contains(&s) {
                self.substitution_rate = s;
            }
        }
        if let Some(i) = ins {
            if (0.0..=1.0).contains(&i) {
                self.insertion_rate = i;
            }
        }
    }

    /// Randomly destroy droplets (simulate oligo loss)
    pub fn mutate_droplets(
        &self,
        droplets: &[Droplet],
        loss_rate: f64,
    ) -> (Vec<Droplet>, ChaosStats) {
        let mut rng = StdRng::seed_from_u64(self.seed);
        let surviving: Vec<Droplet> = droplets
            .iter()
            .filter(|_| rng.gen::<f64>() >= loss_rate)
            .cloned()
            .collect();

        let stats = ChaosStats {
            total_droplets: droplets.len(),
            lost_droplets: droplets.len() - surviving.len(),
            surviving_droplets: surviving.len(),
        };

        (surviving, stats)
    }

    /// Apply random mutations to a DNA sequence
    pub fn mutate_sequence(&self, seq: &str) -> (String, MutationSummary) {
        let mut rng = StdRng::seed_from_u64(self.seed.wrapping_add(1));
        let bases = ['A', 'C', 'G', 'T'];
        let chars: Vec<char> = seq.chars().collect();

        let mut result = String::with_capacity(seq.len());
        let mut subs = 0usize;
        let mut dels = 0usize;
        let mut ins = 0usize;

        for &c in &chars {
            // Deletion
            if rng.gen::<f64>() < self.deletion_rate {
                dels += 1;
                continue;
            }
            // Substitution: replace with a DIFFERENT base
            if rng.gen::<f64>() < self.substitution_rate {
                let other_bases: Vec<char> = bases.iter().filter(|&&b| b != c).copied().collect();
                let new_base = other_bases[rng.gen_range(0..other_bases.len())];
                result.push(new_base);
                subs += 1;
            } else {
                result.push(c);
            }
            // Insertion
            if rng.gen::<f64>() < self.insertion_rate {
                result.push(bases[rng.gen_range(0..4)]);
                ins += 1;
            }
        }

        let summary = MutationSummary {
            total_mutations: subs + dels + ins,
            substitutions: subs,
            deletions: dels,
            insertions: ins,
        };

        (result, summary)
    }

    pub fn get_mutation_summary_empty() -> MutationSummary {
        MutationSummary {
            total_mutations: 0,
            substitutions: 0,
            deletions: 0,
            insertions: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_droplet_loss() {
        let chaos = ChaosMatrix::new(0.15, 0.05, 0.02, 42);
        let droplets: Vec<Droplet> = (0..100)
            .map(|i| Droplet {
                id: i,
                seed: i as u64,
                degree: 1,
                block_indices: vec![i],
                data: vec![0u8; 64],
            })
            .collect();
        let (surviving, stats) = chaos.mutate_droplets(&droplets, 0.30);
        assert!(stats.lost_droplets > 0);
        assert!(surviving.len() + stats.lost_droplets == 100);
    }

    #[test]
    fn test_sequence_mutation() {
        let chaos = ChaosMatrix::new(0.05, 0.10, 0.02, 42);
        let seq = "ACGTACGTACGTACGTACGT";
        let (mutated, summary) = chaos.mutate_sequence(seq);
        assert!(summary.total_mutations > 0);
        assert_ne!(mutated, seq);
    }
}
