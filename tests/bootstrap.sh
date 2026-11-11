#!/usr/bin/env bash
# Self-hosting bootstrap:
#   stage 0 (Rust, src/)            compiles selfhost/*.chim -> stage 1
#   stage 1 (Chimera, via Rust)     compiles selfhost/*.chim -> stage 2
#   stage 2                         must emit exactly what stage 1 emits (fixpoint)
# plus: every stage emits byte-identical Rust for every example, stage 2 runs every
# example with the golden output, and the self-hosted lexer/checker agree with stage 0.
set -uo pipefail
cd "$(dirname "$0")/.."
cargo build -q --release || exit 1
S0=target/release/chimera
mkdir -p build
pass=0; fail=0
ok()  { pass=$((pass+1)); }
bad() { fail=$((fail+1)); echo "FAIL: $1"; }
same() { if cmp -s "$2" "$3"; then ok; else bad "$1"; fi; }

echo "== stage 0 -> stage 1"
$S0 build selfhost/main.chim -o build/stage1 >/dev/null 2>&1 || { echo "stage 1 build failed"; exit 1; }
S1=build/stage1/target/release/main
echo "== stage 1 -> stage 2"
$S1 build selfhost/main.chim -o build/stage2 >/dev/null 2>&1 || { echo "stage 2 build failed"; exit 1; }
S2=build/stage2/target/release/main

echo "== fixpoint"
$S0 emit selfhost/main.chim > build/self.s0.rs
$S1 emit selfhost/main.chim > build/self.s1.rs
$S2 emit selfhost/main.chim > build/self.s2.rs
same "stage0 vs stage1 output for the compiler itself" build/self.s0.rs build/self.s1.rs
same "stage1 vs stage2 output for the compiler itself (fixpoint)" build/self.s1.rs build/self.s2.rs
same "stage1 vs stage2 crate" build/stage1/src/main.rs build/stage2/src/main.rs

echo "== examples: identical Rust from every stage, golden output from stage 2"
for f in examples/*.chim; do
  n=$(basename "$f" .chim)
  $S0 emit "$f" > "build/$n.s0.rs"; $S2 emit "$f" > "build/$n.s2.rs"
  same "$n: emitted Rust" "build/$n.s0.rs" "build/$n.s2.rs"
  $S2 run "$f" -o "build/s2_$n" > "build/$n.s2.out" 2>&1
  same "$n: run output" "tests/expected/$n.out" "build/$n.s2.out"
done

echo "== self-hosted lexer vs stage 0 lexer"
for f in $(find examples selfhost tests -name '*.chim' | sort); do
  $S0 tokens "$f" > build/tok.s0; $S2 tokens "$f" > build/tok.s2
  same "tokens: $f" build/tok.s0 build/tok.s2
done

echo "== structural diagnostics (stage 2 vs stage 0)"
for n in classes contracts ordinal_suffix; do
  $S2 check "tests/errors/$n.chim" > "build/err_$n.s2" 2>&1
  same "diagnostics: $n" "tests/expected/err_$n.out" "build/err_$n.s2"
done

echo "$pass passed, $fail failed"
[ "$fail" -eq 0 ]
