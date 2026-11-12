#!/usr/bin/env bash
# Regenerate the bootstrap seed from the current compiler sources. Run after changing
# compiler/*.chim or runtime/rt.rs, once tests/bootstrap.sh passes.
set -euo pipefail
cd "$(dirname "$0")/.."
[ -x bin/chimera ] || ./build.sh
bin/chimera emit compiler/chimera.chim > bootstrap/src/main.rs.new
mv bootstrap/src/main.rs.new bootstrap/src/main.rs
cp runtime/rt.rs bootstrap/src/rt.rs
echo "seed updated"
