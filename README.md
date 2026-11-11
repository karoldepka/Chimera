# Chimera

A programming language that combines what's best in **Rust, Kotlin, Eiffel, CLOS, Clojure, TypeScript and Google Sheets**. The compiler is written in Rust and compiles Chimera to **Rust** (the primary target) or **JavaScript**.

```kotlin
data class Point(val x: Decimal, val y: Decimal)

struct User { id: Int, name: String, email: String?, salary: Quantity<USD> }
type UserPatch = Partial<Omit<User, "id">>          // TypeScript utility types

fun fib(n: Int): Int                                 // lowered to native Rust `fn fib(n: i64) -> i64`
  require n >= 0                                     // Eiffel contracts
= if n < 2 { n } else { fib(n - 1) + fib(n - 2) }

generic collide(a, b)                                // CLOS multiple dispatch
method collide(a: Asteroid, b: Ship) = "${b.name} is hit!"

fun main() {
  val budget = sheet {                               // reactive Google Sheets
    B1 = 1200 USD; B2 = 450.75 USD; B3 = 89.99 USD
    B4 = =SUM(B1:B3)
    C4 = IF(B4 > 1500 USD, "over budget", "ok")
  }
  println(0.1 + 0.2 == 0.3)                          // true: 0.1 is an exact Decimal
  println(2.5 kW * 6 h)                              // 15.0 kW·h: units of measure
  val names = users -> filter(|u| u.email != nil) -> map(|u| u.name)   // Clojure threading
}
```

## What comes from where

