# Chimera language reference (v0.1)

## 1. Lexical structure

* Comments: `// line` and `/* block */`.
* Newlines end statements; `;` is optional. A binary operator must sit on the same line as its left operand. The chain operators `->`, `->>`, `?:`, `&&`, `||` and `.` may start a continuation line.
* Literals:

| Literal | Type |
|---|---|
| `42`, `1_000` | `Int` (64-bit, overflow is an error) |
| `1.0999`, `0.1` | `Decimal`, exact base-10 (`0.1 + 0.2 == 0.3`) |
| `1.5f`, `2e10`, `3f` | `Float` (IEEE-754 binary64) |
| `15%` | `Decimal` `0.15` (Sheets style) |
| `100 USD`, `2.5 kW` | `Quantity`: a number followed by a unit name on the same line |
| `"text $name ${expr}"` | `String` with templates; escapes `\n \t \r \" \\ \$ \0` |
| `true`, `false`, `nil` | `Bool`, `Nil` |
| `[1, 2]` | persistent vector |
| `{1, 2, 3}`, `{1; 2}`, `{1, 2; 3, 4}` | Sheets array literals: row, column, 2-D |
| `{name: "Ada", "k": 1, ...other}` | persistent map / record, with spread |

## 2. Declarations

```kotlin
fun name(a: Int, b: Int = 2): Int  require a > 0  ensure result > a  { ... }   // or `= expr`
class Name(val x: Int, var y: Int, plain: Int) : Parent(plain), Trait { ... }
data class P(val x: Decimal)                  // structural ==, toString, copy(x = …)
abstract class A    trait T : OtherTrait { abstract fun f(): Int; fun g() = f() + 1 }
enum Shape { Circle(r: Float), Rect(w: Decimal, h: Decimal), Empty }
struct User { id: Int, email?: String, readonly name: String }
type Alias<T> = { value: T, next: Alias<T>? }
generic collide(a, b)
method collide(a: Ship, b: Asteroid) = …     // also: before / after / around
```

Every program has a `fun main()`.

### Classes

* Constructor parameters marked `val`/`var` become fields; body fields are `val x = expr` / `var x = expr`.
* Multiple inheritance: any number of classes and traits. The method resolution order is the **C3 linearization**.
* `override` is required when redefining an inherited member, and rejected when nothing is overridden.
* If two unrelated parents provide the same method, the class must override it. Inside the override, `super<Parent>.m()` picks one implementation. `super.m()` calls the next class in the receiver's MRO (cooperative, as in Python/CLOS).
* In a diamond, each ancestor's initializer runs once: the first one in MRO order wins.
* Concrete classes must implement every abstract member. Abstract classes and traits can't be instantiated.

### Design by Contract (Eiffel)

* `require` is checked on entry. `ensure` is checked on exit, and may use `result` and `old(expr)` (the value of `expr` on entry).
* Clauses may be tagged: `require positive: amount > 0`. Several clauses can be separated by commas.
* `invariant`s are checked after construction and after every *qualified* call (`obj.m()`), but not after calls inside the class (`m()` / `this.m()`). Invariants of all ancestors apply.
* **Contract inheritance.** A method that overrides or implements an inherited method is held to the contracts of every declaration of that method in its class's MRO, including contracts on abstract trait methods:
  * the effective precondition is the method's own `require else` clauses **or** any ancestor's precondition (an original declaration without `require` accepts everything);
  * the effective postcondition is the method's own `ensure then` clauses **and** every ancestor's postcondition;
  * in an override, a plain `require` or `ensure` is a compile error, and `require else` / `ensure then` outside an override is too;
  * inherited contracts are evaluated with the override's parameter names, and violations name their origin: `… violated in LossyStack.push (inherited from Stack.push)`.
* **Loops:** `while cond invariant a, b variant e { … }`. The invariants are checked before every evaluation of the condition, so also on exit. The variant must be an `Int` that stays non-negative and strictly decreases on every iteration, which proves termination. `for` loops accept `invariant` only.
* **`check a, b`** asserts conditions in the middle of a body.
* **Monitoring levels:** `--contracts all` (default), `pre` (only preconditions), `none`. Contracts below the level are removed at compile time.
* A violation stops the program with a message that names the clause.

### Generic functions (CLOS)

A `method` specializes each parameter on a class, trait, enum, variant, or one of `Int`, `Float`, `Decimal`, `Number`, `Quantity`, `String`, `Bool`, `Nil`, `Vec`, `Map`, `Fn`, `Sheet`, `Atom`, `Any`. Dispatch looks at **all** arguments.

Applicable methods are ordered most-specific-first, comparing arguments left to right. They run as: `around` methods, then all `before` methods, then the most specific primary method, then the `after` methods (least specific first). `callNext()` (or `callNext(args…)`) invokes the next method.

## 3. Types

