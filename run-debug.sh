#!/bin/bash
# Run Epoca browser with debug logging to the console.
# Usage:
#   ./run-debug.sh              # default: info level for epoca, warn for deps
#   ./run-debug.sh trace        # trace level for epoca crates
#   ./run-debug.sh "epoca=debug,gpui=info"  # custom filter

FILTER="${1:-epoca=debug,epoca_core=debug,epoca_wallet=debug,epoca_broker=debug,epoca_shield=info,warn}"

echo "Building Epoca (debug)..."
cargo build -p epoca 2>&1 | tail -3

if [ $? -ne 0 ]; then
    echo "Build failed."
    exit 1
fi

echo "Running with RUST_LOG=$FILTER"
RUST_LOG="$FILTER" cargo run -p epoca -- "${@:2}"
