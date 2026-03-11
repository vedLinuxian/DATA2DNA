<p align="center">
  <img src="https://img.shields.io/badge/Rust-2021-orange?logo=rust" alt="Rust">
  <img src="https://img.shields.io/badge/License-AGPL--3.0%20%2F%20Commercial-blue" alt="License">
  <img src="https://img.shields.io/badge/Tests-151%20passing-brightgreen" alt="Tests">
  <img src="https://img.shields.io/badge/Pipeline-8%20stages-purple" alt="Pipeline">
</p>

<h1 align="center">🧬 DATA2DNA</h1>
<h3 align="center">Encode Any Data Into Synthetic DNA — The Future of Archival Storage</h3>

<p align="center">
  <b>151 tests</b> · <b>8-stage pipeline</b> · <b>30% loss recovery</b> · <b>Zero energy storage</b> · <b>10,000+ year durability</b>
</p>

---

## Why DNA?

| Metric | Hard Drive | Tape (LTO-9) | **DNA** |
|--------|-----------|--------------|---------|
| **Density** | ~1 TB/100g | 18 TB/cartridge | **215 PB/gram** |
| **Durability** | 5–10 years | 30 years | **10,000+ years** |
| **Storage Energy** | 6–8W continuous | Climate-controlled | **Zero** (ambient stable) |
| **Carbon Footprint** | Ongoing (power + cooling) | Ongoing (climate control) | **One-time** (synthesis only) |

> *"DNA could store all of the world's data in one room."*
> — Science Magazine, reporting on Erlich & Zielinski (2017)

**1 gram of DNA = 215 petabytes = ~10,750 hard drives weighing 7 tonnes.**

70–80% of enterprise data is "cold" — accessed less than once per year, yet kept on power-hungry storage. DNA storage eliminates the continuous energy cost, potentially saving **millions of tonnes of CO₂ annually**.

---

## What DATA2DNA Does

DATA2DNA is a complete, research-grade pipeline that encodes arbitrary digital data into synthetic DNA oligonucleotides. It's designed for **text-based data** — CSV, JSON, SQL dumps, source code, scientific datasets — achieving extreme compression ratios before DNA encoding.

```
INPUT           ENCODE PIPELINE                                    OUTPUT
─────    ──────────────────────────────────────────────────    ─────────
         ┌─────────────────────────────────────────────┐
 Data ──►│ HyperCompress (BWT+MTF+BPE+ZSTD/Brotli)    │
  │      │ ↓                                           │
  │      │ Interleaved Reed-Solomon RS(255,223)        │      FASTA
  │      │ ↓                                           │──►  (DNA oligos
  │      │ Fountain Codes (Robust Soliton, 2.0× redun) │      ready for
  │      │ ↓                                           │      synthesis)
  │      │ DNA Transcoder (2-bit + rotation cipher)    │
  │      │ ↓                                           │
  │      │ Oligo Builder (primers + index + CRC-32)    │
  │      │ ↓                                           │
  │      │ DNA Constraint Screening                    │
  │      │ ↓                                           │
  │      │ FASTA Output + Cost Estimation              │
         └─────────────────────────────────────────────┘
```

### Key Capabilities

- **Multi-stage compression** — 3–16× on text data before DNA encoding (HyperCompress engine with parallel ZSTD-22/Brotli-11 trials)
- **Triple-layer error correction** — Reed-Solomon RS(255,223) + Interleaved RS (cross-oligo) + Fountain codes with Robust Soliton distribution
- **30% oligo loss recovery** — mathematically guaranteed with 2.0× redundancy (40% safety margin)
- **Biological compatibility** — GC content 40–60%, homopolymer ≤3, restriction enzyme screening, primer compatibility
- **Full decode pipeline** — FASTA → disassemble oligos → fountain decode → RS error correction → decompress → original data
- **Chaos simulation** — simulate DNA degradation (oligo loss, base substitutions, insertions, deletions)
- **Web interface** — Actix-web server with real-time SSE progress, drag-and-drop, encode/decode/chaos/benchmark tabs
- **Cost estimation** — per-oligo synthesis pricing based on Twist/IDT/GenScript rates

---

## Quick Start

