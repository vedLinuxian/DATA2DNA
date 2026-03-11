# Contributing to DATA2DNA

## Before You Contribute

1. Read the [Licensing Overview](LICENSING.md)
2. Sign the [CLA](CLA.md) by adding your name to CONTRIBUTORS.md
3. Open an issue before starting large changes

## Development Setup

```bash
git clone https://github.com/vedLinuxian/DATA2DNA.git
cd DATA2DNA
cargo build
cargo test  # Should show 151 passing
```

## What We Need

### High Priority
- [ ] Wet-lab validated oligo sequences (if you have sequencing access)
- [ ] Random access mechanism (PCR-based file selection, ref: Organick 2018)
- [ ] Nanopore sequencing decoder (real-time, long reads)
- [ ] Indel error correction (HEDGES code integration, Press et al. 2020)

### Medium Priority
- [ ] Domain-specific compression (FASTA/genomics, FITS/astronomy)
- [ ] Cost model updates with 2025 vendor pricing
- [ ] Python bindings (PyO3)
- [ ] WASM build for browser-side encoding

### Research Contributions
- [ ] Benchmarks against DNA Fountain reference implementation
- [ ] Biological constraint validation data
- [ ] Sequencing error profile datasets

## Code Standards

- All new modules must have unit tests (target: 90%+ coverage)
- Run `cargo clippy` before submitting — zero warnings policy
- Run `cargo fmt` — consistent formatting required
- Document public APIs with `///` doc comments
- No unsafe code without explicit justification in comments

## Pull Request Process

1. Fork → branch → commit → PR
2. All CI checks must pass
3. At least one review required for merge
4. Update CHANGELOG.md with your change
5. Add your name to CONTRIBUTORS.md (CLA requirement)

## Research PRs

If you're contributing based on a paper, cite it in:
- The PR description
- Inline code comments at the relevant section
- RESEARCH.md if it's a foundational addition