* Primitives: `Int`, `Float`, `Decimal`, `String`, `Bool`, `Unit`, `Any`, `Number`.
* Others: `Quantity<USD>`, `Vec<T>`, `Map<K, V>`, `(A) -> B`, `T?`, `"lit"`, `A | B`, `{ a: T, b?: U, readonly c: V }`, `keyof T`, `Result<T, E>`, `Sheet`, `Atom`.
* **Utility types:** `Partial`, `Required`, `Readonly`, `Mutable`, `Pick<T, K>`, `Omit<T, K>`, `Record<K, V>`, `NonNullable`, `Exclude`, `Extract`, `ReturnType`. Picking or omitting a key that doesn't exist is an error.
* **Null safety:** `T` never holds `nil`. On a `T?` you must use `?.`, `?:`, `!!`, or a check: `if x != nil { … }` smart-casts `x` inside the branch.
* **Records** are immutable maps. A record literal checked against an expected record type reports missing required fields, unknown fields, and wrong field types. `r.f = v` on a `var` record rebinds it to an updated copy. `Readonly` fields reject this.
* **Numbers:** `Int op Int → Int` (with truncating `/`), `Int/Decimal → Decimal`, anything with `Float → Float`. Decimal division keeps 16 extra digits. An `Int` converts implicitly to `Decimal` or `Float`; a `Float` never converts implicitly to `Decimal`.
* **Units:** `+`, `-`, `%` and comparisons need compatible units. Simple units with the same base convert automatically: `1 kW + 500 W == 1.5 kW`. `*` and `/` combine units (`kW·h`, `USD/h`). When units cancel, the result is a plain number. Mixing a plain number into `+`/`-` with a quantity is an error, both at compile time (when the types are known) and at run time. Built-in convertible families: W/kW/MW/GW, Wh/kWh/MWh, mm/cm/m/km, mg/g/kg/t, ms/s/min/h/d, mL/L, B/KB/MB/GB. Any other identifier (USD, EUR, PLN…) is an atomic unit. Use `q.amount`, `q.unit`, `q.to("W")`, `n.withUnit("USD")`, or the `UNIT`/`CONVERT` functions.

## 4. Expressions

* Operators by precedence, lowest first: `->` `->>` · `||` · `&&` · `==` `!=` `<>` · `<` `<=` `>` `>=` `is` · `&` · `?:` · `..` · `+` `-` · `*` `/` `%` · `^` · unary `-` `!` `@` · postfix `()` `[]` `.` `?.` `!!` `?` `%`.
* `if c { } else { }`, `match`, and blocks are expressions. `return`, `break` and `continue` are expressions too (`val x = y ?: return nil`).
* `match x { Circle(r) if r > 1 => …, Rect(w, _) => …, 0 => …, n => … }`. A match on an enum must cover every variant or have a catch-all arm.
* Lambdas: `|x, y| x + y`, `|| { … }`, or Sheets `LAMBDA(x, x * 2)`. Lambdas may not reassign captured variables; use an `atom`.
* Threading: `x -> f(a)` is `f(x, a)`; `x ->> f(a)` is `f(a, x)`. If `f` is not a function in scope, `x -> f(a)` becomes the method call `x.f(a)`.
* `expr?`: `Ok(v)` gives `v`, while `Err(e)` (or `nil`) returns from the enclosing function.
* Atoms: `val a = atom(0)`, `a.swap(|n| n + 1)`, `a.reset(5)`, `@a`.

## 5. Google Sheets

```
val s = sheet {
  A1 = "Rent";  B1 = 1200 USD
  B4 = =SUM(B1:B3)          // the leading `=` is optional
}
s.B1 = 1300 USD             // inputs can be edited; formulas recompute when read
println(s.B4)  s["B4"]  s.get("B4")  s.cells()
```

* A cell whose expression references cells or calls a function is a **formula**: a closure re-evaluated on every read, with cycle detection (`#REF!`).
* Ranges: a single column or row gives a vector; a block gives a vector of rows.
* Functions:
  * lazy: `IF IFS AND OR NOT SWITCH IFERROR LET LAMBDA`
  * eager: `SUM AVERAGE MIN MAX COUNT COUNTA PRODUCT MEDIAN ROUND ROUNDUP ROUNDDOWN INT ABS SQRT POWER MOD LEN UPPER LOWER TRIM LEFT RIGHT MID CONCATENATE TEXTJOIN SPLIT SUBSTITUTE SUMIF COUNTIF AVERAGEIF FILTER SORT UNIQUE SEQUENCE VLOOKUP INDEX MATCH ISBLANK ISNUMBER ISTEXT ISERROR MAP REDUCE UNIT CONVERT`
* Sheets errors are `Err("#N/A")`, `Err("#DIV/0!")`, and so on; `IFERROR` and `ISERROR` handle them.
* All of these functions work anywhere, not only inside sheets.

## 6. Modules, ordinals, smart casts

* `import "other.chim"` at the top level includes another file once, with its path relative to the importing file. Diagnostics report the original file and line.
* `embedFile("path")` inlines a file's contents as a string at compile time. The path is relative to the main source file.
* **Ordinals:** `1st`, `2nd`, `3rd`, `4th` (no space; the suffix must be correct, so `21th` is an error) or `n ord` have the unit `ord`. Position ± `Int` = position; position − position = `Int`. Position + position, `*`, `/` and non-integer offsets are errors at compile time and at run time. Use `p.index` (zero-based), `p.cardinal`, and `n.ordinal()`.
* **Smart casts** (Kotlin): `x` is non-null inside `if x != nil { … }`, on the right of `x != nil && …` and `x == nil || …`, and after `if x == nil { return / break / continue }`.

## 7. Builtins

`println print eprintln str atom range panic assert readFile writeFile args exit exec mkdirs embedFile Ok Err`, plus methods on strings, numbers, vectors, maps, atoms, `Result` and sheets (see `runtime/rt.rs`, `builtin_method`).

## 8. Known limitations (v0.1)

* The checker is gradual: unannotated values are `Any`. Generic type parameters are accepted but not checked.
* Named arguments are checked and reordered only for functions, constructors and `copy`; calls to methods are positional.
* Rust backend: only native-lowerable functions avoid `rt::Value`. Classes are dynamic objects, not Rust structs.
* JS backend: `Int` is a BigInt (with i64 overflow checks), so it is slower than JS numbers.
* `import` is textual inclusion; there are no namespaces.
