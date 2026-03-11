//! Commercial Cost Estimation Engine
//!
//! Models real-world DNA synthesis and sequencing costs based on
//! current vendor pricing (Twist Bioscience, IDT, GenScript, etc.)
//! Provides cost breakdown, storage density analysis, and TCO comparisons.

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct CostEstimate {
    // Synthesis costs
    pub synthesis_cost_usd: f64,
    pub cost_per_oligo: f64,
    pub cost_per_mb_stored: f64,
    pub num_oligos: usize,
    pub total_bases_synthesized: usize,

    // Sequencing costs
    pub sequencing_cost_usd: f64,
    pub sequencing_method: String,

    // Storage metrics
    pub data_size_bytes: usize,
    pub physical_density_bits_per_nt: f64,
    pub effective_density_bits_per_nt: f64,
    pub total_dna_weight_grams: f64,
    pub volume_microliters: f64,

    // Total cost
    pub total_cost_usd: f64,
    pub cost_breakdown: Vec<CostItem>,

    // Comparison
    pub cloud_storage_10yr_usd: f64,
    pub tape_storage_10yr_usd: f64,
    pub dna_storage_10yr_usd: f64,
    pub dna_retention_years: usize,

    // Vendor recommendations
    pub recommended_vendor: String,
    pub vendor_options: Vec<VendorQuote>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CostItem {
    pub category: String,
    pub item: String,
    pub quantity: f64,
    pub unit_cost: f64,
    pub total_cost: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct VendorQuote {
    pub vendor: String,
    pub oligo_cost: f64,
    pub max_oligo_length: usize,
    pub turnaround_days: usize,
    pub bulk_discount_threshold: usize,
    pub estimated_total: f64,
    pub notes: String,
}

/// Vendor pricing models (2025-2026 approximate)
struct VendorPricing {
    name: &'static str,
    cost_per_oligo_standard: f64,
    cost_per_oligo_bulk: f64,   // 10K+ oligos
    bulk_threshold: usize,
    max_length: usize,
    turnaround_days: usize,
    sequencing_per_gb: f64,
}

const VENDORS: &[VendorPricing] = &[
    VendorPricing {
        name: "Twist Bioscience",
        cost_per_oligo_standard: 0.09,
        cost_per_oligo_bulk: 0.07,
        bulk_threshold: 10_000,
        max_length: 350,
        turnaround_days: 14,
        sequencing_per_gb: 15.0,
    },
    VendorPricing {
        name: "IDT (Integrated DNA Technologies)",
        cost_per_oligo_standard: 0.12,
        cost_per_oligo_bulk: 0.08,
        bulk_threshold: 5_000,
        max_length: 200,
        turnaround_days: 7,
        sequencing_per_gb: 20.0,
    },
    VendorPricing {
        name: "GenScript",
        cost_per_oligo_standard: 0.10,
        cost_per_oligo_bulk: 0.065,
        bulk_threshold: 20_000,
        max_length: 300,
        turnaround_days: 21,
        sequencing_per_gb: 18.0,
    },
    VendorPricing {
        name: "Eurofins Genomics",
        cost_per_oligo_standard: 0.11,
        cost_per_oligo_bulk: 0.075,
        bulk_threshold: 10_000,
        max_length: 250,
        turnaround_days: 10,
        sequencing_per_gb: 22.0,
    },
];

pub struct CostEstimator;

impl CostEstimator {
    pub fn estimate(
        data_size_bytes: usize,
        num_oligos: usize,
        total_bases: usize,
        oligo_length: usize,
        redundancy_ratio: f64,
    ) -> CostEstimate {
        // Physical storage density
        let raw_bits = data_size_bytes * 8;
        let physical_density = if total_bases > 0 {
            raw_bits as f64 / total_bases as f64
        } else {
            0.0
        };

        // Effective density (accounting for redundancy + overhead)
        // Effective density = physical_density / redundancy_ratio
        // With 2-bit encoding, physical max is 2.0 bits/nt.
        // With 2.5x redundancy: effective = 2.0 / 2.5 = 0.8 bits/nt
        // This accounts for the fact that redundancy spreads data across more bases.
        let effective_density = if redundancy_ratio > 0.0 && total_bases > 0 {
            physical_density / redundancy_ratio
        } else {
            0.0
        };

        // DNA weight estimate: avg MW per nucleotide = 330 Da
        // At typical synthesis scale: ~1 pmol (1e-12 mol) per oligo
        // Weight per oligo = oligo_length * 330 Da * 1e-12 mol * (1 g / 6.022e23 Da·mol⁻¹)
        // Total weight = num_oligos * weight_per_oligo
        let moles_per_oligo = 1e-12_f64; // 1 pmol
        let mw_per_oligo = oligo_length as f64 * 330.0; // Daltons
        let weight_grams = num_oligos as f64 * mw_per_oligo * moles_per_oligo;

        // Volume: typical concentration 100 μM in ~10 μL per oligo pool
        let volume_ul = (num_oligos as f64 / 1000.0).max(10.0).min(1000.0);

        // Calculate vendor quotes
        let vendor_options: Vec<VendorQuote> = VENDORS.iter().map(|v| {
            let cost_per = if num_oligos >= v.bulk_threshold {
                v.cost_per_oligo_bulk
            } else {
                v.cost_per_oligo_standard
            };
            let synthesis_total = num_oligos as f64 * cost_per;
            let seq_cost = (data_size_bytes as f64 / 1e9) * v.sequencing_per_gb;
            let total = synthesis_total + seq_cost;

            VendorQuote {
                vendor: v.name.to_string(),
                oligo_cost: cost_per,
                max_oligo_length: v.max_length,
                turnaround_days: v.turnaround_days,
                bulk_discount_threshold: v.bulk_threshold,
                estimated_total: (total * 100.0).round() / 100.0,
                notes: if oligo_length > v.max_length {
                    format!("⚠ Oligos exceed max length ({} > {}bp)", oligo_length, v.max_length)
                } else {
                    "✓ Compatible".to_string()
                },
            }
        }).collect();

        // Best vendor
        let best_vendor = vendor_options.iter()
            .filter(|v| !v.notes.starts_with('⚠'))
            .min_by(|a, b| a.estimated_total.partial_cmp(&b.estimated_total).unwrap())
            .cloned()
            .unwrap_or_else(|| vendor_options[0].clone());

        let synthesis_cost = num_oligos as f64 * best_vendor.oligo_cost;
        let best_vendor_pricing = VENDORS
            .iter()
            .find(|v| v.name == best_vendor.vendor)
            .unwrap_or(&VENDORS[0]);
        let sequencing_cost =
            (data_size_bytes as f64 / 1e9) * best_vendor_pricing.sequencing_per_gb;

        // Cost breakdown
        let breakdown = vec![
            CostItem {
                category: "Synthesis".into(),
                item: "DNA oligo synthesis".into(),
                quantity: num_oligos as f64,
                unit_cost: best_vendor.oligo_cost,
                total_cost: num_oligos as f64 * best_vendor.oligo_cost,
            },
            CostItem {
                category: "Synthesis".into(),
                item: "Quality control (per pool)".into(),
                quantity: 1.0,
                unit_cost: 50.0,
                total_cost: 50.0,
            },
            CostItem {
                category: "Sequencing".into(),
                item: "Illumina sequencing for readback".into(),
                quantity: 1.0,
                unit_cost: sequencing_cost,
                total_cost: sequencing_cost,
            },
            CostItem {
                category: "Storage".into(),
                item: "Lyophilization & cold storage".into(),
                quantity: 1.0,
                unit_cost: 25.0,
                total_cost: 25.0,
            },
        ];

        let total = breakdown.iter().map(|c| c.total_cost).sum::<f64>();
        let cost_per_mb = if data_size_bytes > 0 {
            total / (data_size_bytes as f64 / 1_048_576.0)
        } else {
            0.0
        };

        // 10-year TCO comparisons
        let data_gb = data_size_bytes as f64 / 1e9;
        let cloud_10yr = data_gb.max(0.001) * 0.023 * 12.0 * 10.0; // AWS S3 ~$0.023/GB/month
        let tape_10yr = data_gb.max(0.001) * 0.004 * 12.0 * 10.0;  // Tape ~$0.004/GB/month
        let dna_10yr = total; // One-time synthesis, no ongoing cost

        CostEstimate {
            synthesis_cost_usd: (synthesis_cost * 100.0).round() / 100.0,
            cost_per_oligo: best_vendor.oligo_cost,
            cost_per_mb_stored: (cost_per_mb * 100.0).round() / 100.0,
            num_oligos,
            total_bases_synthesized: total_bases,
            sequencing_cost_usd: (sequencing_cost * 100.0).round() / 100.0,
            sequencing_method: "Illumina NGS".into(),
            data_size_bytes,
            physical_density_bits_per_nt: (physical_density * 1000.0).round() / 1000.0,
            effective_density_bits_per_nt: (effective_density * 1000.0).round() / 1000.0,
            total_dna_weight_grams: weight_grams,
            volume_microliters: volume_ul,
            total_cost_usd: (total * 100.0).round() / 100.0,
            cost_breakdown: breakdown,
            cloud_storage_10yr_usd: (cloud_10yr * 100.0).round() / 100.0,
            tape_storage_10yr_usd: (tape_10yr * 100.0).round() / 100.0,
            dna_storage_10yr_usd: (dna_10yr * 100.0).round() / 100.0,
            dna_retention_years: 1000,
            recommended_vendor: best_vendor.vendor.clone(),
            vendor_options,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cost_estimate() {
        let est = CostEstimator::estimate(1024, 100, 20000, 200, 2.5);
        assert!(est.total_cost_usd > 0.0);
        assert!(est.synthesis_cost_usd > 0.0);
        assert!(!est.recommended_vendor.is_empty());
        assert!(!est.vendor_options.is_empty());
    }

    #[test]
    fn test_density_calculation() {
        let est = CostEstimator::estimate(1000, 50, 8000, 200, 2.5);
        // 8000 bits / 8000 bases = 1.0 bits/nt raw density
        assert!(est.physical_density_bits_per_nt > 0.0);
        assert!(est.physical_density_bits_per_nt <= 2.0);
    }
}