### Prerequisites

- [Rust](https://rustup.rs/) (edition 2021, stable toolchain)

### Build & Run

```bash
# Clone
git clone https://github.com/vedLinuxian/DATA2DNA.git
cd DATA2DNA

# Build (optimized)
cargo build --release

# Run server (default port 5000)
cargo run --release

# Or specify a port
PORT=8080 cargo run --release
./run.sh --port 3000
```

Open [http://localhost:5000](http://localhost:5000) in your browser.

### Run Tests

```bash
# All 151 tests (70 unit + 81 integration)
cargo test

# Integration tests only
cargo test --test integration_tests

# With output
cargo test -- --nocapture
```

---

## Architecture

### Encode Pipeline (8 Stages)

| Stage | Module | Purpose |
|-------|--------|---------|
| 1 | `hypercompress` | Multi-stage maximum compression (BWT+MTF+ZRLE → BPE → ZSTD-22/Brotli-11) |
| 2 | `interleaved_rs` | Cross-oligo Reed-Solomon RS(255,223) error correction |
| 3 | `transcoder` | Binary → DNA (2-bit encoding with rotation cipher for GC balance) |
| 4 | `fountain` | LT Fountain codes with Robust Soliton distribution (2.0× redundancy) |
| 5 | `oligo_builder` | Structured oligos: Forward Primer + Index + Payload + CRC-32 + Reverse Primer |
| 6 | `dna_constraints` | GC content, homopolymer, restriction enzyme screening |
| 7 | `fasta` | FASTA output with embedded metadata for standalone decode |
| 8 | `cost_estimator` | Synthesis/sequencing cost at commercial rates |

### Decode Pipeline (4 Stages)

```
FASTA → Oligo Disassemble → Fountain Decode → RS Error Correction → Decompress → Original Data
```

### Error Correction Stack

```
Layer 1: CRC-32 per oligo          — detects corruption in individual oligos
Layer 2: Interleaved RS(255,223)   — corrects up to 16 oligo losses per group
Layer 3: Fountain Codes (2.0×)     — survives 30% total oligo loss
         Combined: mathematically guaranteed recovery at stated loss rates
```

### Redundancy Math

```
surviving_data = redundancy × (1 - loss_rate)

At 30% loss with 2.0× redundancy:
  2.0 × 0.7 = 1.40  →  40% safety margin  ✓

At 40% loss with 2.5× redundancy:
  2.5 × 0.6 = 1.50  →  50% safety margin  ✓
```

---

## Module Map

| Module | Lines | Description |
|--------|-------|-------------|
| `hypercompress.rs` | 2,471 | Multi-stage maximum compression engine |
| `pipeline.rs` | 1,113 | Pipeline orchestrator connecting all modules |
| `main.rs` | 1,529 | Actix-web HTTP server with SSE progress |
| `oligo_builder.rs` | 534 | Structured oligo construction with CRC-32 |
| `fountain.rs` | 529 | LT Fountain codes with Robust Soliton |
| `reed_solomon.rs` | 449 | GF(2⁸) RS codec with Berlekamp-Massey |
| `interleaved_rs.rs` | 378 | Cross-oligo RS protection |
| `fasta.rs` | 319 | FASTA I/O with metadata embedding |
| `transcoder.rs` | 274 | 2-bit DNA encoding with rotation cipher |
| `chaos.rs` | ~160 | DNA degradation simulation |
| `cost_estimator.rs` | ~150 | Synthesis cost modeling |
| `dna_constraints.rs` | ~200 | Biological constraint screening |
| `consensus.rs` | ~120 | Consensus decoder |
| `compressor.rs` | ~200 | Legacy compressor (backwards compat) |

---

## Technical Parameters

| Parameter | Value | Rationale |
|-----------|-------|-----------|
| Oligo length | 300 bp | Twist Bioscience / IDT standard |
| Overhead per oligo | 72 bp | 20bp forward primer + 16bp index + 16bp CRC + 20bp reverse primer |
| Payload per oligo | 228 bp | 300 - 72 = 228 bp useful data |
| Payload efficiency | 76% | 228/300 |
| RS code | RS(255,223) | 32 parity symbols, 16-error correction |
| Fountain distribution | Robust Soliton | c=0.025, δ=0.001 (DNA Fountain paper) |
| Default redundancy | 2.0× | Survives 30% loss with 40% margin |
| Block size | 64 bytes | RS alignment |
| GF polynomial | 0x11D | x⁸ + x⁴ + x³ + x² + 1 |
| Primers | CTACACGACGCTCTTCCGAT / AGACGTGTGCTCTTCCGATC | 20bp each |

---

## Research & Motivation

### The Problem

Global data centers consume **240–340 TWh of electricity annually** (IEA, 2024), roughly 1–1.3% of global electricity demand. A single hyperscale data center draws **20–50 MW** continuously.

Yet **70–80% of enterprise data is cold** — stored for compliance, archival, or disaster recovery, accessed less than once per year. This cold data sits on spinning disks and tape libraries consuming energy 24/7.

### The DNA Solution

DNA offers a fundamentally different storage paradigm:

- **Density**: 215 PB/gram (Erlich & Zielinski, Science 2017) — 1 gram replaces 10,750 hard drives
- **Durability**: 10,000+ years when properly stored (Grass et al., 2015, demonstrated stability with silica encapsulation)
- **Zero storage energy**: A dried DNA sample in a tube requires no power, no cooling, no hardware refresh cycles
- **Carbon impact**: Moving even 1% of cold data to DNA could save ~3.6 million tonnes CO₂/year

### Academic Foundation

This project builds on peer-reviewed research:

1. **Erlich & Zielinski** (2017). "DNA Fountain enables a robust and efficient storage architecture." *Science*, 355(6328), 950–954. DOI: [10.1126/science.aaj2038](https://doi.org/10.1126/science.aaj2038)
2. **Church, Gao & Kosuri** (2012). "Next-Generation Digital Information Storage in DNA." *Science*, 337(6102), 1628. DOI: [10.1126/science.1226355](https://doi.org/10.1126/science.1226355)
3. **Goldman et al.** (2013). "Towards practical, high-capacity, low-maintenance information storage in synthesized DNA." *Nature*, 494(7435), 77–80. DOI: [10.1038/nature11875](https://doi.org/10.1038/nature11875)
4. **Organick et al.** (2018). "Random access in large-scale DNA data storage." *Nature Biotechnology*, 36(3), 242–248. DOI: [10.1038/nbt.4079](https://doi.org/10.1038/nbt.4079)
5. **Grass et al.** (2015). "Robust chemical preservation of digital information on DNA in silica with error-correcting codes." *Angewandte Chemie*, 54(8), 2552–2555. DOI: [10.1002/anie.201411378](https://doi.org/10.1002/anie.201411378)
6. **Ceze, Nivala & Strauss** (2019). "Molecular digital data storage using DNA." *Nature Reviews Genetics*, 20(8), 456–466. DOI: [10.1038/s41576-019-0125-3](https://doi.org/10.1038/s41576-019-0125-3)
7. **Zhirnov et al.** (2016). "Nucleic acid memory." *Nature Materials*, 15(4), 366–370. DOI: [10.1038/nmat4594](https://doi.org/10.1038/nmat4594)
8. **Luby** (2002). "LT codes." *Proc. 43rd IEEE FOCS*, 271–280.

### Competitive Landscape

| System | Year | Density (bits/nt) | Error Correction | Status |
|--------|------|-------------------|-----------------|--------|
| Church et al. | 2012 | ~0.83 | Repetition | Paper |
| Goldman et al. | 2013 | ~0.33 | Fourfold redundancy | Paper |
| DNA Fountain | 2017 | 1.57 | RS + Fountain | Paper |
| Microsoft/UW | 2018 | ~1.10 | RS + Repetition | Paper |
| Catalog DNA | 2021 | ~0.70 | Proprietary | Commercial |
| **DATA2DNA** | **2025** | **~1.6*** | **RS + IRS + Fountain** | **Open Source** |

*Effective density with HyperCompress on text data reaches 6–12 bits of original data per nucleotide.

---

## Use Cases

### 1. Long-Term Archival (Government & Cultural Heritage)
DNA lasts 10,000+ years without energy. Libraries, museums, national archives can preserve digital collections permanently.

### 2. Scientific Dataset Preservation
Genomics, climate, astronomy — petabytes growing exponentially. DNA provides a medium that outlasts any technology cycle.

### 3. Regulatory Compliance
HIPAA (7 years), SOX (7 years), SEC Rule 17a-4 (6 years), GDPR (variable). DNA eliminates hardware refresh cycles during retention periods.

### 4. Cold Data Carbon Reduction
Moving cold data from spinning disks to DNA eliminates continuous energy consumption (6–8W per drive × millions of drives × 24/7).

### 5. Disaster Recovery
DNA is physically transportable (1g = 215 PB), radiation-resistant, and doesn't require specific hardware to read — only a sequencer, which gets cheaper every year.

---

## API Endpoints

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/api/encode` | POST | Encode data to DNA (multipart file upload) |
| `/api/decode-fasta` | POST | Decode from FASTA file |
| `/api/chaos` | POST | Apply chaos simulation |
| `/api/decode` | POST | Decode after chaos |
| `/api/benchmark` | POST | Run benchmark suite |
| `/api/progress/{id}` | GET | Poll task progress |
| `/api/events/{id}` | GET | SSE progress stream |
| `/api/download/{type}/{id}` | GET | Download FASTA/data |
| `/api/config` | GET | Get pipeline configuration |
| `/api/health` | GET | Health check |

---

## Project Structure

```
DATA2DNA/
├── src/
│   ├── main.rs              # Actix-web HTTP server
│   ├── lib.rs               # Module declarations
│   ├── pipeline.rs          # Pipeline orchestrator
│   ├── hypercompress.rs     # Multi-stage compression engine
│   ├── reed_solomon.rs      # RS(255,223) codec
│   ├── interleaved_rs.rs    # Cross-oligo RS protection
│   ├── fountain.rs          # LT Fountain codes
│   ├── transcoder.rs        # DNA ↔ binary transcoding
│   ├── oligo_builder.rs     # Structured oligo construction
│   ├── dna_constraints.rs   # Biological constraint screening
│   ├── fasta.rs             # FASTA I/O
│   ├── chaos.rs             # Degradation simulation
│   ├── cost_estimator.rs    # Synthesis cost modeling
│   ├── consensus.rs         # Consensus decoder
│   └── compressor.rs        # Legacy compressor
├── static/
│   ├── index.html           # Web interface
│   ├── script.js            # Frontend controller
│   └── style.css            # Biotech-themed styling
├── tests/
│   └── integration_tests.rs # 81 integration tests
├── .github/
│   ├── workflows/ci.yml     # CI/CD pipeline
│   ├── agents/              # 7 specialized AI agents
│   └── copilot-instructions.md
├── Cargo.toml
├── README.md
├── RESEARCH.md              # Detailed research documentation
├── CONTRIBUTING.md
├── LICENSE
└── run.sh                   # Port-configurable launcher
```

---

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines. We welcome contributions in:

- Error correction algorithms
- Compression optimizations for specific data formats
- DNA constraint screening improvements
- Random access mechanisms
- Cost model updates with current vendor pricing
- Test coverage expansion

---

## License

Dual licensed under **AGPL-3.0** (open source) and a **Commercial License** (proprietary use).

- Open-source use: [LICENSE](LICENSE) (AGPL-3.0)
- Commercial use: [LICENSE-COMMERCIAL.md](LICENSE-COMMERCIAL.md)
- Overview: [LICENSING.md](LICENSING.md)

Contact vedcimit@gmail.com for commercial licensing inquiries.

---

## Citation

If you use DATA2DNA in research, please cite:

```bibtex
@software{data2dna2025,
  title={DATA2DNA: Encoding Digital Data Into Synthetic DNA},
  author={Ved},
  email={vedcimit@gmail.com},
  year={2025},
  url={https://github.com/vedLinuxian/DATA2DNA},
  note={Open-source DNA data storage pipeline with triple-layer error correction}
}
```

---

<p align="center">
  <i>Every byte saved is an oligo you didn't need to synthesize — and that's carbon you didn't emit.</i>
</p>
