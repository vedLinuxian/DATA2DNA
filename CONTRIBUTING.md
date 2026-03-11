# Contributing to DATA2DNA

Thank you for your interest in contributing to DATA2DNA! This project is building the future of archival data storage using synthetic DNA, and every contribution matters.

## Getting Started

### Prerequisites

- **Rust** (edition 2021, stable toolchain): [Install Rust](https://rustup.rs/)
- **Git**: For version control

### Setup

```bash
git clone https://github.com/vedLinuxian/DATA2DNA.git
cd DATA2DNA
cargo build
cargo test  # All 151 tests should pass
```

## Development Workflow

### 1. Find or Create an Issue

Before starting work, check for existing issues or create a new one describing the change you'd like to make.

### 2. Create a Branch

```bash
git checkout -b feature/your-feature-name
# or
git checkout -b fix/your-bug-fix
```

### 3. Make Your Changes

Follow our coding standards:

- **Error handling**: Use `anyhow::Result` for error propagation. No `.unwrap()` on user data paths.
- **Parallelism**: Use Rayon for parallel computation (compression trials, oligo screening), not for sequential pipeline stages.
- **Documentation**: All public APIs must have doc comments.
- **Tests**: Every bug fix needs a regression test. Every new feature needs unit + integration tests.

### 4. Test Your Changes

```bash
# Run all tests (must pass)
cargo test

# Check for warnings
cargo check 2>&1

# Format check
cargo fmt -- --check

# Lint
cargo clippy
```

### 5. Submit a Pull Request

- Write a clear title and description
- Reference any related issues
- Ensure CI passes

## Areas Where We Need Help

### Error Correction
- Improved fountain code decoding (Gaussian elimination fallback optimization)
- Adaptive redundancy based on data characteristics
- Random access mechanisms for selective oligo retrieval

### Compression
- Format-specific preprocessing for CSV, JSON, SQL
- Dictionary training for domain-specific data
- Streaming compression for large files

### DNA Constraints
- Updated restriction enzyme databases
- Improved GC balancing algorithms
- Secondary structure prediction (hairpin avoidance)

### Testing
- Fuzz testing with `cargo-fuzz`
- Property-based testing for GF(2^8) arithmetic
- Stress tests with large datasets (>100MB)

### Documentation
- Tutorials for specific use cases
- API documentation improvements
- Performance benchmarking guides

### Research
- Cost model updates with current synthesis vendor pricing
- New use case analysis and validation
- Comparison studies with other DNA storage systems

## Code Style

- Follow Rust 2021 edition conventions
- Use `snake_case` for functions and variables
- Use `PascalCase` for types and traits
- Keep functions under 100 lines where possible
- Comment complex algorithms with references to papers

## Data Format Focus

This system is optimized for **text-based data only**:
- CSV, TSV, tabular data
- JSON, JSONL, structured data
- SQL dumps, database exports
- Source code (any language)
- Scientific datasets (FASTA genomics, measurement logs)
- Plain text, logs, configuration files

**NOT supported**: Images, video, audio, pre-compressed archives (these are already at Shannon entropy limits).

## Commit Messages

Use clear, descriptive commit messages:

```
feat: add adaptive redundancy based on data entropy
fix: correct off-by-one in Chien search for RS decoder
test: add chaos recovery tests at 40% loss rate
docs: update cost model with 2025 Twist pricing
perf: optimize fountain encoding with SIMD
```

## License

By contributing, you agree that your contributions will be licensed under the MIT License.

---

Thank you for helping build the future of data storage! 🧬
