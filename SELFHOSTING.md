# Self-hosting plan

**Goal (milestone 1):** `chimera` is written in Chimera, compiled by the Rust-hosted compiler (stage 0) into a Rust binary (stage 1). Stage 1 then compiles its own sources into a stage-2 binary whose output is identical. That is the classic bootstrap fixpoint.

The Rust compiler in `src/` is **stage 0**. It is the reference implementation, and every self-hosted component is tested against it on the whole corpus.

| Step | Component | Chimera source | Verified by | Status |
|---|---|---|---|---|
| 1 | Lexer | `selfhost/lexer.chim` | token dump == `chimera tokens` on every `.chim` file (`tests/selfhost.sh`) | ✅ done |
| 2 | Parser + AST | `selfhost/parser.chim` | AST dump == `chimera ast` (a new stage-0 command) | next |
| 3 | Desugaring | `selfhost/desugar.chim` | desugared AST dump | |
| 4 | Checker | `selfhost/sema.chim` | diagnostics == `tests/expected/err_*.out` | |
| 5 | Rust backend + native lowering | `selfhost/gen_rust.chim` | generated `main.rs` byte-identical to stage 0's | |
| 6 | Driver | `selfhost/main.chim` | `tests/run.sh` passes with the stage-1 binary | |
| 7 | Fixpoint | — | stage1(compiler) == stage2(compiler) | |

## Language work the later stages need

* **Modules:** `import lexer` across files, so the compiler isn't one giant file (needed by step 2).
* **Enum methods and recursive enum types** for the AST (`enum Expr { Binary(op: String, l: Expr, r: Expr), … }`). Runtime support exists already; the checker needs recursive aliases.
* **`StringBuilder`-style output:** `s = s & x` is already an in-place append in the Rust backend, and `xs = xs.push(x)` / `m = m.assoc(k, v)` / `this.f = this.f.push(x)` are amortised O(1) through `Rc::make_mut`.
* **Native lowering for more types** (strings, vectors of natives, data classes → Rust `struct`s), so the self-hosted compiler runs at a speed comparable to stage 0.
* **`chimera ast` and `chimera desugar`** dump commands in stage 0, to diff against.

## Why the lexer first

The lexer is the smallest component that exercises everything a compiler written in Chimera needs: file I/O (`readFile`, `args`, `exit`), string and char processing, classes with mutable state, persistent vectors with in-place fast paths, `Result` error handling, `match`, and string building. It also gives a byte-exact oracle to test against. The self-hosted lexer tokenizes its own source correctly, and that is part of the test.
