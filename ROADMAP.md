# DATA2DNA Roadmap

## Current Status: v5.0.0 (Software-Complete Encoder)

✅ Full encode/decode pipeline (151 tests passing)
✅ Triple-layer error correction (RS + IRS + Fountain)
✅ HyperCompress pipeline (BWT+MTF+ZRLE+BPE → ZSTD-22/Brotli-11)
✅ Biological constraint screening
✅ Web interface + REST API
✅ Cost estimation (Twist/IDT/GenScript rates)
✅ Chaos simulation for DNA degradation testing

---

## v5.1.0 — Wet-Lab Validation (Target: Q2 2026)

The critical milestone: physical synthesis and recovery.

- [ ] Synthesize 100–500 oligos from DATA2DNA output (via Twist/IDT)
- [ ] Sequence synthesized oligos (Illumina MiSeq or NovaSeq)
- [ ] Run decode pipeline on sequencing output
- [ ] Document end-to-end hash verification
- [ ] Publish results as preprint (bioRxiv)

**This milestone converts DATA2DNA from software demo to working prototype.**

---

## v5.2.0 — Random Access (Target: Q3 2026)

- [ ] PCR-based file selection (primer design per file)
- [ ] Metadata index structure for large archives
- [ ] Partial decode (retrieve single file from pool)
- Reference: Organick et al. 2018, Nature Biotechnology

---

## v5.3.0 — Sequencer Integration (Target: Q4 2026)

- [ ] Oxford Nanopore real-time streaming decoder
- [ ] HEDGES indel error correction (Press et al. 2020)
- [ ] Basecaller-aware quality scoring
- [ ] Direct MinION integration

---

## v6.0.0 — Production Archive System (Target: 2027)

- [ ] Multi-file archive format (DNA filesystem)
- [ ] Verified physical demo: 1 MB stored and recovered
- [ ] Performance benchmarks vs published systems
- [ ] Academic paper submission (target: Nature Methods / Nucleic Acids Research)

---

## Long-Term Vision

- DNA-native compression codecs (for genomic/climate/astronomical data)
- Integration with synthesis automation platforms
- Open standard for DNA archive format (like ZIP but for molecules)
- Python bindings (PyO3) and WASM build for browser-side encoding
