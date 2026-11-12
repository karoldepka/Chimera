#!/usr/bin/env bash
# Build the Chimera compiler from a clean checkout:
#   1. cargo builds the checked-in seed (bootstrap/), giving a first compiler
#   2. that compiler compiles the real sources (compiler/*.chim) into bin/chimera
set -euo pipefail
cd "$(dirname "$0")"
cargo build --release -q --manifest-path bootstrap/Cargo.toml
bootstrap/target/release/chimera build compiler/chimera.chim -o build/chimera
mkdir -p bin
cp build/chimera/target/release/chimera bin/chimera
echo "built bin/chimera"
