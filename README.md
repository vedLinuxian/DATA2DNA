<p align="center">
  <img src="https://img.shields.io/badge/Rust-Edition%202021-dea584?style=flat-square&logo=rust&logoColor=white" alt="Rust">
  <img src="https://img.shields.io/badge/Tests-183%20passing-22c55e?style=flat-square" alt="Tests">
  <img src="https://img.shields.io/badge/Pipeline-8%20Stage-7c3aed?style=flat-square" alt="Pipeline">
  <img src="https://img.shields.io/badge/License-AGPL--3.0%20%2F%20Commercial-3b82f6?style=flat-square" alt="License">
  <img src="https://img.shields.io/badge/LOC-12%2C500+-64748b?style=flat-square" alt="Lines of Code">
</p>

<h1 align="center">🧬 DATA2DNA</h1>
<h3 align="center">Encode Any Digital Data Into Synthetic DNA Oligonucleotides</h3>

<p align="center">
  <b>Triple-layer error correction</b> · <b>Multi-stage compression</b> · <b>30% loss recovery</b> · <b>Web UI + REST API</b>
</p>

<p align="center">
  <a href="#quick-start">Quick Start</a> ·
  <a href="#architecture">Architecture</a> ·
  <a href="#error-correction">Error Correction</a> ·
  <a href="#web-interface">Web Interface</a> ·
  <a href="#api-reference">API</a> ·
  <a href="#research">Research</a> ·
  <a href="#contributing">Contributing</a>
</p>

---

## What Is This?

DATA2DNA is a complete pipeline that encodes arbitrary digital data into synthetic DNA oligonucleotides and decodes it back. It's written in Rust, runs as a web server with a browser UI, and is designed for **text-based data** — CSV, JSON, SQL dumps, source code, scientific datasets, logs, config files.

DNA is interesting as a storage medium because:

| | Hard Drive | Magnetic Tape | DNA |
|---|---|---|---|
| **Density** | ~1 TB / 100g | 18 TB / cartridge | 215 PB / gram (theoretical) |
| **Durability** | 5–10 years | 15–30 years | 10,000+ years (demonstrated) |
| **Storage energy** | 6–8W continuous | Climate-controlled | Zero (ambient stable) |
| **Read technology** | Fixed (SATA/NVMe) | Fixed (LTO generation) | Sequencing (gets cheaper yearly) |

> **Honest caveat:** DNA synthesis is currently expensive (~$0.07–0.12/nt at scale) and slow. This is a research-grade tool, not a production storage replacement today. The economics improve roughly 10× per decade — similar to how sequencing costs dropped 1,000,000× since 2001.

---

## Quick Start

### Prerequisites

