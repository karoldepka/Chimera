#!/usr/bin/env bash
# Regression suite: every example must produce its golden output on BOTH backends, every
# file in tests/errors must fail with exactly the expected diagnostics, and the lexer must
# produce the golden token dump.
set -uo pipefail
cd "$(dirname "$0")/.."
[ -x bin/chimera ] || ./build.sh || exit 1
C=bin/chimera
mkdir -p build
pass=0; fail=0
check() { if diff -u "$2" "$3" >/dev/null; then pass=$((pass+1)); else fail=$((fail+1)); echo "FAIL: $1"; diff -u "$2" "$3" | head -20; fi; }
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
$C tokens examples/basics.chim > build/tokens_basics.out 2>&1
check "tokens" tests/expected/tokens_basics.out build/tokens_basics.out
echo "$pass passed, $fail failed"
[ "$fail" -eq 0 ]
