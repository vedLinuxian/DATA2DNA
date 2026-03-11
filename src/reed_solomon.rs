// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2025 Ved
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as published
// by the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// For commercial licensing, contact: vedcimit@gmail.com

//! Reed-Solomon Error Correction for DNA Oligos
//!
//! Implements RS(255,223) — 32 parity symbols per 223 data symbols.
//! Industry standard for per-oligo error correction in DNA data storage.
//!
//! GF(2^8) arithmetic: primitive polynomial x^8 + x^4 + x^3 + x^2 + 1 (0x11D).
//! Generator roots: α^0, α^1, ..., α^(2t-1) where α=2 is the primitive element.
//!
//! Codeword layout: [data_0 .. data_{k-1} | parity_0 .. parity_{2t-1}]
//! Polynomial convention: c(x) = c[0]*x^(n-1) + c[1]*x^(n-2) + ... + c[n-1]
//! (descending power order = natural storage order)

use serde::Serialize;

const GF_POLY: u32 = 0x11D;

#[derive(Debug, Clone)]
pub struct ReedSolomonCodec {
    pub data_symbols: usize,
    pub parity_symbols: usize,
    pub total_symbols: usize,
    exp_table: [u8; 512],
    log_table: [u8; 256],
    generator: Vec<u8>, // descending order: gen[0] = x^nsym coeff = 1
}

#[derive(Debug, Clone, Serialize)]
pub struct RSStats {
    pub data_symbols: usize,
    pub parity_symbols: usize,
    pub total_symbols: usize,
    pub max_correctable_errors: usize,
    pub overhead_percent: f64,
    pub blocks_encoded: usize,
    pub blocks_corrected: usize,
    pub total_errors_corrected: usize,
}

// ═══════════════════ GF(2^8) ═══════════════════

fn build_gf_tables() -> ([u8; 512], [u8; 256]) {
    let mut exp = [0u8; 512];
    let mut log = [0u8; 256];
    let mut x: u32 = 1;
    for i in 0..255u32 {
        exp[i as usize] = x as u8;
        log[x as usize] = i as u8;
        x <<= 1;
        if x & 0x100 != 0 { x ^= GF_POLY; }
    }
    for i in 255..512 { exp[i] = exp[i - 255]; }
    (exp, log)
}

/// Build generator in descending order: g(x) = Π(x + α^i) for i=0..nsym-1
/// gen[0] = 1 (coeff of x^nsym), gen[nsym] = constant term
fn build_generator_desc(exp: &[u8; 512], log: &[u8; 256], nsym: usize) -> Vec<u8> {
    let mut g = vec![1u8]; // Start with "1" (x^0 in descending = just constant 1)
    for i in 0..nsym {
        let alpha_i = exp[i];
        // Multiply g(x) by (x + α^i)
        // In descending: if g has coeffs [g0, g1, ..., gm],
        // result has coeffs [g0, g1^(α^i·g0), g2^(α^i·g1), ..., α^i·gm]
        let mut new_g = vec![0u8; g.len() + 1];
        new_g[0] = g[0]; // leading coeff stays
        for j in 1..g.len() {
            new_g[j] = g[j] ^ gf_mul_raw(exp, log, alpha_i, g[j - 1]);
        }
        new_g[g.len()] = gf_mul_raw(exp, log, alpha_i, g[g.len() - 1]);
        g = new_g;
    }
    g
}

#[inline]
fn gf_mul_raw(exp: &[u8; 512], log: &[u8; 256], a: u8, b: u8) -> u8 {
    if a == 0 || b == 0 { return 0; }
    exp[(log[a as usize] as usize + log[b as usize] as usize) % 255]
}

// ═══════════════════ Codec ═══════════════════

impl ReedSolomonCodec {
    pub fn new(data_symbols: usize, parity_symbols: usize) -> Self {
        assert!(data_symbols + parity_symbols <= 255);
        assert!(parity_symbols >= 2 && parity_symbols % 2 == 0);
        let (exp, log) = build_gf_tables();
        let gen = build_generator_desc(&exp, &log, parity_symbols);
        Self {
            data_symbols,
            parity_symbols,
            total_symbols: data_symbols + parity_symbols,
            exp_table: exp,
            log_table: log,
            generator: gen,
        }
    }

    pub fn default_commercial() -> Self { Self::new(223, 32) }
    pub fn lightweight() -> Self { Self::new(239, 16) }

    // ——— GF helpers ———

