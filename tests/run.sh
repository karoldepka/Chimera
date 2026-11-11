#!/usr/bin/env bash
# Regression suite: every example must produce its golden output on BOTH backends,
# and every file in tests/errors must fail with exactly the expected diagnostics.
set -uo pipefail
cd "$(dirname "$0")/.."
cargo build -q --release || exit 1
C=target/release/chimera
mkdir -p build
pass=0; fail=0
check() { if diff -u "$2" <(cat "$3") >/dev/null; then pass=$((pass+1)); else fail=$((fail+1)); echo "FAIL: $1"; diff -u "$2" "$3" | head -20; fi; }
for f in examples/*.chim; do
  n=$(basename "$f" .chim)
  $C run "$f" --target rust -o "build/ex_$n" > "build/$n.rust.out" 2>&1
  check "$n (rust)" "tests/expected/$n.out" "build/$n.rust.out"
  $C run "$f" --target js -o "build/$n.js" > "build/$n.js.out" 2>&1
  check "$n (js)" "tests/expected/$n.out" "build/$n.js.out"
done
for f in tests/errors/*.chim; do
  n=$(basename "$f" .chim)
  $C check "$f" > "build/err_$n.out" 2>&1
  check "errors/$n" "tests/expected/err_$n.out" "build/err_$n.out"
done
echo "$pass passed, $fail failed"
[ "$fail" -eq 0 ]
