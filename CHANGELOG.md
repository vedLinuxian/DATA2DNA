# Changelog

All notable changes to DATA2DNA will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [5.0.0] — 2025-03-11

### Added
- Full 8-stage encode/decode pipeline
- HyperCompress engine: BWT+MTF+ZRLE+BPE → parallel ZSTD-22/Brotli-11 trials
- Reed-Solomon RS(255,223) in GF(2^8) with Berlekamp-Massey decoder
- Interleaved Reed-Solomon for cross-oligo error protection
- Hybrid systematic/LT Fountain codes with Robust Soliton distribution (c=0.025, δ=0.001)
- 2-bit DNA transcoder with rotation cipher
- Structured oligo builder (300bp: primers + index + CRC + payload)
- Biological constraint screening (GC%, homopolymers, restriction enzymes)
- FASTA I/O with metadata embedding
- Commercial synthesis cost estimator (Twist/IDT/GenScript rates)
- Chaos engine for DNA degradation simulation
- Actix-Web 4 HTTP server with SSE progress reporting
- Web UI with 5-tab SPA interface
- 153 tests (72 unit + 81 integration), all passing
- Multi-file upload support
- FASTA round-trip decode
- AGPL-3.0 + Commercial dual licensing
- Research documentation with academic references
- GitHub Actions CI/CD pipeline
- GitHub Pages scientific website

### Known Limitations
- No wet-lab validation (physical synthesis not yet performed)
- No RS erasure decoding (error correction only, not erasure mode)
- No random access (whole-archive decode only)
- Substitution-only chaos model (no indel simulation)
- Compression ineffective on pre-compressed/binary data