| From | Feature |
|---|---|
| **Rust** | algebraic `enum`s, `match` with guards and **exhaustiveness checking**, `Result<T, E>` with the `?` operator, immutable by default, expression-oriented blocks |
| **Kotlin** | `val`/`var`, primary constructors (`class P(val x: Int)`), `data class` with `copy(x = …)` and structural equality, null safety (`T?`, `?.`, `?:`, `!!`, smart casts), string templates `"$x ${y}"`, named and default arguments, mandatory `override`, `return` as an expression |
| **Eiffel** | full design by contract: `require`, `ensure` with `old(…)` and `result`, class `invariant`s; **contract inheritance** (overrides and trait implementations are held to their ancestors' contracts; `require else` weakens, `ensure then` strengthens); loop `invariant` / `variant`; `check`; monitoring levels (`--contracts all\|pre\|none`) |
| **CLOS** | multiple inheritance with C3 linearization, generic functions with **multiple dispatch**, `before`/`after`/`around` methods, `callNext()` |
| **Clojure** | persistent immutable vectors and maps, records as maps (`{...user, name: "x"}`), threading macros `->` / `->>`, atoms (`atom`, `swap`, `@a`) |
| **TypeScript** | structural record types, literal and union types, `Partial`, `Required`, `Readonly`, `Pick`, `Omit`, `Record`, `NonNullable`, `Exclude`, `Extract`, `ReturnType`, `keyof`, excess-property checks |
| **Google Sheets** | `sheet { }` blocks with cells, ranges (`B1:B3`) and reactive formulas; 50+ functions (`SUM`, `IF`, `IFS`, `VLOOKUP`, `SUMIF`, `TEXTJOIN`, `LET`, `LAMBDA`, `MAP`, `REDUCE`…); `&` concatenation, `<>`, `^`, `15%`, `{1, 2; 3, 4}` array literals |
| *new* | **ordinals as an affine unit**: `1st`, `2nd`, `3rd` are positions and plain numbers are counts. `3rd + 2 == 5th`, `10th - 3rd == 7`, and `1st + 3rd` is a compile error |
| *new* | **exact decimals by default** (`1.0999` is a `Decimal`; `1.5f` is a `Float`) and **units of measure** (`100 USD`, `2.5 kW`) with conversion (`1 kW + 500 W`) and compile-time unit checks |
| **Traits** | `trait`s with abstract and default methods, mixed freely with classes |

## Getting started

```bash
cargo build --release
./target/release/chimera run examples/sheets.chim              # compile to Rust, build with cargo, run
./target/release/chimera run examples/oop.chim --target js     # or compile to JavaScript and run with node
./target/release/chimera build examples/data.chim -o out/data  # emit a Cargo crate in out/data
./target/release/chimera emit examples/native.chim             # print the generated Rust
./target/release/chimera check tests/errors/records.chim       # type-check only
```

You need Rust (cargo, rustc; rustfmt is optional) and, for the JS target, Node 18 or newer.

## Examples

| File | Shows |
|---|---|
| `examples/basics.chim` | enums, `match`, contracts, decimals vs floats, units, threading, Sheets functions |
| `examples/oop.chim` | traits, multiple inheritance, diamonds, `super` and `super<T>`, data classes, invariants |
| `examples/dispatch.chim` | multimethods on all arguments, before/after/around, dispatch on `Int`/`Decimal`/`Float`/`Quantity` |
| `examples/data.chim` | records and utility types, null safety, `Result` and `?`, persistent collections, atoms |
| `examples/sheets.chim` | reactive sheets, Sheets formulas, exact decimals, units of measure |
| `examples/native.chim` | functions lowered to native Rust (`i64`/`f64`/`bool`) |
| `examples/dbc.chim` | design by contract: inherited contracts, `require else` / `ensure then`, loop invariants and variants, `check` |
| `examples/ordinals.chim` | ordinal vs cardinal numbers |
| `examples/tour.chim` | everything together: a small energy-billing system |
| `selfhost/*.chim` | the Chimera compiler, in Chimera |
| `tests/errors/*.chim` | what the checker rejects: unit mismatches, null safety, excess properties, missing overrides, diamond conflicts, non-exhaustive matches… |

## Rust backend

`chimera build` writes a normal Cargo crate (`Cargo.toml`, `src/main.rs`, `src/rt.rs`), formats it with rustfmt, and builds it with `overflow-checks = true`.

* **Native lowering:** a function whose signature uses only `Int`/`Float`/`Bool`, and whose body stays in the arithmetic/loop/call subset, compiles to a plain Rust function, with contracts kept as runtime checks:
  ```rust
  /// native: `fib` compiled to plain Rust (i64 -> i64)
  fn fib(mut n: i64) -> i64 {
      if !(n >= 0i64) { rt::fail("precondition violated in fib: n >= 0"); }
      if n < 2i64 { n } else { fib(n - 1i64) + fib(n - 2i64) }
  }
  ```
* **Everything else** goes through `rt::Value`, a uniform value type (`Rc` based; persistent collections are copy-on-write through `Rc::make_mut`, so `xs = xs.push(v)` is amortised O(1) when `xs` is not shared).
* Classes, MROs, invariants and multimethods are registered in a small runtime table at startup.

## Self-hosting ✅

The compiler compiles itself. `selfhost/` contains the full compiler written in Chimera (about 3,600 lines): lexer, parser, desugaring, semantic analysis, native lowering, the Rust backend, and a driver that writes and builds Cargo crates.

```
stage 0  src/*.rs (Rust)        ──compiles──▶  stage 1  (selfhost/*.chim → Rust → native binary)
stage 1                         ──compiles──▶  stage 2
stage 2 emits exactly what stage 1 emits  ⇒  fixpoint ✓
```

`tests/bootstrap.sh` verifies all of this (52 checks):
* the fixpoint: stage 0, stage 1 and stage 2 emit the same 458 KB of Rust for the compiler itself;
* every example: all stages emit byte-identical Rust, and the stage-2 binary runs it with the golden output;
* the self-hosted lexer produces the same token stream as stage 0 on every `.chim` file;
* the ported checks give exactly the same diagnostics as stage 0: C3/override/abstract/diamond rules, name resolution, arity, `val`/capture rules, contract scoping, exhaustiveness.

Stage 1 compiles its own source in about 0.7 s. Not ported yet: the gradual *type* checks (records and utility types, units, null safety, ordinal arithmetic) and the JS backend. Those remain stage-0 features, and porting them is the next milestone (see [SELFHOSTING.md](SELFHOSTING.md)).

```bash
./target/release/chimera build selfhost/main.chim -o build/stage1     # stage 0 → stage 1
build/stage1/target/release/main run examples/tour.chim               # use the self-hosted compiler
```

## Tests

```bash
tests/run.sh        # every example on both backends vs. golden output, plus expected diagnostics
tests/bootstrap.sh  # self-hosting: stage0 → stage1 → stage2 fixpoint, identical output everywhere
```

## Layout

```
src/lexer.rs  parser.rs  desugar.rs  sema.rs  units.rs   front end
src/gen_rust.rs  native.rs  gen_js.rs  common.rs         back ends
src/main.rs                                              CLI
runtime/rt.rs  runtime/rt.js                             runtimes (kept semantically identical)
selfhost/main.chim  ast  lexer  parser  desugar  sema  common  native  gen_rust   the compiler, in Chimera
SPEC.md                                                  language reference
```

## Status

This is a v0.1 prototype. The checker is gradual: types it can't infer become `Any` and aren't reported. What it does know (records, nullability, units, class members, arity, exhaustiveness) is checked strictly. See SPEC.md for limitations.

MIT licensed.
