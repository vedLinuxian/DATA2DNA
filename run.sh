#!/usr/bin/env bash
set -e

PROJECT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$PROJECT_DIR"

# ── Port Configuration ───────────────────────────────────────────
# Usage:  ./run.sh              → default port 5000
#         ./run.sh 8080         → port as positional arg
#         ./run.sh --port 9000  → port as named flag
#         PORT=8080 ./run.sh    → port via environment variable

PORT="${PORT:-5000}"

while [[ $# -gt 0 ]]; do
    case "$1" in
        --port|-p)
            PORT="$2"; shift 2 ;;
        [0-9]*)
            PORT="$1"; shift ;;
        --help|-h)
            echo "Usage: $0 [--port PORT] [PORT]"
            echo "  Default port : 5000"
            echo "  Env var      : PORT=8080 ./run.sh"
            exit 0 ;;
        *)
            echo "Unknown option: $1"; exit 1 ;;
    esac
done

if ! [[ "$PORT" =~ ^[0-9]+$ ]] || [ "$PORT" -lt 1 ] || [ "$PORT" -gt 65535 ]; then
    echo "Error: Invalid port '$PORT'. Must be 1-65535."; exit 1
fi

echo "============================================================"
echo "  Project Helix-Core v5.0 — DNA Data Storage OS"
echo "  Pipeline: Compress → RS(255,223) → Fountain → Transcode"
echo "           → OligoBuilder → Constraints → FASTA → Cost"
echo "============================================================"

echo ""
echo "[1/2] Building (release)..."
cargo build --release 2>&1

echo ""
echo "[2/2] Starting server on http://localhost:${PORT}"
echo "      Press Ctrl+C to stop."
echo ""

export PORT
RUST_LOG=info ./target/release/helix-core
