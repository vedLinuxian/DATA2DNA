# Project Helix-Core — Copilot Instructions

## Project Overview
Helix-Core is a DNA data storage system written in Rust (edition 2021). It encodes arbitrary digital data into synthetic DNA oligonucleotides with extreme fault tolerance, using Reed-Solomon codes, Fountain/LT codes, and multi-stage compression.

## Target Data Formats
This system is optimized for **text-based data only**:
- CSV, TSV, and tabular data
- JSON, JSONL, and structured data
- SQL dumps and database exports
- Source code (any language)
- Scientific datasets (FASTA genomics, measurement logs, etc.)
- Plain text, logs, configuration files

**NOT supported**: Images, video, audio, pre-compressed archives. These formats are already at Shannon entropy limits and cannot benefit from our compression pipeline.

## Architecture
```
ENCODE: Data → HyperCompress → InterleavedRS → Fountain → Transcode → OligoBuilder → Constraints → FASTA → Cost
DECODE: FASTA → OligoDisassemble → Fountain(decode) → InterleavedRS(decode) → Decompress → Data
CHAOS:  Simulate DNA degradation (oligo loss, substitution, insertion errors)
```

## Key Technical Parameters
- **Oligo length**: 300bp (72bp overhead: primers + index + CRC)
- **Block size**: 64 bytes (RS alignment)
- **Redundancy**: 2.0x (survives 30% loss with 40% margin)
- **RS code**: RS(255,223) in GF(2^8), 16-error correction per block
- **Fountain code**: Robust Soliton distribution (c=0.025, delta=0.001)
- **Compression**: BWT+MTF+ZRLE → BPE → ZSTD-22/Brotli-11 (parallel trials, best wins)

## Coding Standards
- Rust edition 2021, target stable toolchain
- Use `anyhow::Result` for error propagation (no `.unwrap()` on user data)
- Parallel computation via Rayon (compression trials, oligo screening)
- Actix-web 4 for the HTTP server with SSE progress reporting
- All public APIs must have doc comments
- Every pipeline change must pass `cargo test` (70+ existing tests)

## Module Map
| Module | Purpose |
|--------|---------|
| `hypercompress.rs` | Multi-stage maximum compression engine |
| `pipeline.rs` | Pipeline orchestrator connecting all modules |
| `fountain.rs` | Fountain/LT codes with Soliton distribution |
| `reed_solomon.rs` | GF(2^8) RS codec with Berlekamp-Massey |
| `interleaved_rs.rs` | Cross-oligo RS protection |
| `transcoder.rs` | 2-bit DNA encoding with rotation cipher |
| `oligo_builder.rs` | Structured oligo construction with CRC |
| `dna_constraints.rs` | Biological constraint screening |
| `fasta.rs` | FASTA I/O with metadata embedding |
| `cost_estimator.rs` | Commercial synthesis cost modeling |
| `chaos.rs` | DNA degradation simulation |
| `consensus.rs` | Consensus decoder (fountain wrapper) |
| `compressor.rs` | Legacy single-algorithm compressor |
| `main.rs` | Actix-web HTTP server |

## Available Agents
Activate these specialized agents in `.github/agents/`:
- **DNA Systems Architect** — Core pipeline architecture
- **Compression Research Scientist** — Compression optimization for text formats
- **Error Correction Engineer** — RS, Fountain codes, recovery guarantee math
- **Performance & Benchmark Engineer** — Profiling, benchmarking, cost analysis
- **Research Scientist** — Deep research, use cases, competitive analysis
- **Test & Verification Engineer** — Testing, chaos simulation, QA
- **Data Pipeline Engineer** — Data flow, format detection, streaming