- [Rust](https://rustup.rs/) (stable toolchain, edition 2021)

### Build & Run

```bash
git clone https://github.com/vedLinuxian/DATA2DNA.git
cd DATA2DNA

# Quick start (builds + launches server)
chmod +x start.sh
./start.sh

# Or manually
cargo build --release
cargo run --release
```

Open **http://localhost:5000** in your browser.

### Run Tests

```bash
# Full suite (183 tests: 84 unit + 99 integration)
cargo test

# With output
cargo test -- --nocapture

# Specific module
cargo test reed_solomon
cargo test integration_tests
```

### Port Configuration

```bash
./start.sh                    # default port 5000
./start.sh --port 8080        # named flag
PORT=3000 ./start.sh          # environment variable
```

---

## Architecture

### Encode Pipeline (8 Stages)

```
Data ──► HyperCompress ──► Interleaved RS ──► Fountain Codes ──► DNA Transcode
                                                                       │
         FASTA ◄── Cost Estimate ◄── Constraint Screen ◄── Oligo Build ◄┘
```

| # | Stage | Module | What It Does |
|---|-------|--------|---|
| 1 | **Compress** | `hypercompress.rs` | Multi-stage: BWT+MTF+ZRLE → BPE → parallel ZSTD-22 / Brotli-11 trials (best wins) |
| 2 | **Interleaved RS** | `interleaved_rs.rs` | Spread RS(255,223) symbols across oligos — 1 lost oligo = 1 error per RS block |
| 3 | **Fountain Codes** | `fountain.rs` | Hybrid systematic + LT codes, Robust Soliton distribution (c=0.025, δ=0.001) |
| 4 | **Transcode** | `transcoder.rs` | Binary → DNA (2-bit encoding: 00=A, 01=C, 10=G, 11=T) with rotation cipher |
| 5 | **Oligo Builder** | `oligo_builder.rs` | Primers (20bp each) + index (16bp) + payload + CRC-32 (16bp) |
| 6 | **Constraints** | `dna_constraints.rs` | GC 40–60%, homopolymer ≤3bp, restriction enzyme screening (20 enzymes) |
| 7 | **FASTA** | `fasta.rs` | Output with embedded metadata for standalone decode |
| 8 | **Cost** | `cost_estimator.rs` | Synthesis pricing at Twist / IDT / GenScript rates |

### Decode Pipeline

```
FASTA → Oligo Disassemble → Fountain Decode (peeling + Gaussian) → RS Error Correction → Decompress → Original Data
```

---

## Error Correction

This is the core of what makes DNA storage actually work. Three independent layers protect your data:

### Layer 1: Per-Oligo CRC-32
Detects if an individual oligo has been corrupted during synthesis or sequencing.

### Layer 2: Reed-Solomon RS(255,223) in GF(2⁸)
- **16 errors corrected** per codeword (standard error-only decoding)
- **32 erasures corrected** when loss positions are known (erasure-only mode)
- **Combined error + erasure decoding**: 2×errors + erasures ≤ 32
  - Example: 10 lost oligos + 11 base substitutions = 32 → recoverable ✓
- Full algebraic decoder: Berlekamp-Massey → Chien Search → Forney Algorithm
- Erasure decoding via Forney syndromes (removes erasure effects, then standard BM)

### Layer 3: Fountain/LT Codes
- Robust Soliton distribution with parameters from the DNA Fountain paper
- Hybrid: systematic degree-1 droplets first (baseline), then XOR droplets
- Peeling decoder with Gaussian elimination fallback
- Default 2.0× redundancy → survives 30% oligo loss with 40% safety margin

### Redundancy Math

```
surviving_data = redundancy × (1 - loss_rate)

30% loss, 2.0× redundancy:  2.0 × 0.7 = 1.40  →  40% margin  ✓
40% loss, 2.5× redundancy:  2.5 × 0.6 = 1.50  →  50% margin  ✓
30% loss, 1.5× redundancy:  1.5 × 0.7 = 1.05  →  5% margin   ⚠ (risky)
```

---

## Web Interface

The server provides a full browser UI at `http://localhost:5000` with:

- **Encode tab** — Drag-and-drop file upload or paste text, configurable redundancy, real-time SSE progress
- **FASTA Decode tab** — Upload a `.fasta` file to recover original data
- **Chaos tab** — Simulate DNA degradation (oligo loss, substitutions, insertions) with preset profiles (Illumina, Nanopore, PacBio, 1000-year aging, catastrophic)
- **Pipeline Decode tab** — Decode after chaos simulation, verify bit-perfect recovery
- **Benchmark tab** — Run benchmarks across data types, view compression ratios, oligo counts, timing

There's also a dedicated benchmark app at `/benchmark` for custom parameter tuning.

---

## API Reference

All endpoints accept JSON or multipart form data. SSE provides real-time progress.

| Endpoint | Method | Description |
|---|---|---|
| `/api/encode` | POST | Encode data to DNA oligos |
| `/api/decode-fasta` | POST | Decode from FASTA content |
| `/api/chaos` | POST | Apply chaos/degradation simulation |
| `/api/decode` | POST | Decode after chaos |
| `/api/benchmark` | POST | Run standard benchmark suite |
| `/api/benchmark_custom` | POST | Run benchmark with custom parameters |
| `/api/events/{id}` | GET | SSE progress stream |
| `/api/config` | GET | Current pipeline configuration |
| `/api/download_fasta` | GET | Download encoded FASTA |
| `/api/download_original` | GET | Download original file |
| `/api/download_recovered` | GET | Download recovered file |
| `/api/health` | GET | Health check |

---

## Technical Parameters

| Parameter | Value | Notes |
|---|---|---|
| Oligo length | 300 bp | Twist Bioscience / IDT standard synthesis length |
| Overhead per oligo | 72 bp | 20bp forward primer + 16bp index + 16bp CRC + 20bp reverse primer |
| Useful payload | 228 bp (76%) | 300 − 72 |
| RS code | RS(255,223) | 32 parity symbols, GF(2⁸) with polynomial 0x11D |
| Fountain params | c=0.025, δ=0.001 | Robust Soliton (DNA Fountain paper values) |
| Default redundancy | 2.0× | Configurable 1.0–5.0× |
| Block size | 64 bytes | RS alignment |
| Compression | ZSTD-22 / Brotli-11 | Parallel trials, smaller output wins |
| Physical density | 0.76 bits/nt | 2.0 raw × 76% efficiency ÷ 2.0× redundancy |

> **On density claims:** Our physical encoding density is 0.76 bits/nt. When compression is applied to text data (CSV, JSON, SQL), the *effective* throughput can reach 6–12 original bits per nucleotide — but that's a compression metric, not a physical encoding property. We keep these numbers separate because conflating them would be misleading.

---

## Module Map

| Module | Lines | Purpose |
|---|---|---|
| `hypercompress.rs` | 2,501 | Multi-stage compression: BWT+MTF+ZRLE, BPE, ZSTD/Brotli, parallel Rayon trials |
| `main.rs` | 1,909 | Actix-web 4 server: REST API, SSE progress, multipart upload, CORS |
| `pipeline.rs` | 1,332 | Pipeline orchestrator, Shannon entropy estimator, data classifier, adaptive redundancy |
| `reed_solomon.rs` | 847 | GF(2⁸) RS: error-only, erasure-only, and combined E&E via Forney syndromes |
| `compressor.rs` | 663 | Legacy single-algorithm compressor (backwards compatibility) |
| `dna_constraints.rs` | 586 | Biological screening: GC, homopolymer, restriction enzymes, melting temperature |
| `fountain.rs` | 583 | Hybrid systematic/LT codes, Robust Soliton, Rayon-parallel XOR droplets |
| `oligo_builder.rs` | 574 | Structured oligo construction: primers, index, payload, CRC-32, quality scoring |
| `interleaved_rs.rs` | 393 | Cross-oligo RS protection with variable depth (4/16/64) |
| `fasta.rs` | 328 | FASTA I/O with metadata embedding for standalone decode |
| `cost_estimator.rs` | 295 | Synthesis cost modeling for Twist, IDT, GenScript |
| `transcoder.rs` | 283 | 2-bit DNA encoding with rotation cipher for GC balance |
| `chaos.rs` | 185 | DNA degradation simulation (loss, substitution, insertion) |
| `consensus.rs` | 99 | Consensus decoder (fountain wrapper) |
| `lib.rs` | 26 | Public API re-exports |
| **Total** | **10,604** | Plus 1,901 lines of integration tests |

---

## Performance Optimizations

These are implemented, not hypothetical:

- **SIMD-friendly XOR** — Fountain droplet XOR processes 8 bytes at a time via u64 pointer casts
- **Parallel fountain encoding** — Rayon work-stealing for XOR droplet generation (batch >64)
- **MTF stack array** — Move-to-Front uses `[u8; 256]` on stack instead of `Vec` heap allocation
- **Byte-level DNA ops** — Homopolymer check, GC analysis, quality scoring work on `&[u8]` directly
- **Compile-time Tm tables** — Nearest-neighbor melting temperature uses `const` 4×4 arrays (no HashMap)
- **RS pre-allocation** — `encode_buffer()` computes exact output size with `Vec::with_capacity`
- **Shannon entropy pre-analysis** — Classifies data before compression to guide strategy selection

---

## Data Intelligence

The pipeline includes entropy analysis that runs before compression:

- **`estimate_entropy(data)`** — First-order Shannon entropy in bits/byte (0.0–8.0)
- **`classify_data(data)`** — Categorizes as: `highly_repetitive`, `structured_text`, `natural_text`, `code_or_markup`, `pre_compressed`, `high_entropy_binary`, `mixed_text`, `binary_data`
- **`calculate_adaptive_redundancy()`** — Computes minimum safe redundancy from Shannon capacity + fountain overhead + RS correction budget + safety margin

These are exposed in the encode output and inform compression strategy.

---

## Research & Motivation

### The Cold Data Problem

60–80% of enterprise data is "cold" — stored for compliance or disaster recovery, accessed less than once per year. It still sits on powered, cooled storage infrastructure. DNA eliminates the ongoing energy cost entirely.

### Academic Foundation

This project builds on peer-reviewed research:

1. **Erlich & Zielinski** (2017). "DNA Fountain enables a robust and efficient storage architecture." *Science*, 355(6328), 950–954. [DOI: 10.1126/science.aaj2038](https://doi.org/10.1126/science.aaj2038)
2. **Church, Gao & Kosuri** (2012). "Next-Generation Digital Information Storage in DNA." *Science*, 337(6102), 1628. [DOI: 10.1126/science.1226355](https://doi.org/10.1126/science.1226355)
3. **Goldman et al.** (2013). "Towards practical, high-capacity, low-maintenance information storage in synthesized DNA." *Nature*, 494(7435), 77–80. [DOI: 10.1038/nature11875](https://doi.org/10.1038/nature11875)
4. **Organick et al.** (2018). "Random access in large-scale DNA data storage." *Nature Biotechnology*, 36(3), 242–248. [DOI: 10.1038/nbt.4079](https://doi.org/10.1038/nbt.4079)
5. **Grass et al.** (2015). "Robust chemical preservation of digital information on DNA in silica with error-correcting codes." *Angewandte Chemie*, 54(8), 2552–2555. [DOI: 10.1002/anie.201411378](https://doi.org/10.1002/anie.201411378)
6. **Ceze, Nivala & Strauss** (2019). "Molecular digital data storage using DNA." *Nature Reviews Genetics*, 20(8), 456–466. [DOI: 10.1038/s41576-019-0125-3](https://doi.org/10.1038/s41576-019-0125-3)
7. **Luby** (2002). "LT codes." *Proc. 43rd IEEE FOCS*, 271–280.

### Where We Stand

| System | Year | Bits/nt | Error Correction | Open Source |
|---|---|---|---|---|
| Church et al. | 2012 | ~0.83 | Repetition | Paper only |
| Goldman et al. | 2013 | ~0.33 | Fourfold redundancy | Paper only |
| DNA Fountain | 2017 | 1.57 | RS + Fountain | Paper only |
| Microsoft/UW | 2018 | ~1.10 | RS + Repetition | No |
| **DATA2DNA** | **2025** | **0.76** | **RS + IRS + Fountain + E&E** | **Yes (AGPL)** |

Our physical density (0.76 bits/nt) is lower than DNA Fountain's 1.57 — we trade density for a much deeper error correction stack (three layers + combined error-and-erasure decoding). DNA Fountain uses RS + Fountain only with minimal overhead structure. Our oligos carry 72bp of overhead (primers, index, CRC) which reduces raw payload efficiency but enables practical features like random access indexing and per-oligo integrity checking.

---

## Project Structure

```
DATA2DNA/
├── src/
│   ├── main.rs               # Actix-web 4 HTTP server (SSE, multipart, CORS)
│   ├── lib.rs                 # Public API re-exports
│   ├── pipeline.rs            # Pipeline orchestrator + entropy analysis
│   ├── hypercompress.rs       # Multi-stage compression engine
│   ├── reed_solomon.rs        # RS(255,223): errors, erasures, combined E&E
│   ├── interleaved_rs.rs      # Cross-oligo RS protection
│   ├── fountain.rs            # Hybrid systematic/LT Fountain codes
│   ├── transcoder.rs          # DNA ↔ binary transcoding
│   ├── oligo_builder.rs       # Structured oligo construction + quality scoring
│   ├── dna_constraints.rs     # Biological constraint screening + Tm tables
│   ├── fasta.rs               # FASTA I/O with metadata
│   ├── chaos.rs               # Degradation simulation
│   ├── cost_estimator.rs      # Synthesis cost modeling
│   ├── consensus.rs           # Consensus decoder
│   └── compressor.rs          # Legacy compressor
├── static/
│   ├── index.html             # Web interface
│   ├── benchmark.html         # Benchmark app
│   ├── script.js              # Frontend controller
│   └── style.css              # Dark biotech theme
├── tests/
│   └── integration_tests.rs   # 99 integration tests
├── docs/
│   ├── index.html             # Documentation site
│   └── GREEN_TECH_RESEARCH.md # Sustainability analysis
├── .github/
│   ├── agents/                # 7 specialized Copilot agents
│   └── copilot-instructions.md
├── Cargo.toml
├── start.sh                   # Quick-start launcher
├── run.sh                     # Port-configurable launcher
├── RESEARCH.md
├── CONTRIBUTING.md
├── CHANGELOG.md
├── LICENSE                    # AGPL-3.0
└── LICENSE-COMMERCIAL.md
```

---

## Supported Data Formats

DATA2DNA is optimized for **text-based data** where compression makes the biggest impact on oligo count and synthesis cost:

| Format | Expected Compression | Notes |
|---|---|---|
| CSV / TSV | 5–10× | Column repetition, numeric patterns |
| JSON / JSONL | 4–8× | Repeated keys, structural dedup |
| SQL dumps | 8–20× | Massive keyword repetition |
| Source code | 3–5× | Indentation, keywords |
| FASTA genomic | 3–8× | Repetitive sequences |
| Plain text / logs | 3–6× | Natural language redundancy |
| Config files | 4–8× | Key-value patterns |

**Not recommended:** Images, video, audio, pre-compressed archives. These are already near Shannon entropy limits and won't compress further — you'd pay for DNA synthesis with minimal density benefit.

---

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). We welcome contributions in:

- Error correction algorithms (RS variants, LDPC, polar codes)
- Compression optimizations for specific data formats
- DNA constraint improvements (new synthesis vendor specs)
- Random access mechanisms
- Cost model updates with current vendor pricing
- Test coverage expansion
- Web UI improvements

---

## License

Dual licensed:

- **Open source**: [AGPL-3.0](LICENSE) — free for open-source and research use
- **Commercial**: [LICENSE-COMMERCIAL.md](LICENSE-COMMERCIAL.md) — for proprietary/closed-source use

Contact vedcimit@gmail.com for commercial licensing.

---

## Citation

```bibtex
@software{data2dna2025,
  title   = {DATA2DNA: Encoding Digital Data Into Synthetic DNA},
  author  = {Ved},
  year    = {2025},
  url     = {https://github.com/vedLinuxian/DATA2DNA},
  note    = {Open-source DNA data storage with triple-layer error correction (RS + Interleaved RS + Fountain)}
}
```

---

<p align="center">
  <sub>Built with Rust 🦀 — 10,604 lines of systems code, 183 tests, zero <code>.unwrap()</code> on user data paths.</sub>
</p>
