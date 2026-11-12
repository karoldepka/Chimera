#!/usr/bin/env bash
# Self-hosting check:
#   seed    (bootstrap/, generated Rust)  builds  stage 1  from compiler/*.chim
#   stage 1                               builds  stage 2
#   stage 1 and stage 2 must emit identical Rust and JS for the compiler (fixpoint),
#   and the checked-in seed must be exactly what the compiler generates for itself.
set -uo pipefail
cd "$(dirname "$0")/.."
mkdir -p build
pass=0; fail=0
same() { if cmp -s "$2" "$3"; then pass=$((pass+1)); else fail=$((fail+1)); echo "FAIL: $1"; fi; }

echo "== seed -> stage 1"
./build.sh >/dev/null 2>&1 || { echo "stage 1 build failed"; exit 1; }
S1=bin/chimera
echo "== stage 1 -> stage 2"
$S1 build compiler/chimera.chim -o build/stage2 >/dev/null 2>&1 || { echo "stage 2 build failed"; exit 1; }
S2=build/stage2/target/release/chimera

echo "== fixpoint"
for t in rust js; do
  $S1 emit compiler/chimera.chim --target $t > build/self.s1.$t
  $S2 emit compiler/chimera.chim --target $t > build/self.s2.$t
  same "stage 1 vs stage 2 ($t) for the compiler itself" build/self.s1.$t build/self.s2.$t
done
same "stage 1 vs stage 2 crate" build/chimera/src/main.rs build/stage2/src/main.rs

echo "== seed is up to date"
same "bootstrap/src/main.rs (run scripts/update-seed.sh)" bootstrap/src/main.rs build/self.s1.rust
same "bootstrap/src/rt.rs (run scripts/update-seed.sh)" bootstrap/src/rt.rs runtime/rt.rs

echo "== stage 2 on the examples"
for f in examples/*.chim; do
  n=$(basename "$f" .chim)
  $S1 emit "$f" > "build/$n.s1.rs"; $S2 emit "$f" > "build/$n.s2.rs"
  same "$n: emitted Rust" "build/$n.s1.rs" "build/$n.s2.rs"
done

echo "$pass passed, $fail failed"
[ "$fail" -eq 0 ]
