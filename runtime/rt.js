// Chimera runtime for the JavaScript backend. Mirrors runtime/rt.rs exactly.
"use strict";
const $rt = (() => {
  // ============================================================ values
  // Nil = null, Bool = boolean, Int = bigint (i64-checked), Float = number, String = string,
  // Vec = Array (never mutated), the rest are classes below.
  class ChimeraError extends Error {}
  class Dec { constructor(m, s) { this.m = m; this.s = s; } }
  class Qty { constructor(amt, unit) { this.amt = amt; this.unit = unit; } }
  class CMap { constructor(entries) { this.entries = entries; } }
  class CObj { constructor(cls) { this.cls = cls; this.fields = new Map(); this.inited = []; } }
  class Variant { constructor(en, name, names, vals) { this.en = en; this.name = name; this.names = names; this.vals = vals; } }
  class Atom { constructor(v) { this.v = v; } }
  class Sheet { constructor(cells) { this.cells = cells; this.evaluating = []; } }
  class Formula { constructor(f) { this.f = f; } }

  const fail = (msg) => { throw new ChimeraError(msg); };
  const I64_MIN = -(2n ** 63n), I64_MAX = 2n ** 63n - 1n;
  const I128_MAX = 2n ** 127n - 1n;
  const chkI = (r, what) => (r < I64_MIN || r > I64_MAX) ? fail(`integer overflow in ${what}`) : r;
  const chk128 = (r) => (r > I128_MAX || r < -I128_MAX - 1n) ? fail("decimal overflow") : r;
  const isInt = (v) => typeof v === "bigint";
  const isFloat = (v) => typeof v === "number";
  const isNum = (v) => isInt(v) || isFloat(v) || v instanceof Dec;
  const isNil = (v) => v === null || v === undefined;
  const abs = (x) => (x < 0n ? -x : x);

  const variant = (en, name, names, vals) => new Variant(en, name, names, vals);
  const ok = (v) => variant("Result", "Ok", ["value"], [v]);
  const err = (v) => variant("Result", "Err", ["error"], [v]);

  // ============================================================ decimals
  const P10 = (n) => 10n ** BigInt(n);
  const DEC_MAX_SCALE = 18;
  function decParse(t) {
    const neg = t.startsWith("-");
    t = t.replace(/^-+/, "");
    const [ip, fp = ""] = t.split(".");
    if (!/^\d+$/.test(ip + fp)) fail(`invalid decimal ${t}`);
    const m = BigInt(ip + fp);
    return new Dec(neg ? -m : m, fp.length);
  }
  const decFromI = (i) => new Dec(i, 0);
  const decFromF = (f) => decParse(showRustFloat(f));
  const up = (d, s) => s < d.s ? d.m / P10(d.s - s) : chk128(d.m * P10(s - d.s));
  const decToF = (d) => Number(d.m) / 10 ** d.s;
  function norm(d, min) { let { m, s } = d; while (s > min && m % 10n === 0n) { m /= 10n; s--; } return new Dec(m, s); }
  const decAdd = (a, b) => { const s = Math.max(a.s, b.s); return new Dec(chk128(up(a, s) + up(b, s)), s); };
  const decNeg = (a) => new Dec(-a.m, a.s);
  const decSub = (a, b) => decAdd(a, decNeg(b));
  function decMul(a, b) { const r = new Dec(chk128(a.m * b.m), a.s + b.s); return r.s > DEC_MAX_SCALE ? decRound(r, DEC_MAX_SCALE, 0) : r; }
  function decDiv(a, b) {
    if (b.m === 0n) fail("division by zero");
    const base = Math.max(a.s, b.s);
    for (let extra = 16; extra >= 0; extra--) {
      const t = base + extra;
      const k = P10(t + b.s - a.s);
      if (k > I128_MAX) continue;
      const num = a.m * k;
      if (num > I128_MAX || num < -I128_MAX - 1n) continue;
      let q = num / b.m;
      const r = num % b.m;
      if (r !== 0n && abs(r) * 2n >= abs(b.m)) q += ((num < 0n) === (b.m < 0n)) ? 1n : -1n;
      return norm(new Dec(q, t), base);
    }
    fail("decimal overflow");
  }
  const decRem = (a, b) => { if (b.m === 0n) fail("division by zero"); const s = Math.max(a.s, b.s); return new Dec(up(a, s) % up(b, s), s); };
  const decCmp = (a, b) => { const s = Math.max(a.s, b.s); const x = up(a, s), y = up(b, s); return x < y ? -1 : x > y ? 1 : 0; };
  function decRound(d, digits, mode) {
    digits = Math.max(0, Number(digits));
    if (digits >= d.s) return new Dec(up(d, digits), digits);
    const k = P10(d.s - digits);
    const q = d.m / k, r = d.m % k;
    const bump = mode === 0 ? abs(r) * 2n >= k : mode === 1 ? r !== 0n : false;
    const sign = d.m > 0n ? 1n : d.m < 0n ? -1n : 0n;
    return new Dec(bump ? q + sign : q, digits);
  }
  function decShow(d) {
    const neg = d.m < 0n, a = abs(d.m), k = P10(d.s);
    let o = (neg ? "-" : "") + (a / k).toString();
    if (d.s > 0) o += "." + (a % k).toString().padStart(d.s, "0");
    return o;
  }
  function toDec(v) {
    if (isInt(v)) return decFromI(v);
    if (v instanceof Dec) return v;
    if (isFloat(v)) return decFromF(v);
    fail(`expected a number but found ${typeName(v)}`);
  }
  const dec = (t) => decParse(t);

  // ============================================================ units
  const CONVERSIONS = [
    ["W", "W", "1"], ["kW", "W", "1000"], ["MW", "W", "1000000"], ["GW", "W", "1000000000"],
    ["Wh", "Wh", "1"], ["kWh", "Wh", "1000"], ["MWh", "Wh", "1000000"],
    ["mm", "m", "0.001"], ["cm", "m", "0.01"], ["m", "m", "1"], ["km", "m", "1000"],
    ["mg", "g", "0.001"], ["g", "g", "1"], ["kg", "g", "1000"], ["t", "g", "1000000"],
    ["ms", "s", "0.001"], ["s", "s", "1"], ["min", "s", "60"], ["h", "s", "3600"], ["d", "s", "86400"],
    ["mL", "L", "0.001"], ["L", "L", "1"],
    ["B", "B", "1"], ["KB", "B", "1000"], ["MB", "B", "1000000"], ["GB", "B", "1000000000"],
  ];
  function qty(amt, unit) {
    if (!isNum(amt)) fail(`unit ${unit} needs a number, got ${typeName(amt)}`);
    return new Qty(amt, [[unit, 1]]);
  }
  const mkQty = (amt, unit) => unit.length === 0 ? amt : new Qty(amt, unit);
  const unitSimple = (u) => (u.length === 1 && u[0][1] === 1) ? u[0][0] : null;
  const unitKey = (u) => u.map(([n, e]) => `${n}^${e}`).sort().join(",");
  const unitSame = (a, b) => unitKey(a) === unitKey(b);
  const conv = (u) => { const c = CONVERSIONS.find((c) => c[0] === u); return c ? [c[1], decParse(c[2])] : null; };
  function factor(from, to) {
    if (unitSame(from, to)) return decFromI(1n);
    const a = unitSimple(from), b = unitSimple(to);
    if (a === null || b === null) return null;
    const ca = conv(a), cb = conv(b);
    if (!ca || !cb || ca[0] !== cb[0]) return null;
    return decDiv(ca[1], cb[1]);
  }
  function convertAmt(amt, f) {
    if (f.m === 1n && f.s === 0) return amt;
    if (isFloat(amt)) return amt * decToF(f);
    return norm(decMul(toDec(amt), f), 0);
  }
  function unitShow(u) {
    const num = u.filter(([, e]) => e > 0).map(([n, e]) => e === 1 ? n : `${n}^${e}`);
    const den = u.filter(([, e]) => e < 0).map(([n, e]) => e === -1 ? n : `${n}^${-e}`);
    const n = num.length ? num.join("·") : "1";
    return den.length ? `${n}/${den.join("/")}` : n;
  }
  function unitCombine(a, b, sign) {
    const out = a.map(([n, e]) => [n, e]);
    for (const [n, e] of b) { const slot = out.find((x) => x[0] === n); if (slot) slot[1] += e * sign; else out.push([n, e * sign]); }
    return out.filter(([, e]) => e !== 0);
  }
  function qtyAlign(a, b, op) {
    const f = factor(b.unit, a.unit);
    if (!f) fail(`cannot ${op} ${unitShow(a.unit)} and ${unitShow(b.unit)}: incompatible units`);
    return convertAmt(b.amt, f);
  }
  function convertTo(v, unit) {
    if (!(v instanceof Qty)) fail(`${typeName(v)} has no unit to convert`);
    const target = [[unit, 1]];
    const f = factor(v.unit, target);
    if (!f) fail(`cannot convert ${unitShow(v.unit)} to ${unit}`);
    return mkQty(convertAmt(v.amt, f), target);
  }

  // ============================================================ classes & dispatch
  const CLASSES = new Map();
  const GENERICS = new Map();
  const registerClass = (c) => CLASSES.set(c.name, c);
  const registerMethod = (gf, m) => { if (!GENERICS.has(gf)) GENERICS.set(gf, []); GENERICS.get(gf).push(m); };
  const cls = (n) => CLASSES.get(n) ?? fail(`unknown class ${n}`);
  const newObj = (c) => new CObj(c);
  function beginInit(o, part) { if (o.inited.includes(part)) return false; o.inited.push(part); return true; }
  const finishNew = (o) => { checkInvariants(o); return o; };
  function findMethod(c, name, after) {
    let skipping = after !== undefined;
    for (const k of cls(c).mro) {
      if (skipping) { if (k === after) skipping = false; continue; }
      const m = cls(k).methods.find((x) => x[0] === name);
      if (m) return m[1];
    }
    return null;
  }
  function checkInvariants(o) {
    if (!(o instanceof CObj)) return;
    const ci = cls(o.cls);
    if (!ci.hasInvariants) return;
    for (const k of ci.mro) {
      const inv = cls(k).invariant;
      if (inv) { const msg = inv(o); if (msg !== null) fail(`invariant violation in ${o.cls} (class ${k}): ${msg}`); }
    }
  }
  function call(recv, name, args) {
    if (recv instanceof CObj) {
      const f = findMethod(recv.cls, name);
      if (f) { const r = f(recv, args); checkInvariants(recv); return r; }
      const fv = recv.fields.get(name);
      if (typeof fv === "function") return apply(fv, args);
    }
    return builtinMethod(recv, name, args);
  }
  const callSafe = (recv, name, args) => isNil(recv) ? null : call(recv, name, args);
  function callSelf(t, name, args) { const f = findMethod(t.cls, name); return f ? f(t, args) : builtinMethod(t, name, args); }
  function callSuper(t, from, name, args) { const f = findMethod(t.cls, name, from); return f ? f(t, args) : fail(`no super method ${name} after ${from}`); }
  function callFrom(t, q, name, args) { const f = findMethod(q, name); return f ? f(t, args) : fail(`${q} has no method ${name}`); }
  const apply = (f, args) => typeof f === "function" ? f(...args) : fail(`${typeName(f)} is not a function`);
  function cpl(v) {
    let out;
    if (isNil(v)) out = ["Nil"];
    else if (typeof v === "boolean") out = ["Bool"];
    else if (isInt(v)) out = ["Int", "Number"];
    else if (isFloat(v)) out = ["Float", "Number"];
    else if (v instanceof Dec) out = ["Decimal", "Number"];
    else if (v instanceof Qty) out = ["Quantity"];
    else if (typeof v === "string") out = ["String"];
    else if (Array.isArray(v)) out = ["Vec"];
    else if (v instanceof CMap) out = ["Map"];
    else if (typeof v === "function") out = ["Fn"];
    else if (v instanceof Atom) out = ["Atom"];
    else if (v instanceof Sheet) out = ["Sheet"];
    else if (v instanceof CObj) out = [...cls(v.cls).mro];
    else if (v instanceof Variant) out = [v.name, v.en];
    else out = [];
    out.push("Any");
    return out;
  }
  const isInstance = (v, t) => cpl(v).includes(t);
  const isVariant = (v, n) => v instanceof Variant && v.name === n;
  const vfield = (v, i) => (v instanceof Variant ? (v.vals[i] ?? null) : null);
  function cmpKeys(a, b) { for (let i = 0; i < Math.min(a.length, b.length); i++) if (a[i] !== b[i]) return a[i] - b[i]; return a.length - b.length; }
  function callGeneric(gf, args) {
    const methods = GENERICS.get(gf) ?? [];
    const cpls = args.map(cpl);
    const app = [];
    for (const m of methods) {
      const key = [];
      let okm = true;
      for (let i = 0; i < m.specs.length; i++) { const p = cpls[i].indexOf(m.specs[i]); if (p < 0) { okm = false; break; } key.push(p); }
      if (okm) app.push([key, m]);
    }
    app.sort((a, b) => cmpKeys(a[0], b[0]));
    const pick = (q) => app.filter(([, m]) => m.qual === q).map(([, m]) => m.f);
    const prim = pick(0), before = pick(1), after = pick(2).reverse(), around = pick(3);
    if (prim.length === 0) fail(`no applicable method for ${gf}(${args.map(typeName).join(", ")})`);
    const core = (a) => {
      for (const b of before) b(a, () => fail("callNext() is not allowed in a before method"));
      const r = runChain(prim, 0, a, gf);
      for (const f of after) f(a, () => fail("callNext() is not allowed in an after method"));
      return r;
    };
    return runAround(around, 0, args, core);
  }
  function runChain(ms, i, args, gf) { if (i >= ms.length) fail(`no next method for ${gf}`); return ms[i](args, (a) => runChain(ms, i + 1, a, gf)); }
  function runAround(ms, i, args, core) { if (i >= ms.length) return core(args); return ms[i](args, (a) => runAround(ms, i + 1, a, core)); }

  // ============================================================ fields, maps, records
  function set(t, name, val) { if (t instanceof CObj) { t.fields.set(name, val); return; } fail(`cannot set field ${name} on ${typeName(t)}`); }
  function setIn(base, name, val) {
    if (base instanceof CObj) { set(base, name, val); return base; }
    if (base instanceof CMap) return assoc(base, name, val);
    if (base instanceof Sheet) { setCell(base, name, val, false); return base; }
    fail(`cannot set field ${name} on ${typeName(base)}`);
  }
  function get(v, name) {
    if (v instanceof CObj) {
      if (v.fields.has(name)) return v.fields.get(name);
      if (findMethod(v.cls, name)) return (...a) => call(v, name, a);
      fail(`${v.cls} has no field ${name}`);
    }
    if (v instanceof CMap) { const e = v.entries.find(([k]) => k === name); return e ? e[1] : null; }
    if (v instanceof Variant) { const i = v.names.indexOf(name); return i >= 0 ? v.vals[i] : builtinMethod(v, name, []); }
    if (v instanceof Sheet) return cell(v, name);
    if (isNil(v)) fail(`cannot read field ${name} of nil`);
    return builtinMethod(v, name, []);
  }
  const getSafe = (v, name) => isNil(v) ? null : get(v, name);
  function map(entries) { let out = new CMap([]); for (const [k, v] of entries) out = assoc(out, k, v); return out; }
  function merge(a, b) {
    if (b instanceof CMap) { let out = a; for (const [k, v] of b.entries) out = assoc(out, k, v); return out; }
    if (b instanceof CObj) { let out = a; for (const [k, v] of b.fields) out = assoc(out, k, v); return out; }
    if (isNil(b)) return a;
    fail(`cannot spread ${typeName(b)} into a map`);
  }
  function assoc(m, k, v) {
    if (m instanceof CMap) {
      const e = m.entries.map((p) => [p[0], p[1]]);
      const slot = e.find((p) => eq(p[0], k));
      if (slot) slot[1] = v; else e.push([k, v]);
      return new CMap(e);
    }
    if (Array.isArray(m)) {
      const i = Number(asInt(k));
      const e = [...m];
      if (i >= 0 && i < e.length) e[i] = v; else if (i === e.length) e.push(v); else fail(`index ${i} out of bounds for length ${e.length}`);
      return e;
    }
    fail(`cannot assoc on ${typeName(m)}`);
  }
  function index(c, k) {
    if (Array.isArray(c)) { const i = asInt(k); return i < 0n ? null : (c[Number(i)] ?? null); }
    if (c instanceof CMap) { const e = c.entries.find((p) => eq(p[0], k)); return e ? e[1] : null; }
    if (typeof c === "string") { const ch = Array.from(c)[Number(asInt(k) < 0n ? 0n : asInt(k))]; return ch ?? null; }
    if (c instanceof Sheet) return cell(c, toStr(k));
    if (c instanceof CObj) return get(c, toStr(k));
    fail(`cannot index ${typeName(c)}`);
  }
  function copy(o, changes) {
    if (!(o instanceof CObj)) fail("copy() needs an object");
    const n = new CObj(o.cls);
    n.fields = new Map(o.fields);
    n.inited = [...o.inited];
    for (const [k, v] of changes) { if (!n.fields.has(k)) fail(`no field ${k}`); n.fields.set(k, v); }
    checkInvariants(n);
    return n;
  }
  const atom = (v) => new Atom(v);

  // ============================================================ sheets
  const sheet = (cells) => new Sheet(cells.map(([n, c]) => [n, c]));
  const formula = (f) => new Formula(f);
  function setCell(sh, name, v) { const slot = sh.cells.find((c) => c[0] === name); if (slot) slot[1] = v; else sh.cells.push([name, v]); }
  function cell(sh, name) {
    if (!(sh instanceof Sheet)) fail("not a sheet");
    const c = sh.cells.find((x) => x[0] === name);
    if (!c) return null;
    if (!(c[1] instanceof Formula)) return c[1];
    if (sh.evaluating.includes(name)) fail(`#REF! circular dependency at cell ${name}`);
    sh.evaluating.push(name);
    try { return c[1].f(sh); } finally { sh.evaluating.pop(); }
  }
  function splitCell(c) {
    const letters = c.match(/^[A-Z]+/)[0];
    let col = 0;
    for (const ch of letters) col = col * 26 + (ch.charCodeAt(0) - 64);
    return [col, parseInt(c.slice(letters.length), 10)];
  }
  function colName(col) { let s = ""; while (col > 0) { const r = (col - 1) % 26; s = String.fromCharCode(65 + r) + s; col = Math.floor((col - 1) / 26); } return s; }
  function cellRange(sh, from, to) {
    let [c1, r1] = splitCell(from), [c2, r2] = splitCell(to);
    [c1, c2] = [Math.min(c1, c2), Math.max(c1, c2)];
    [r1, r2] = [Math.min(r1, r2), Math.max(r1, r2)];
    const rows = [];
    for (let r = r1; r <= r2; r++) { const row = []; for (let c = c1; c <= c2; c++) row.push(cell(sh, `${colName(c)}${r}`)); rows.push(row); }
    if (c1 === c2) return rows.map((r) => r[0]);
    if (r1 === r2) return rows[0];
    return rows;
  }
  function sheetCellsSorted(sh) {
    return sh.cells.map((c) => c[0]).map((n) => [n, splitCell(n)]).sort((a, b) => (a[1][1] - b[1][1]) || (a[1][0] - b[1][0])).map((x) => x[0]);
  }

  // ============================================================ conversions & display
  function typeName(v) {
    if (isNil(v)) return "Nil";
    if (typeof v === "boolean") return "Bool";
    if (isInt(v)) return "Int";
    if (isFloat(v)) return "Float";
    if (v instanceof Dec) return "Decimal";
    if (v instanceof Qty) return `Quantity<${unitShow(v.unit)}>`;
    if (typeof v === "string") return "String";
    if (Array.isArray(v)) return "Vec";
    if (v instanceof CMap) return "Map";
    if (typeof v === "function") return "Fn";
    if (v instanceof Atom) return "Atom";
    if (v instanceof Sheet) return "Sheet";
    if (v instanceof CObj) return v.cls;
    if (v instanceof Variant) return v.en;
    return "?";
  }
  function truthy(v) { if (typeof v === "boolean") return v; if (isNil(v)) return false; fail(`expected Bool but found ${typeName(v)}`); }
  function asInt(v) {
    if (isInt(v)) return v;
    if (isFloat(v)) return BigInt(Math.trunc(v));
    if (v instanceof Dec) return decRound(v, 0, 2).m;
    fail(`expected Int but found ${typeName(v)}`);
  }
  function asF64(v) {
    if (isInt(v)) return Number(v);
    if (isFloat(v)) return v;
    if (v instanceof Dec) return decToF(v);
    if (typeof v === "boolean") return v ? 1 : 0;
    if (isNil(v)) return 0;
    fail(`expected a number but found ${typeName(v)}`);
  }
  // Rust's `{}` formatting of f64 (never uses exponent notation)
  function showRustFloat(f) {
    if (Number.isNaN(f)) return "NaN";
    if (!Number.isFinite(f)) return f > 0 ? "inf" : "-inf";
    const s = String(f);
    if (!/e/i.test(s)) return s;
    const [mant, ex] = s.split(/e/i);
    const e = parseInt(ex, 10);
    const neg = mant.startsWith("-");
    const digits = mant.replace(/^-/, "").replace(".", "");
    const point = mant.replace(/^-/, "").indexOf(".") < 0 ? mant.replace(/^-/, "").length : mant.replace(/^-/, "").indexOf(".");
    const pos = point + e;
    let out;
    if (pos <= 0) out = "0." + "0".repeat(-pos) + digits;
    else if (pos >= digits.length) out = digits + "0".repeat(pos - digits.length);
    else out = digits.slice(0, pos) + "." + digits.slice(pos);
    return (neg ? "-" : "") + out;
  }
  function fmtFloat(f) {
    if (Number.isNaN(f)) return "NaN";
    if (!Number.isFinite(f)) return f > 0 ? "Infinity" : "-Infinity";
    if (Number.isInteger(f) && Math.abs(f) < 1e15) return `${Math.trunc(f) === 0 ? 0 : Math.trunc(f)}.0`;
    return showRustFloat(f);
  }
  const quote = (s) => '"' + s.replace(/\\/g, "\\\\").replace(/"/g, '\\"').replace(/\n/g, "\\n") + '"';
  const repr = (v) => typeof v === "string" ? quote(v) : toStr(v);
  function toStr(v) {
    if (isNil(v)) return "nil";
    if (typeof v === "boolean") return String(v);
    if (isInt(v)) return v.toString();
    if (isFloat(v)) return fmtFloat(v);
    if (v instanceof Dec) return decShow(v);
    if (isOrd(v)) { const n = asInt(v.amt); return `${n}${ordinalSuffix(n)}`; }
    if (v instanceof Qty) return `${toStr(v.amt)} ${unitShow(v.unit)}`;
    if (typeof v === "string") return v;
    if (Array.isArray(v)) return `[${v.map(repr).join(", ")}]`;
    if (v instanceof CMap) return `{${v.entries.map(([k, x]) => `${repr(k)}: ${repr(x)}`).join(", ")}}`;
    if (typeof v === "function") return "<fn>";
    if (v instanceof Atom) return `atom(${repr(v.v)})`;
    if (v instanceof Variant) return v.vals.length === 0 ? v.name : `${v.name}(${v.vals.map(repr).join(", ")})`;
    if (v instanceof CObj) {
      if (findMethod(v.cls, "toString")) return toStr(call(v, "toString", []));
      const ci = cls(v.cls);
      if (ci.data) return `${v.cls}(${ci.fields.filter((f) => v.fields.has(f)).map((f) => `${f}=${repr(v.fields.get(f))}`).join(", ")})`;
      return `<${v.cls}>`;
    }
    if (v instanceof Sheet) return `Sheet{${sheetCellsSorted(v).map((n) => `${n}: ${repr(cell(v, n))}`).join(", ")}}`;
    return String(v);
  }
  const s = (x) => x;
  const concat = (parts) => parts.map(toStr).join("");
  const println = (v) => { process.stdout.write(toStr(v) + "\n"); return null; };
  const print = (v) => { process.stdout.write(toStr(v)); return null; };

  // ============================================================ operators
  function eq(a, b) {
    if (isNil(a) && isNil(b)) return true;
    if (typeof a === "boolean" && typeof b === "boolean") return a === b;
    if (isInt(a) && isInt(b)) return a === b;
    if (isNum(a) && isNum(b)) {
      if (isFloat(a) || isFloat(b)) return asF64(a) === asF64(b);
      return decCmp(toDec(a), toDec(b)) === 0;
    }
    if (a instanceof Qty && b instanceof Qty) { const f = factor(b.unit, a.unit); return f !== null && eq(a.amt, convertAmt(b.amt, f)); }
    if (typeof a === "string" && typeof b === "string") return a === b;
    if (Array.isArray(a) && Array.isArray(b)) return a.length === b.length && a.every((x, i) => eq(x, b[i]));
    if (a instanceof CMap && b instanceof CMap) return a.entries.length === b.entries.length && a.entries.every(([k, v]) => b.entries.some(([k2, v2]) => eq(k, k2) && eq(v, v2)));
    if (a instanceof Variant && b instanceof Variant) return a.name === b.name && a.en === b.en && a.vals.length === b.vals.length && a.vals.every((x, i) => eq(x, b.vals[i]));
    if (a instanceof CObj && b instanceof CObj) {
      if (a === b) return true;
      if (a.cls !== b.cls || !cls(a.cls).data) return false;
      for (const [n, v] of a.fields) if (!b.fields.has(n) || !eq(v, b.fields.get(n))) return false;
      return true;
    }
    return a === b;
  }
  const opEq = (a, b) => eq(a, b);
  const opNe = (a, b) => !eq(a, b);

  function numOp(a, b, op, fi, fd, ff) {
    if (isInt(a) && isInt(b)) return chkI(fi(a, b), `${a} ${op} ${b}`);
    if (isNum(a) && isNum(b)) {
      if (isFloat(a) || isFloat(b)) return ff(asF64(a), asF64(b));
      return fd(toDec(a), toDec(b));
    }
    fail(`operator '${op}' cannot be applied to ${typeName(a)} and ${typeName(b)}`);
  }
  const isOrd = (v) => v instanceof Qty && v.unit.length === 1 && v.unit[0][0] === "ord";
  function ordinalSuffix(n) {
    n = n < 0n ? -n : n;
    if (n % 100n >= 11n && n % 100n <= 13n) return "th";
    return { 1n: "st", 2n: "nd", 3n: "rd" }[n % 10n] ?? "th";
  }
  function ordOp(a, b, op) {
    if (!isOrd(a) && !isOrd(b)) return undefined;
    const amt = (v) => v instanceof Qty ? v.amt : v;
    const oa = isOrd(a), ob = isOrd(b);
    if ((op === "+" || op === "-") && oa && !ob && isInt(b)) return mkQty(arith(amt(a), b, op), [["ord", 1]]);
    if (op === "+" && !oa && ob && isInt(a)) return mkQty(add(a, amt(b)), [["ord", 1]]);
    if (op === "-" && oa && ob) return sub(amt(a), amt(b));
    if (op === "+" && oa && ob) fail("cannot add two ordinals (positions)");
    if (op === "+" || op === "-") fail(`ordinals only move by whole cardinal numbers, not by ${typeName(oa ? b : a)}`);
    fail(`operator '${op}' is not defined for ordinals`);
  }
  function qtyOp(a, b, op) {
    const o = ordOp(a, b, op); if (o !== undefined) return o;
    const verb = { "-": "subtract", "*": "multiply", "/": "divide" }[op] ?? "add";
    if (a instanceof Qty && b instanceof Qty) {
      if (op === "+" || op === "-" || op === "%") return mkQty(arith(a.amt, qtyAlign(a, b, verb), op), a.unit);
      if (op === "*") {
        const f = factor(b.unit, a.unit);
        if (f && !unitSame(a.unit, b.unit)) return mkQty(arith(a.amt, convertAmt(b.amt, f), "*"), unitCombine(a.unit, a.unit, 1));
        return mkQty(arith(a.amt, b.amt, "*"), unitCombine(a.unit, b.unit, 1));
      }
      if (op === "/") {
        const f = factor(b.unit, a.unit);
        if (f) return arith(a.amt, convertAmt(b.amt, f), "/");
        return mkQty(arith(a.amt, b.amt, "/"), unitCombine(a.unit, b.unit, -1));
      }
      fail(`operator '${op}' cannot be applied to quantities`);
    }
    if (a instanceof Qty && isNum(b)) {
      if (op === "*" || op === "/") return mkQty(arith(a.amt, b, op), a.unit);
      fail(`cannot ${verb} a plain number and ${toStr(a)}; give the number a unit`);
    }
    if (isNum(a) && b instanceof Qty) {
      if (op === "*") return mkQty(arith(a, b.amt, "*"), b.unit);
      if (op === "/") return mkQty(arith(a, b.amt, "/"), unitCombine([], b.unit, -1));
      fail(`cannot ${verb} a plain number and ${toStr(b)}; give the number a unit`);
    }
    return undefined;
  }
  const arith = (a, b, op) => op === "+" ? add(a, b) : op === "-" ? sub(a, b) : op === "*" ? mul(a, b) : op === "/" ? div(a, b) : rem(a, b);
  function add(a, b) {
    const q = qtyOp(a, b, "+"); if (q !== undefined) return q;
    if (typeof a === "string" || typeof b === "string") return toStr(a) + toStr(b);
    if (Array.isArray(a) && Array.isArray(b)) return [...a, ...b];
    return numOp(a, b, "+", (x, y) => x + y, decAdd, (x, y) => x + y);
  }
  function sub(a, b) { const q = qtyOp(a, b, "-"); if (q !== undefined) return q; return numOp(a, b, "-", (x, y) => x - y, decSub, (x, y) => x - y); }
  function mul(a, b) { const q = qtyOp(a, b, "*"); if (q !== undefined) return q; return numOp(a, b, "*", (x, y) => x * y, decMul, (x, y) => x * y); }
  function div(a, b) {
    const q = qtyOp(a, b, "/"); if (q !== undefined) return q;
    if (isInt(a) && b === 0n) fail("division by zero");
    return numOp(a, b, "/", (x, y) => x / y, decDiv, (x, y) => x / y);
  }
  function rem(a, b) {
    const q = qtyOp(a, b, "%"); if (q !== undefined) return q;
    if (isInt(a) && b === 0n) fail("division by zero");
    return numOp(a, b, "%", (x, y) => x % y, decRem, (x, y) => x % y);
  }
  function pow(a, b) {
    if (isInt(a) && isInt(b) && b >= 0n) return chkI(a ** b, "^");
    if (a instanceof Dec && isInt(b) && b >= 0n) { let r = decFromI(1n); for (let i = 0n; i < b; i++) r = decMul(r, a); return r; }
    if (a instanceof Qty && isInt(b)) return mkQty(pow(a.amt, b), a.unit.map(([n, e]) => [n, e * Number(b)]));
    return Math.pow(asF64(a), asF64(b));
  }
  function neg(a) {
    if (isInt(a)) return -a;
    if (isFloat(a)) return -a;
    if (a instanceof Dec) return decNeg(a);
    if (a instanceof Qty) return mkQty(neg(a.amt), a.unit);
    fail(`cannot negate ${typeName(a)}`);
  }
  const not = (a) => !truthy(a);
  const amp = (a, b) => toStr(a) + toStr(b);
  function cmp(a, b) {
    if (isInt(a) && isInt(b)) return a < b ? -1 : a > b ? 1 : 0;
    if (isNum(a) && isNum(b)) {
      if (isFloat(a) || isFloat(b)) { const x = asF64(a), y = asF64(b); return x < y ? -1 : x > y ? 1 : 0; }
      return decCmp(toDec(a), toDec(b));
    }
    if (a instanceof Qty && b instanceof Qty) return cmp(a.amt, qtyAlign(a, b, "compare"));
    if (typeof a === "string" && typeof b === "string") return a < b ? -1 : a > b ? 1 : 0;
    if (typeof a === "boolean" && typeof b === "boolean") return a === b ? 0 : a ? 1 : -1;
    fail(`cannot compare ${typeName(a)} with ${typeName(b)}`);
  }
  const lt = (a, b) => cmp(a, b) < 0, le = (a, b) => cmp(a, b) <= 0, gt = (a, b) => cmp(a, b) > 0, ge = (a, b) => cmp(a, b) >= 0;
  const notNull = (v, what) => isNil(v) ? fail(`null pointer: ${what} is nil`) : v;
  function tryBranch(v) {
    if (v instanceof Variant && v.en === "Result") return v.name === "Ok" ? { err: false, v: v.vals[0] } : { err: true, v };
    if (isNil(v)) return { err: true, v: null };
    return { err: false, v };
  }
  function range(a, b) { const out = []; for (let i = asInt(a); i < asInt(b); i++) out.push(i); return out; }
  function iter(v) {
    if (Array.isArray(v)) return v;
    if (v instanceof CMap) return v.entries.map(([k, x]) => [k, x]);
    if (typeof v === "string") return Array.from(v);
    if (isNil(v)) return [];
    fail(`${typeName(v)} is not iterable`);
  }
  function assert(c, msg) { if (!truthy(c)) fail(`assertion failed: ${toStr(msg)}`); return null; }

  // ============================================================ builtin methods
  const arg = (args, i) => (i < args.length ? args[i] : null);
  const noMethod = (recv, name) => fail(`${typeName(recv)} has no method ${name}`);
  const INT_RE = /^[+-]?\d+$/;
  const FLOAT_RE = /^[+-]?(\d+\.?\d*|\.\d+)([eE][+-]?\d+)?$/;
  function parseI64(t) { if (!INT_RE.test(t)) return null; const v = BigInt(t); return v < I64_MIN || v > I64_MAX ? null : v; }
  function builtinMethod(recv, name, args) {
    const a0 = () => arg(args, 0), a1 = () => arg(args, 1);
    if (name === "toString") return toStr(recv);
    if (name === "equals") return eq(recv, a0());
    if (name === "isNil") return isNil(recv);
    if (typeof recv === "string") {
      const st = recv;
      switch (name) {
        case "length": case "count": return BigInt(Array.from(st).length);
        case "code": return BigInt(st.length ? st.codePointAt(0) : 0);
        case "upper": case "uppercase": return st.toUpperCase();
        case "lower": case "lowercase": return st.toLowerCase();
        case "trim": return st.trim();
        case "contains": return st.includes(toStr(a0()));
        case "startsWith": return st.startsWith(toStr(a0()));
        case "endsWith": return st.endsWith(toStr(a0()));
        case "split": return st.split(toStr(a0()));
        case "chars": return Array.from(st);
        case "repeat": { const n = asInt(a0()); return st.repeat(n < 0n ? 0 : Number(n)); }
        case "replace": return st.split(toStr(a0())).join(toStr(a1()));
        case "isEmpty": return st.length === 0;
        case "reverse": return Array.from(st).reverse().join("");
        case "substring": { const c = Array.from(st); const a = Math.max(0, Number(asInt(a0()))); const b = args.length > 1 ? Number(asInt(a1())) : c.length; return c.slice(Math.min(a, c.length), Math.min(b, c.length)).join(""); }
        case "toInt": return parseI64(st.trim());
        case "toFloat": { const t = st.trim(); return FLOAT_RE.test(t) ? parseFloat(t) : null; }
        case "toDecimal": { const t = st.trim(); return FLOAT_RE.test(t) && !/[eE]/.test(t) ? decParse(t.replace(/^\+/, "")) : null; }
      }
      return noMethod(recv, name);
    }
    if (recv instanceof Qty) {
      switch (name) {
        case "amount": case "cardinal": return recv.amt;
        case "index": if (isOrd(recv)) return sub(recv.amt, 1n); break;
        case "unit": return unitShow(recv.unit);
        case "to": return convertTo(recv, toStr(a0()));
        case "abs": return mkQty(builtinMethod(recv.amt, "abs", []), recv.unit);
        case "round": return mkQty(sheets("ROUND", [recv.amt, args.length ? a0() : 0n]), recv.unit);
      }
      return noMethod(recv, name);
    }
    if (isNum(recv)) {
      switch (name) {
        case "withUnit": return qty(recv, toStr(a0()));
        case "ordinal": return qty(asInt(recv), "ord");
        case "toDecimal": return toDec(recv);
        case "toFloat": return asF64(recv);
        case "toInt": return BigInt(Math.trunc(asF64(recv)));
        case "abs": return isInt(recv) ? (recv < 0n ? -recv : recv) : recv instanceof Dec ? (recv.m < 0n ? decNeg(recv) : recv) : Math.abs(recv);
        case "pow": return pow(recv, a0());
        case "sqrt": return Math.sqrt(asF64(recv));
        case "round": return sheets("ROUND", [recv, args.length ? a0() : 0n]);
        case "max": return cmp(recv, a0()) >= 0 ? recv : a0();
        case "min": return cmp(recv, a0()) <= 0 ? recv : a0();
      }
      return noMethod(recv, name);
    }
    if (Array.isArray(recv)) return vecMethod(recv, name, args);
    if (recv instanceof CMap) {
      const m = recv.entries;
      switch (name) {
        case "get": return index(recv, a0());
        case "assoc": return assoc(recv, a0(), a1());
        case "dissoc": { const k = a0(); return new CMap(m.filter(([kk]) => !eq(kk, k))); }
        case "keys": return m.map(([k]) => k);
        case "values": return m.map(([, v]) => v);
        case "entries": return iter(recv);
        case "count": case "size": return BigInt(m.length);
        case "isEmpty": return m.length === 0;
        case "containsKey": case "has": { const k = a0(); return m.some(([kk]) => eq(kk, k)); }
        case "merge": return merge(recv, a0());
        case "update": { const k = a0(); return assoc(recv, k, apply(a1(), [index(recv, k)])); }
      }
      const f = m.find(([k]) => k === name);
      if (f && typeof f[1] === "function") return apply(f[1], args);
      return noMethod(recv, name);
    }
    if (recv instanceof Atom) {
      switch (name) {
        case "deref": case "get": return recv.v;
        case "reset": recv.v = a0(); return a0();
        case "swap": { const nv = apply(a0(), [recv.v, ...args.slice(1)]); recv.v = nv; return nv; }
      }
      return noMethod(recv, name);
    }
    if (recv instanceof Variant && recv.en === "Result") {
      const isOk = recv.name === "Ok";
      switch (name) {
        case "isOk": return isOk;
        case "isErr": return !isOk;
        case "unwrap": return isOk ? recv.vals[0] : fail(`called unwrap() on Err(${repr(recv.vals[0])})`);
        case "unwrapOr": return isOk ? recv.vals[0] : a0();
        case "map": return isOk ? ok(apply(a0(), [recv.vals[0]])) : recv;
      }
      return noMethod(recv, name);
    }
    if (recv instanceof Sheet) {
      switch (name) {
        case "get": return cell(recv, toStr(a0()));
        case "set": setCell(recv, toStr(a0()), a1()); return recv;
        case "cells": return sheetCellsSorted(recv);
      }
      return cell(recv, name);
    }
    if (typeof recv === "function") { if (name === "call" || name === "invoke") return apply(recv, args); return noMethod(recv, name); }
    if (isNil(recv)) fail(`cannot call ${name}() on nil`);
    return noMethod(recv, name);
  }
  function vecMethod(items, name, args) {
    const a0 = () => arg(args, 0), a1 = () => arg(args, 1);
    const n = (v) => { const i = asInt(v); return i < 0n ? 0 : Number(i); };
    switch (name) {
      case "count": case "size": case "length": return BigInt(items.length);
      case "isEmpty": return items.length === 0;
      case "get": return index(items, a0());
      case "first": return items.length ? items[0] : null;
      case "last": return items.length ? items[items.length - 1] : null;
      case "conj": case "push": case "plus": return [...items, a0()];
      case "assoc": return assoc(items, a0(), a1());
      case "concat": return add(items, a0());
      case "map": { const f = a0(); return items.map((x) => apply(f, [x])); }
      case "mapIndexed": { const f = a0(); return items.map((x, i) => apply(f, [BigInt(i), x])); }
      case "filter": { const f = a0(); return items.filter((x) => truthy(apply(f, [x]))); }
      case "remove": { const f = a0(); return items.filter((x) => !truthy(apply(f, [x]))); }
      case "reduce": case "fold": { const f = a1(); return items.reduce((acc, x) => apply(f, [acc, x]), a0()); }
      case "each": case "forEach": { const f = a0(); for (const x of items) apply(f, [x]); return null; }
      case "any": { const f = a0(); return items.some((x) => truthy(apply(f, [x]))); }
      case "all": { const f = a0(); return items.every((x) => truthy(apply(f, [x]))); }
      case "find": { const f = a0(); const r = items.find((x) => truthy(apply(f, [x]))); return r === undefined ? null : r; }
      case "sum": return sheets("SUM", [items]);
      case "min": return items.length ? items.reduce((a, b) => cmp(b, a) < 0 ? b : a) : null;
      case "max": return items.length ? items.reduce((a, b) => cmp(b, a) > 0 ? b : a) : null;
      case "join": return items.map(toStr).join(args.length ? toStr(a0()) : "");
      case "contains": { const x = a0(); return items.some((y) => eq(y, x)); }
      case "indexOf": { const x = a0(); return BigInt(items.findIndex((y) => eq(y, x))); }
      case "reverse": case "reversed": return [...items].reverse();
      case "sort": case "sorted": return [...items].sort(cmp);
      case "sortBy": { const f = a0(); return items.map((x) => [apply(f, [x]), x]).sort((a, b) => cmp(a[0], b[0])).map((p) => p[1]); }
      case "take": return items.slice(0, n(a0()));
      case "drop": return items.slice(n(a0()));
      case "distinct": { const v = []; for (const x of items) if (!v.some((y) => eq(y, x))) v.push(x); return v; }
      case "flatten": return items.flatMap((x) => Array.isArray(x) ? x : [x]);
      case "zip": { const o = iter(a0()); return items.slice(0, o.length).map((a, i) => [a, o[i]]); }
    }
    return noMethod(items, name);
  }

  // ============================================================ Google Sheets functions
  const flat = (args) => { const out = []; for (const a of args) { if (Array.isArray(a)) out.push(...flat(a)); else out.push(a); } return out; };
  const nums = (args) => flat(args).filter((v) => isNum(v) || v instanceof Qty);
  const sumVals = (vs) => vs.length === 0 ? 0n : vs.slice(1).reduce((a, b) => add(a, b), vs[0]);
  function criteria(c) {
    if (typeof c === "string") {
      for (const op of [">=", "<=", "<>", ">", "<", "="]) {
        if (c.startsWith(op)) {
          const rest = c.slice(op.length), r = rest.trim();
          const target = (r !== "" && /^[0-9.\-]+$/.test(r) && !Number.isNaN(parseFloat(r)) && FLOAT_RE.test(r)) ? decParse(r) : rest;
          return (v) => {
            const comparable = (target instanceof Dec && isNum(v)) || (typeof target === "string" && typeof v === "string");
            if (!comparable) return op === "<>";
            const o = cmp(v, target);
            return { ">=": o >= 0, "<=": o <= 0, "<>": o !== 0, ">": o > 0, "<": o < 0, "=": o === 0 }[op];
          };
        }
      }
    }
    return (v) => eq(v, c);
  }
  function roundTo(x, d, mode) {
    if (isInt(x) && d >= 0n) return x;
    if (isFloat(x)) {
      const m = Math.pow(10, Number(d)), y = x * m;
      const r = mode === 0 ? rustRound(y) : mode === 1 ? Math.ceil(Math.abs(y)) * Math.sign(y) : Math.trunc(y);
      return r / m;
    }
    if (x instanceof Qty) return mkQty(roundTo(x.amt, d, mode), x.unit);
    return decRound(toDec(x), d, mode);
  }
  const rustRound = (y) => Math.sign(y) * Math.round(Math.abs(y)); // half away from zero
  function avg(n) {
    let total = sumVals(n);
    if (isInt(total)) total = decFromI(total);
    return div(total, BigInt(n.length));
  }
  function sheets(name, args) {
    const a = (i) => arg(args, i);
    const na = () => "#N/A";
    switch (name) {
      case "SUM": return sumVals(nums(args));
      case "PRODUCT": return nums(args).reduce((x, y) => mul(x, y), 1n);
      case "AVERAGE": { const n = nums(args); return n.length ? avg(n) : err("#DIV/0!"); }
      case "MEDIAN": { const n = [...nums(args)].sort(cmp); if (!n.length) return err(na()); const l = n.length; return l % 2 === 1 ? n[(l - 1) / 2] : avg(n.slice(l / 2 - 1, l / 2 + 1)); }
      case "MIN": { const n = nums(args); return n.length ? n.reduce((x, y) => cmp(y, x) < 0 ? y : x) : 0n; }
      case "MAX": { const n = nums(args); return n.length ? n.reduce((x, y) => cmp(y, x) > 0 ? y : x) : 0n; }
      case "COUNT": return BigInt(nums(args).length);
      case "COUNTA": return BigInt(flat(args).filter((v) => !isNil(v) && v !== "").length);
      case "ROUND": return roundTo(a(0), asInt(a(1) ?? 0n), 0);
      case "ROUNDUP": return roundTo(a(0), asInt(a(1) ?? 0n), 1);
      case "ROUNDDOWN": return roundTo(a(0), asInt(a(1) ?? 0n), 2);
      case "INT": { const v = a(0); if (isFloat(v)) return BigInt(Math.floor(v)); const d = toDec(v); const t = decRound(d, 0, 2); return (d.m < 0n && decCmp(t, d) !== 0) ? t.m - 1n : t.m; }
      case "ABS": return builtinMethod(a(0), "abs", []);
      case "SQRT": return Math.sqrt(asF64(a(0)));
      case "UNIT": return qty(a(0), toStr(a(1)));
      case "CONVERT": return convertTo(a(0), toStr(a(1)));
      case "POWER": return pow(a(0), a(1));
      case "MOD": { const x = a(0), y = a(1); const r = rem(x, y); if (!isFloat(r) && cmp(r, 0n) !== 0 && ((cmp(r, 0n) < 0) !== (cmp(y, 0n) < 0))) return add(r, y); return r; }
      case "LEN": return BigInt(Array.from(toStr(a(0))).length);
      case "UPPER": return toStr(a(0)).toUpperCase();
      case "LOWER": return toStr(a(0)).toLowerCase();
      case "TRIM": return toStr(a(0)).split(/\s+/).filter((x) => x).join(" ");
      case "LEFT": { const n = args.length > 1 ? Number(asInt(a(1))) : 1; return Array.from(toStr(a(0))).slice(0, Math.max(0, n)).join(""); }
      case "RIGHT": { const n = args.length > 1 ? Number(asInt(a(1))) : 1; const c = Array.from(toStr(a(0))); return c.slice(Math.max(0, c.length - Math.max(0, n))).join(""); }
      case "MID": { const c = Array.from(toStr(a(0))); const st = Math.max(0, Number(asInt(a(1))) - 1); return c.slice(st, st + Math.max(0, Number(asInt(a(2))))).join(""); }
      case "CONCATENATE": return concat(flat(args));
      case "TEXTJOIN": { const sep = toStr(a(0)); const skip = truthy(a(1)); return flat(args.slice(2)).filter((v) => !(skip && (isNil(v) || v === ""))).map(toStr).join(sep); }
      case "SPLIT": return toStr(a(0)).split(toStr(a(1))).filter((x) => x).map((x) => parseI64(x) ?? (x.includes(".") && FLOAT_RE.test(x) ? decParse(x) : x));
      case "SUBSTITUTE": return toStr(a(0)).split(toStr(a(1))).join(toStr(a(2)));
      case "SUMIF": case "COUNTIF": case "AVERAGEIF": {
        const rg = flat([a(0)]); const pred = criteria(a(1)); const vals = args.length > 2 ? flat([a(2)]) : rg;
        const picked = rg.map((r, i) => [r, vals[i]]).filter(([r, v]) => v !== undefined && pred(r)).map(([, v]) => v);
        if (name === "COUNTIF") return BigInt(picked.length);
        if (name === "SUMIF") return sumVals(nums(picked));
        const n = nums(picked); return n.length ? avg(n) : err("#DIV/0!");
      }
      case "FILTER": { const items = iter(a(0)), conds = iter(a(1)); const out = items.slice(0, conds.length).filter((_, i) => truthy(conds[i])); return out.length ? out : err(na()); }
      case "SORT": { const asc = args.length < 2 || a(args.length - 1) !== false; const v = [...iter(a(0))].sort(cmp); if (!asc) v.reverse(); return v; }
      case "UNIQUE": return builtinMethod(a(0), "distinct", []);
      case "SEQUENCE": {
        const rows = asInt(a(0)), cols = args.length > 1 ? asInt(a(1)) : 1n, start = args.length > 2 ? asInt(a(2)) : 1n, step = args.length > 3 ? asInt(a(3)) : 1n;
        let k = start; const out = [];
        for (let r = 0n; r < rows; r++) { const row = []; for (let c = 0n; c < cols; c++) { row.push(k); k += step; } out.push(cols === 1n ? row[0] : row); }
        return out;
      }
      case "VLOOKUP": {
        const key = a(0), col = asInt(a(2)) - 1n;
        for (const row of iter(a(1))) { const r = iter(row); if (r.length && eq(r[0], key)) { const v = r[Number(col < 0n ? 0n : col)]; return v === undefined ? err("#REF!") : v; } }
        return err(na());
      }
      case "INDEX": { const row = index(a(0), asInt(a(1)) - 1n); return args.length > 2 ? index(row, asInt(a(2)) - 1n) : row; }
      case "MATCH": { const key = a(0); const p = iter(a(1)).findIndex((v) => eq(v, key)); return p >= 0 ? BigInt(p + 1) : err(na()); }
      case "ISBLANK": return isNil(a(0)) || a(0) === "";
      case "ISNUMBER": return isNum(a(0));
      case "ISTEXT": return typeof a(0) === "string";
      case "ISERROR": { const v = a(0); return isNil(v) || (v instanceof Variant && v.en === "Result" && v.name === "Err"); }
      case "MAP": { const f = a(1); return iter(a(0)).map((x) => apply(f, [x])); }
      case "REDUCE": { const f = a(2); return iter(a(1)).reduce((acc, x) => apply(f, [acc, x]), a(0)); }
      case "readFile": try { return ok(require("fs").readFileSync(toStr(a(0)), "utf8")); } catch (e) { return err(e.message); }
      case "writeFile": try { require("fs").writeFileSync(toStr(a(0)), toStr(a(1))); return ok(null); } catch (e) { return err(e.message); }
      case "args": return process.argv.slice(2);
      case "exec": { const r = require("child_process").spawnSync(toStr(a(0)), iter(a(1)).map(toStr), { stdio: "inherit" }); return BigInt(r.status ?? 127); }
      case "mkdirs": try { require("fs").mkdirSync(toStr(a(0)), { recursive: true }); return ok(null); } catch (e) { return err(e.message); }
      case "exit": process.exit(Number(asInt(a(0))));
      case "eprintln": process.stderr.write(toStr(a(0)) + "\n"); return null;
      case "TRUE": return true;
      case "FALSE": return false;
    }
    fail(`unknown function ${name}`);
  }

  function run(main) {
    try { main(); }
    catch (e) {
      if (e instanceof ChimeraError) { process.stderr.write(`Error: ${e.message}\n`); process.exit(1); }
      if (e instanceof RangeError) { process.stderr.write(`Error: ${e.message}\n`); process.exit(1); }
      throw e;
    }
  }

  return {
    fail, variant, ok, err, dec, qty, convertTo, registerClass, registerMethod, newObj, beginInit, finishNew, checkInvariants,
    call, callSafe, callSelf, callSuper, callFrom, apply, cpl, isInstance, isVariant, vfield, callGeneric,
    set, setIn, get, getSafe, map, merge, assoc, index, copy, atom, sheet, formula, cell, cellRange,
    typeName, truthy, asInt, asF64, toStr, repr, s, concat, println, print,
    eq, opEq, opNe, add, sub, mul, div, rem, pow, neg, not, amp, lt, le, gt, ge, notNull, tryBranch, range, iter, assert,
    builtinMethod, sheets, run,
  };
})();