    #[inline] fn mul(&self, a: u8, b: u8) -> u8 {
        if a == 0 || b == 0 { return 0; }
        self.exp_table[(self.log_table[a as usize] as usize + self.log_table[b as usize] as usize) % 255]
    }
    #[inline] fn inv(&self, a: u8) -> u8 {
        assert_ne!(a, 0);
        self.exp_table[255 - self.log_table[a as usize] as usize]
    }

    // ——— Systematic Encoding ———

    /// Encode: codeword[0..k] = data, codeword[k..n] = parity
    /// c(x) = m(x)*x^nsym + (m(x)*x^nsym mod g(x))
    pub fn encode(&self, data: &[u8]) -> Vec<u8> {
        let k = self.data_symbols;
        let nsym = self.parity_symbols;
        let n = self.total_symbols;

        let mut cw = vec![0u8; n];
        let dlen = data.len().min(k);
        cw[..dlen].copy_from_slice(&data[..dlen]);

        // Compute parity = data * x^nsym mod g(x) using synthetic division
        // generator is in descending order, gen[0] = 1
        let mut feedback;
        for i in 0..k {
            feedback = cw[i] ^ cw[k]; // cw[k] is first parity position
            if feedback != 0 {
                // gen[0] is 1, skip it; gen[1..nsym] are middle terms; gen[nsym] is constant
                for j in 1..nsym {
                    cw[k + j - 1] = cw[k + j] ^ self.mul(feedback, self.generator[j]);
                }
                cw[k + nsym - 1] = self.mul(feedback, self.generator[nsym]);
            } else {
                // Shift parity register left
                for j in 0..nsym - 1 {
                    cw[k + j] = cw[k + j + 1];
                }
                cw[k + nsym - 1] = 0;
            }
        }
        cw
    }

    // ——— Syndromes ———

    /// S_i = c(α^i) for i = 0..2t-1
    /// With descending convention: c(x) = c[0]*x^(n-1) + ... + c[n-1]
    fn syndromes(&self, cw: &[u8]) -> Vec<u8> {
        let n = self.total_symbols;
        let nsym = self.parity_symbols;
        let mut s = vec![0u8; nsym];
        for i in 0..nsym {
            let alpha_i = self.exp_table[i];
            let mut val = 0u8;
            for j in 0..n {
                val = self.mul(val, alpha_i) ^ cw[j];
            }
            s[i] = val;
        }
        s
    }

    // ——— Berlekamp–Massey ———

    fn berlekamp_massey(&self, s: &[u8]) -> Option<Vec<u8>> {
        let nsym = s.len();
        // σ(x) ascending: σ[0]=1, σ[j] = coeff of x^j
        // Uses the standard textbook formulation with shift counter m
        let mut c = vec![1u8];         // C(x) = current error locator
        let mut b_poly = vec![1u8];    // B(x) = previous
        let mut l = 0usize;
        let mut m = 1usize;           // shift amount
        let mut b_val = 1u8;          // previous discrepancy

        for n in 0..nsym {
            // Discrepancy δ = S_n + Σ(C_i * S_{n-i})
            let mut delta = s[n];
            for j in 1..=l.min(c.len() - 1) {
                delta ^= self.mul(c[j], s[n - j]);
            }

            if delta == 0 {
                m += 1;
            } else if 2 * l <= n {
                // Update needed, and L increases
                let t = c.clone();
                let factor = self.mul(delta, self.inv(b_val));
                // C(x) = C(x) - factor * x^m * B(x)
                let needed = b_poly.len() + m;
                if c.len() < needed { c.resize(needed, 0); }
                for (i, &bi) in b_poly.iter().enumerate() {
                    c[i + m] ^= self.mul(factor, bi);
                }
                l = n + 1 - l;
                b_poly = t;
                b_val = delta;
                m = 1;
            } else {
                // Update C but don't change B
                let factor = self.mul(delta, self.inv(b_val));
                let needed = b_poly.len() + m;
                if c.len() < needed { c.resize(needed, 0); }
                for (i, &bi) in b_poly.iter().enumerate() {
                    c[i + m] ^= self.mul(factor, bi);
                }
                m += 1;
            }
        }

        // Trim trailing zeros
        while c.len() > 1 && c.last() == Some(&0) { c.pop(); }

        let t = self.parity_symbols / 2;
        if c.len() - 1 > t { return None; }
        Some(c)
    }

    // ——— Chien Search ———
    // Error at codeword position j means error in coefficient of x^(n-1-j).
    // Error value X_j = α^(n-1-j). Root of σ gives X_j^(-1) = α^(-(n-1-j)) = α^(j-(n-1)).
    // So we test σ(α^(j-n+1)) for j=0..n-1 and look for zeros.

    fn chien_search(&self, sigma: &[u8]) -> Option<Vec<usize>> {
        let n = self.total_symbols;
        let ne = sigma.len() - 1;
        let mut positions = Vec::with_capacity(ne);

        for j in 0..n {
            // X_j = α^(n-1-j), so X_j^(-1) = α^(j-n+1) = α^((j+256-n) mod 255)
            let power = (256 + j - n) % 255;
            let x_inv = self.exp_table[power];
            // Evaluate σ(x_inv) using ascending polynomial
            let mut val = 0u8;
            let mut x_pow = 1u8;
            for &c in sigma {
                val ^= self.mul(c, x_pow);
                x_pow = self.mul(x_pow, x_inv);
            }
            if val == 0 {
                positions.push(j);
            }
        }

        if positions.len() == ne { Some(positions) } else { None }
    }

    // ——— Forney Algorithm ———

    fn forney(&self, s: &[u8], sigma: &[u8], positions: &[usize]) -> Option<Vec<u8>> {
        let nsym = self.parity_symbols;
        let n = self.total_symbols;

        // Syndrome polynomial S(x) = S_0 + S_1*x + ... + S_{2t-1}*x^{2t-1} (ascending)
        // Error evaluator Ω(x) = S(x)*σ(x) mod x^{2t}
        let mut omega = vec![0u8; nsym];
        for i in 0..nsym {
            for j in 0..sigma.len().min(i + 1) {
                omega[i] ^= self.mul(sigma[j], s[i - j]);
            }
        }

        // Formal derivative σ'(x) in GF(2): keep odd-index coefficients shifted down
        let mut sigma_d = vec![0u8; sigma.len().saturating_sub(1).max(1)];
        for i in (1..sigma.len()).step_by(2) {
            sigma_d[i - 1] = sigma[i];
        }

        let mut magnitudes = Vec::with_capacity(positions.len());
        for &j in positions {
            // X_j = α^(n-1-j)
            let x_j = self.exp_table[(n - 1 - j) % 255];
            let x_j_inv = self.inv(x_j);

            // Evaluate Ω(X_j^{-1})
            let mut omega_val = 0u8;
            let mut xp = 1u8;
            for &c in &omega {
                omega_val ^= self.mul(c, xp);
                xp = self.mul(xp, x_j_inv);
            }

            // Evaluate σ'(X_j^{-1})
            let mut sd_val = 0u8;
            xp = 1u8;
            for &c in &sigma_d {
                sd_val ^= self.mul(c, xp);
                xp = self.mul(xp, x_j_inv);
            }

            if sd_val == 0 { return None; }

            // e_j = X_j * Ω(X_j^{-1}) / σ'(X_j^{-1})
            magnitudes.push(self.mul(x_j, self.mul(omega_val, self.inv(sd_val))));
        }

        Some(magnitudes)
    }

    // ——— Public Decode ———

    pub fn decode(&self, received: &[u8]) -> Option<(Vec<u8>, usize)> {
        if received.len() != self.total_symbols { return None; }

        let s = self.syndromes(received);
        if s.iter().all(|&v| v == 0) {
            return Some((received[..self.data_symbols].to_vec(), 0));
        }

        let sigma = self.berlekamp_massey(&s)?;
        let ne = sigma.len() - 1;
        if ne == 0 || ne > self.parity_symbols / 2 { return None; }

        let positions = self.chien_search(&sigma)?;
        if positions.len() != ne { return None; }

        let mags = self.forney(&s, &sigma, &positions)?;

        let mut corrected = received.to_vec();
        for (i, &pos) in positions.iter().enumerate() {
            corrected[pos] ^= mags[i];
        }

        // Verify correction succeeded
        let sv = self.syndromes(&corrected);
        if sv.iter().any(|&v| v != 0) { return None; }

        Some((corrected[..self.data_symbols].to_vec(), ne))
    }

    // ——— Buffer API ———

    pub fn encode_buffer(&self, data: &[u8]) -> (Vec<u8>, RSStats) {
        let len_bytes = (data.len() as u64).to_le_bytes();
        let mut buf = len_bytes.to_vec();
        buf.extend_from_slice(data);
        let mut encoded = Vec::new();
        let mut blocks = 0usize;
        for chunk in buf.chunks(self.data_symbols) {
            encoded.extend_from_slice(&self.encode(chunk));
            blocks += 1;
        }
        (encoded, RSStats {
            data_symbols: self.data_symbols,
            parity_symbols: self.parity_symbols,
            total_symbols: self.total_symbols,
            max_correctable_errors: self.parity_symbols / 2,
            overhead_percent: (self.parity_symbols as f64 / self.data_symbols as f64 * 1000.0).round() / 10.0,
            blocks_encoded: blocks, blocks_corrected: 0, total_errors_corrected: 0,
        })
    }

    pub fn decode_buffer(&self, encoded: &[u8]) -> Option<(Vec<u8>, RSStats)> {
        if encoded.len() % self.total_symbols != 0 { return None; }
        let mut decoded = Vec::new();
        let mut total_errors = 0usize;
        let mut blocks_corrected = 0usize;
        let num_blocks = encoded.len() / self.total_symbols;
        for chunk in encoded.chunks(self.total_symbols) {
            let (data, ne) = self.decode(chunk)?;
            decoded.extend_from_slice(&data);
            if ne > 0 { blocks_corrected += 1; total_errors += ne; }
        }
        if decoded.len() < 8 { return None; }
        let orig_len = u64::from_le_bytes([
            decoded[0], decoded[1], decoded[2], decoded[3],
            decoded[4], decoded[5], decoded[6], decoded[7],
        ]) as usize;
        if orig_len + 8 > decoded.len() { return None; }
        Some((decoded[8..8 + orig_len].to_vec(), RSStats {
            data_symbols: self.data_symbols, parity_symbols: self.parity_symbols,
            total_symbols: self.total_symbols,
            max_correctable_errors: self.parity_symbols / 2,
            overhead_percent: (self.parity_symbols as f64 / self.data_symbols as f64 * 1000.0).round() / 10.0,
            blocks_encoded: num_blocks, blocks_corrected, total_errors_corrected: total_errors,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gf_arithmetic() {
        let rs = ReedSolomonCodec::new(10, 4);
        assert_eq!(rs.mul(0, 100), 0);
        assert_eq!(rs.mul(1, 100), 100);
        let a = 42u8;
        assert_eq!(rs.mul(a, rs.inv(a)), 1);
    }

    #[test]
    fn test_encode_decode_no_errors() {
        let rs = ReedSolomonCodec::new(223, 32);
        let data: Vec<u8> = (0..223).map(|i| (i % 256) as u8).collect();
        let cw = rs.encode(&data);
        assert_eq!(cw.len(), 255);
        let (dec, ne) = rs.decode(&cw).expect("should decode clean codeword");
        assert_eq!(ne, 0);
        assert_eq!(dec, data);
    }

    #[test]
    fn test_encode_decode_with_errors() {
        let rs = ReedSolomonCodec::new(223, 32);
        let data: Vec<u8> = (0..223).map(|i| (i * 7 % 256) as u8).collect();
        let mut cw = rs.encode(&data);
        // 10 errors (max correctable = 16)
        for &p in &[5, 20, 50, 100, 150, 200, 210, 220, 3, 18] {
            cw[p] ^= 0xFF;
        }
        let result = rs.decode(&cw);
        assert!(result.is_some(), "RS should correct 10 errors");
        let (dec, ne) = result.unwrap();
        assert_eq!(ne, 10);
        assert_eq!(dec, data);
    }

    #[test]
    fn test_buffer_roundtrip() {
        let rs = ReedSolomonCodec::new(223, 32);
        let data = b"Reed-Solomon error correction for DNA oligos!".to_vec();
        let (enc, _) = rs.encode_buffer(&data);
        let (dec, stats) = rs.decode_buffer(&enc).unwrap();
        assert_eq!(dec, data);
        assert_eq!(stats.total_errors_corrected, 0);
    }

    #[test]
    fn test_buffer_with_corruption() {
        let rs = ReedSolomonCodec::new(223, 32);
        let data: Vec<u8> = (0..500).map(|i| (i % 256) as u8).collect();
        let (mut enc, _) = rs.encode_buffer(&data);
        // Corrupt a few bytes per block (< 16)
        for i in (0..enc.len()).step_by(51) {
            enc[i] ^= 0xFF;
        }
        let result = rs.decode_buffer(&enc);
        assert!(result.is_some(), "Should recover from moderate corruption");
        let (dec, stats) = result.unwrap();
        assert_eq!(dec, data);
        assert!(stats.total_errors_corrected > 0);
    }
}
