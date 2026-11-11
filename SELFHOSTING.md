# Self-hosting

**Milestone 1 is done ✅: the compiler is self-hosting.**

```
stage 0   src/*.rs            the reference compiler (Rust)
stage 1   selfhost/*.chim     compiled by stage 0 to Rust, then by cargo to a native binary
stage 2   selfhost/*.chim     compiled by stage 1
fixpoint  stage1(compiler) == stage2(compiler), byte for byte
```

Run `tests/bootstrap.sh` to rebuild every stage and verify (46 checks).

| Component | Chimera source | Verified by | Status |
|---|---|---|---|
| Lexer | `selfhost/lexer.chim` | same token stream as `chimera tokens` on every `.chim` file | ✅ |
| Parser + AST | `selfhost/parser.chim`, `ast.chim` | identical generated Rust for every program | ✅ |
| Desugaring | `selfhost/desugar.chim` | same (including temporary numbering) | ✅ |
| Structural checks | `selfhost/sema.chim` | identical diagnostics: classes, contracts, ordinal syntax | ✅ |
| Native lowering | `selfhost/native.chim` | identical native functions | ✅ |
| Rust backend | `selfhost/gen_rust.chim` | byte-identical `main.rs` | ✅ |
| Driver | `selfhost/main.chim` | imports, diagnostics, `check/emit/build/run/tokens` | ✅ |
| Fixpoint | | stage 1 output == stage 2 output | ✅ |

## What bootstrapping found

Compiling the compiler with itself exposed real bugs in stage 0, which are now fixed in both stages:
* a method named `init` collided with the generated constructor (constructors are now `new_C` / `init_C`);
* a lambda parameter `_` generated invalid Rust;
* the in-place `xs = xs.assoc(xs.count() - 1, v)` fast path violated the borrow checker;
* the checker needed Kotlin's flow-sensitive smart casts (`x == nil || x.f`, `if x == nil { return }`) to accept idiomatic null-safe code.

Language features added for self-hosting: `import "file.chim"`, compile-time `embedFile(...)` (the compiler embeds its runtime), and `exec` / `mkdirs` / `readFile` / `writeFile` / `args` / `exit`. The runtime also gained a 1 GiB main stack and copy-on-write fast paths.

## Milestone 2: full parity

* Port the gradual **type checker** (records and utility types, units of measure, null safety, ordinal arithmetic) into `selfhost/sema.chim`. Today these checks run only in stage 0, so stage 1 accepts some ill-typed programs that stage 0 rejects.
* Port the **JavaScript backend** (`src/gen_js.rs`).
* Then make stage 1 the default compiler and keep stage 0 only for bootstrapping.
* Performance: lower data classes and enums to Rust `struct`s/`enum`s, so the self-hosted compiler stops going through `rt::Value`.
