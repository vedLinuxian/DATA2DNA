#!/usr/bin/env bash
set -e

PROJECT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$PROJECT_DIR"

# ── Configuration ────────────────────────────────────────────────
# Usage:  ./start.sh                    → build + run on port 5000
#         ./start.sh --port 8080        → custom port
#         ./start.sh --bench            → run benchmarks after build
#         ./start.sh --test             → run tests, then start server
#         ./start.sh --release          → release build (default)
#         ./start.sh --debug            → debug build (faster compile)
#         PORT=3000 ./start.sh          → env var port

PORT="${PORT:-5000}"
BUILD_MODE="release"
RUN_TESTS=false
RUN_BENCH=false
OPEN_BROWSER=false

while [[ $# -gt 0 ]]; do
    case "$1" in
        --port|-p)
            PORT="$2"; shift 2 ;;
        --test|-t)
            RUN_TESTS=true; shift ;;
        --bench|-b)
            RUN_BENCH=true; shift ;;
        --debug|-d)
            BUILD_MODE="debug"; shift ;;
        --release|-r)
            BUILD_MODE="release"; shift ;;
        --open|-o)
            OPEN_BROWSER=true; shift ;;
        [0-9]*)
            PORT="$1"; shift ;;
        --help|-h)
            cat << 'EOF'
DATA2DNA — Start Script

Usage: ./start.sh [OPTIONS] [PORT]

Options:
  --port, -p PORT     Set server port (default: 5000)
  --test, -t          Run test suite before starting server
  --bench, -b         Run cargo benchmarks after build
  --debug, -d         Debug build (faster compile, slower runtime)
  --release, -r       Release build (default — slower compile, faster runtime)
  --open, -o          Open browser after server starts
  --help, -h          Show this help

Examples:
  ./start.sh                       # Build release + start on :5000
  ./start.sh --port 8080           # Custom port
  ./start.sh --test                # Run 183 tests, then start server
  ./start.sh --test --bench        # Tests + benchmarks + server
  PORT=3000 ./start.sh --debug     # Debug build on port 3000
EOF
            exit 0 ;;
        *)
            echo "Unknown option: $1 (try --help)"; exit 1 ;;
    esac
done

if ! [[ "$PORT" =~ ^[0-9]+$ ]] || [ "$PORT" -lt 1 ] || [ "$PORT" -gt 65535 ]; then
    echo "Error: Invalid port '$PORT'. Must be 1-65535."; exit 1
fi

# ── Banner ───────────────────────────────────────────────────────
echo ""
echo "  ╔══════════════════════════════════════════════════════╗"
echo "  ║         🧬 DATA2DNA v5.0 — DNA Data Storage         ║"
echo "  ║                                                      ║"
echo "  ║  HyperCompress → RS(255,223) → Fountain/LT          ║"
echo "  ║  → Transcode → OligoBuilder → Constraints → FASTA   ║"
echo "  ╚══════════════════════════════════════════════════════╝"
echo ""
echo "  Mode: ${BUILD_MODE} | Port: ${PORT} | Tests: ${RUN_TESTS} | Bench: ${RUN_BENCH}"
echo ""

# ── Step 1: Build ────────────────────────────────────────────────
STEP=1
TOTAL_STEPS=$((1 + ($RUN_TESTS && echo 1 || echo 0) + ($RUN_BENCH && echo 1 || echo 0) + 1))

echo "[${STEP}] Building (${BUILD_MODE})..."
if [ "$BUILD_MODE" = "release" ]; then
    cargo build --release 2>&1
    BINARY="./target/release/helix-core"
else
    cargo build 2>&1
    BINARY="./target/debug/helix-core"
fi
echo "    ✓ Build complete"
STEP=$((STEP + 1))

# ── Step 2: Tests (optional) ────────────────────────────────────
if [ "$RUN_TESTS" = true ]; then
    echo ""
    echo "[${STEP}] Running test suite..."
    cargo test 2>&1
    echo "    ✓ All tests passed"
    STEP=$((STEP + 1))
fi

# ── Step 3: Benchmarks (optional) ───────────────────────────────
if [ "$RUN_BENCH" = true ]; then
    echo ""
    echo "[${STEP}] Running benchmarks..."
    # Start server briefly for API benchmarks
    export PORT
    RUST_LOG=warn $BINARY &
    SERVER_PID=$!
    sleep 2

    # Quick benchmark via API
    echo "    Sending benchmark request to http://localhost:${PORT}/api/benchmark ..."
    if command -v curl &> /dev/null; then
        BENCH_RESULT=$(curl -s -X POST "http://localhost:${PORT}/api/benchmark" \
            -H "Content-Type: application/json" \
            -d '{}' 2>&1) || true
        echo "    Benchmark response received"
        echo "$BENCH_RESULT" | head -100
    else
        echo "    curl not found — skipping API benchmark"
    fi

    kill $SERVER_PID 2>/dev/null || true
    wait $SERVER_PID 2>/dev/null || true
    echo "    ✓ Benchmarks complete"
    STEP=$((STEP + 1))
fi

# ── Step 4: Start Server ────────────────────────────────────────
echo ""
echo "[${STEP}] Starting server..."
echo ""
echo "    🌐 http://localhost:${PORT}          — Web UI"
echo "    🌐 http://localhost:${PORT}/benchmark — Benchmark App"
echo "    📡 http://localhost:${PORT}/api/health — Health Check"
echo ""
echo "    Press Ctrl+C to stop."
echo ""

# Open browser if requested
if [ "$OPEN_BROWSER" = true ]; then
    URL="http://localhost:${PORT}"
    if command -v xdg-open &> /dev/null; then
        (sleep 2 && xdg-open "$URL") &
    elif command -v open &> /dev/null; then
        (sleep 2 && open "$URL") &
    fi
fi

export PORT
RUST_LOG=info $BINARY
