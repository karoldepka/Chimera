#!/usr/bin/env bash
# Self-hosting check, stage 1: the Chimera-written lexer (compiled to Rust) must produce
# exactly the same token stream as the Rust lexer for every .chim file in the repo.
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build -q --release
C=target/release/chimera
$C build selfhost/lexer.chim -o build/lexer >/dev/null 2>&1
LEX=build/lexer/target/release/lexer
pass=0; fail=0
for f in $(find examples selfhost tests -name '*.chim' | sort); do
  if diff <($C tokens "$f") <($LEX "$f") >/dev/null; then pass=$((pass+1)); else fail=$((fail+1)); echo "MISMATCH: $f"; fi
done
echo "self-hosted lexer: $pass identical, $fail mismatched"
[ "$fail" -eq 0 ]
