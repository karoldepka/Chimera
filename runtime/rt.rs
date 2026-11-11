//! Chimera runtime for the Rust backend (`chimera_rt`).
//! Generated programs `mod rt;` this file. Dependency-free.
#![allow(dead_code, non_snake_case)]

use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::rc::Rc;

// ============================================================ values

#[derive(Clone)]
pub enum Value {
    Nil,
    Bool(bool),
    Int(i64),
    Float(f64),
    Dec(Dec),
    Qty(Rc<QtyV>),
    Str(Rc<str>),
    Vec(Rc<Vec<Value>>),
    Map(Rc<Vec<(Value, Value)>>), // insertion-ordered persistent map
    Obj(Rc<RefCell<Obj>>),
    Variant(Rc<VariantV>),
    Func(Rc<dyn Fn(Vec<Value>) -> Value>),
    Atom(Rc<RefCell<Value>>),
    Sheet(Rc<RefCell<SheetV>>),
}

pub struct Obj {
    pub class: &'static str,
    pub fields: Vec<(&'static str, Value)>,
    pub inited: Vec<&'static str>,
}

pub struct VariantV {
    pub enum_name: &'static str,
    pub name: &'static str,
    pub names: &'static [&'static str],
    pub vals: Vec<Value>,
}

pub enum Cell {
    Val(Value),
    Formula(Rc<dyn Fn(&Value) -> Value>),
}

pub struct SheetV {
    pub cells: Vec<(String, Cell)>,
    pub evaluating: Vec<String>,
}

impl Value {
    pub fn is_nil(&self) -> bool { matches!(self, Value::Nil) }
}

pub struct QtyV {
    pub amt: Value,
    pub unit: Vec<(String, i32)>,
}

pub fn s(x: &str) -> Value { Value::Str(Rc::from(x)) }
pub fn vec(items: Vec<Value>) -> Value { Value::Vec(Rc::new(items)) }
pub fn func(f: impl Fn(Vec<Value>) -> Value + 'static) -> Value { Value::Func(Rc::new(f)) }
pub fn variant(enum_name: &'static str, name: &'static str, names: &'static [&'static str], vals: Vec<Value>) -> Value {
    Value::Variant(Rc::new(VariantV { enum_name, name, names, vals }))
}
pub fn ok(v: Value) -> Value { variant("Result", "Ok", &["value"], vec![v]) }
pub fn err(v: Value) -> Value { variant("Result", "Err", &["error"], vec![v]) }

pub fn fail(msg: impl Into<String>) -> ! { std::panic::panic_any(ChimeraError(msg.into())) }
pub struct ChimeraError(pub String);

pub fn init() {
    std::panic::set_hook(Box::new(|info| {
        let msg = if let Some(e) = info.payload().downcast_ref::<ChimeraError>() { e.0.clone() }
            else if let Some(s) = info.payload().downcast_ref::<&str>() { s.to_string() }
            else if let Some(s) = info.payload().downcast_ref::<String>() { s.clone() }
            else { "unknown error".into() };
        eprintln!("Error: {}", msg);
    }));
}

pub fn run(main: fn() -> Value) {
    // run on a thread with a large stack: Chimera programs (e.g. the self-hosted compiler) recurse deeply
    let t = std::thread::Builder::new().stack_size(1 << 30).spawn(move || {
        init();
        std::panic::catch_unwind(|| { main(); }).is_ok()
    }).unwrap();
    if !t.join().unwrap_or(false) { std::process::exit(1); }
}

// ============================================================ classes & dispatch

pub type MethodFn = fn(Value, Vec<Value>) -> Value;
pub type InvFn = fn(&Value) -> Option<&'static str>;

pub struct ClassInfo {
    pub name: &'static str,
    pub mro: &'static [&'static str],
    pub methods: Vec<(&'static str, MethodFn)>,
    pub invariant: Option<InvFn>,
    pub has_invariants: bool,
    pub data: bool,
    pub fields: &'static [&'static str],
}

pub type GFn = fn(Vec<Value>, &dyn Fn(Vec<Value>) -> Value) -> Value;

#[derive(Clone)]
pub struct GMethod {
    pub qual: u8, // 0 primary, 1 before, 2 after, 3 around
    pub specs: &'static [&'static str],
    pub f: GFn,
}

thread_local! {
    static CLASSES: RefCell<HashMap<&'static str, Rc<ClassInfo>>> = RefCell::new(HashMap::new());
    static GENERICS: RefCell<HashMap<&'static str, Vec<GMethod>>> = RefCell::new(HashMap::new());
}

pub fn register_class(c: ClassInfo) { CLASSES.with(|m| { m.borrow_mut().insert(c.name, Rc::new(c)); }); }
pub fn register_method(gf: &'static str, m: GMethod) { GENERICS.with(|g| g.borrow_mut().entry(gf).or_default().push(m)); }
fn class(name: &str) -> Rc<ClassInfo> { CLASSES.with(|m| m.borrow().get(name).cloned()).unwrap_or_else(|| fail(format!("unknown class {}", name))) }

pub fn new_obj(class: &'static str) -> Value {
    Value::Obj(Rc::new(RefCell::new(Obj { class, fields: vec![], inited: vec![] })))
}
/// returns true the first time a class part of `this` is initialized (diamond inheritance runs each init once)
pub fn begin_init(this: &Value, part: &'static str) -> bool {
    if let Value::Obj(o) = this {
        let mut o = o.borrow_mut();
        if o.inited.contains(&part) { return false; }
        o.inited.push(part);
        return true;
    }
    false
}
pub fn finish_new(this: Value) -> Value { check_invariants(&this); this }

fn find_method(cls: &str, name: &str, after: Option<&str>) -> Option<MethodFn> {
    let ci = class(cls);
    let mut skipping = after.is_some();
    for c in ci.mro {
        if skipping { if Some(*c) == after { skipping = false; } continue; }
        if let Some((_, f)) = class(c).methods.iter().find(|(n, _)| *n == name) { return Some(*f); }
    }
    None
}

pub fn check_invariants(this: &Value) {
    if let Value::Obj(o) = this {
        let cls = o.borrow().class;
        let ci = class(cls);
        if !ci.has_invariants { return; }
        for c in ci.mro {
            if let Some(inv) = class(c).invariant {
                if let Some(msg) = inv(this) { fail(format!("invariant violation in {} (class {}): {}", cls, c, msg)); }
            }
        }
    }
}

fn obj_class(v: &Value) -> Option<&'static str> { if let Value::Obj(o) = v { Some(o.borrow().class) } else { None } }

/// qualified call `recv.name(args)`: dynamic dispatch, then class invariants (Eiffel semantics)
pub fn call(recv: &Value, name: &str, args: Vec<Value>) -> Value {
    if let Some(cls) = obj_class(recv) {
        if let Some(f) = find_method(cls, name, None) {
            let r = f(recv.clone(), args);
            check_invariants(recv);
            return r;
        }
        let fv = get_field(recv, name);
        if let Some(fv) = fv { if let Value::Func(_) = fv { return apply(&fv, args); } }
    }
    builtin_method(recv, name, args)
}
pub fn call_safe(recv: &Value, name: &str, args: Vec<Value>) -> Value { if recv.is_nil() { Value::Nil } else { call(recv, name, args) } }
/// unqualified call inside a class: dynamic dispatch, no invariant check
pub fn call_self(this: &Value, name: &str, args: Vec<Value>) -> Value {
    let cls = obj_class(this).unwrap();
    match find_method(cls, name, None) { Some(f) => f(this.clone(), args), None => builtin_method(this, name, args) }
}
/// `super.name(args)` from class `from`: next class after `from` in the receiver's MRO (cooperative, like Python/CLOS)
pub fn call_super(this: &Value, from: &str, name: &str, args: Vec<Value>) -> Value {
    let cls = obj_class(this).unwrap();
    match find_method(cls, name, Some(from)) { Some(f) => f(this.clone(), args), None => fail(format!("no super method {} after {}", name, from)) }
}
/// `super<Q>.name(args)`: the implementation visible from Q
pub fn call_from(this: &Value, q: &str, name: &str, args: Vec<Value>) -> Value {
    match find_method(q, name, None) { Some(f) => f(this.clone(), args), None => fail(format!("{} has no method {}", q, name)) }
}

pub fn apply(f: &Value, args: Vec<Value>) -> Value {
    match f { Value::Func(f) => f(args), other => fail(format!("{} is not a function", type_name(other))) }
}

/// class precedence list used by `is` and multiple dispatch
pub fn cpl(v: &Value) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = match v {
        Value::Nil => vec!["Nil"], Value::Bool(_) => vec!["Bool"], Value::Int(_) => vec!["Int", "Number"], Value::Float(_) => vec!["Float", "Number"], Value::Dec(_) => vec!["Decimal", "Number"], Value::Qty(_) => vec!["Quantity"],
        Value::Str(_) => vec!["String"], Value::Vec(_) => vec!["Vec"], Value::Map(_) => vec!["Map"], Value::Func(_) => vec!["Fn"],
        Value::Atom(_) => vec!["Atom"], Value::Sheet(_) => vec!["Sheet"],
        Value::Obj(o) => { let c = o.borrow().class; class(c).mro.to_vec() }
        Value::Variant(x) => vec![x.name, x.enum_name],
    };
    out.push("Any");
    out
}
pub fn is_variant(v: &Value, name: &str) -> bool { matches!(v, Value::Variant(x) if x.name == name) }
pub fn vfield(v: &Value, i: usize) -> Value { if let Value::Variant(x) = v { x.vals.get(i).cloned().unwrap_or(Value::Nil) } else { Value::Nil } }
pub fn is_instance(v: &Value, ty: &str) -> bool { cpl(v).contains(&ty) }

/// CLOS-style multiple dispatch with :around / :before / primary / :after and call-next-method
pub fn call_generic(gf: &'static str, args: Vec<Value>) -> Value {
    let methods = GENERICS.with(|g| g.borrow().get(gf).cloned().unwrap_or_default());
    let cpls: Vec<Vec<&'static str>> = args.iter().map(cpl).collect();
    let mut app: Vec<(Vec<usize>, GMethod)> = vec![];
    for m in methods {
        let mut key = vec![];
        let mut ok = true;
        for (i, sp) in m.specs.iter().enumerate() {
            match cpls[i].iter().position(|c| c == sp) { Some(p) => key.push(p), None => { ok = false; break; } }
        }
        if ok { app.push((key, m)); }
    }
    app.sort_by(|a, b| a.0.cmp(&b.0)); // most specific first
    let pick = |q: u8| -> Vec<GFn> { app.iter().filter(|(_, m)| m.qual == q).map(|(_, m)| m.f).collect() };
    let (prim, before, mut after, around) = (pick(0), pick(1), pick(2), pick(3));
    after.reverse();
    if prim.is_empty() {
        fail(format!("no applicable method for {}({})", gf, args.iter().map(|a| type_name(a)).collect::<Vec<_>>().join(", ")));
    }
    let core = move |a: Vec<Value>| -> Value {
        for b in &before { b(a.clone(), &|_| fail("callNext() is not allowed in a before method")); }
        let r = run_chain(&prim, 0, a.clone(), gf);
        for f in &after { f(a.clone(), &|_| fail("callNext() is not allowed in an after method")); }
        r
    };
    run_around(&around, 0, args, &core)
}
fn run_chain(ms: &[GFn], i: usize, args: Vec<Value>, gf: &str) -> Value {
    if i >= ms.len() { fail(format!("no next method for {}", gf)); }
    let next = |a: Vec<Value>| run_chain(ms, i + 1, a, gf);
    ms[i](args, &next)
}
fn run_around(ms: &[GFn], i: usize, args: Vec<Value>, core: &dyn Fn(Vec<Value>) -> Value) -> Value {
    if i >= ms.len() { return core(args); }
    let next = |a: Vec<Value>| run_around(ms, i + 1, a, core);
    ms[i](args, &next)
}

// ============================================================ fields, maps, records

pub fn get_field(v: &Value, name: &str) -> Option<Value> {
    if let Value::Obj(o) = v { return o.borrow().fields.iter().find(|(n, _)| *n == name).map(|(_, x)| x.clone()); }
    None
}
pub fn set(this: &Value, name: &'static str, val: Value) {
    if let Value::Obj(o) = this {
        let mut o = o.borrow_mut();
        match o.fields.iter_mut().find(|(n, _)| *n == name) { Some(slot) => slot.1 = val, None => o.fields.push((name, val)) }
        return;
    }
    fail(format!("cannot set field {} on {}", name, type_name(this)))
}
/// `x.f = v` where x is a local: objects mutate in place; maps/records/sheets produce an updated value
pub fn set_in(base: &Value, name: &'static str, val: Value) -> Value {
    match base {
        Value::Obj(_) => { set(base, name, val); base.clone() }
        Value::Map(_) => assoc(base, s(name), val),
        Value::Sheet(sh) => { set_cell(sh, name, Cell::Val(val)); base.clone() }
        _ => fail(format!("cannot set field {} on {}", name, type_name(base))),
    }
}

pub fn get(v: &Value, name: &str) -> Value {
    match v {
        Value::Obj(o) => {
            if let Some(x) = o.borrow().fields.iter().find(|(n, _)| *n == name) { return x.1.clone(); }
            let cls = o.borrow().class;
            if find_method(cls, name, None).is_some() { let me = v.clone(); let n = name.to_string(); return func(move |a| call(&me, &n, a)); }
            fail(format!("{} has no field {}", cls, name))
        }
        Value::Map(m) => m.iter().find(|(k, _)| matches!(k, Value::Str(s) if &**s == name)).map(|(_, x)| x.clone()).unwrap_or(Value::Nil),
        Value::Variant(x) => match x.names.iter().position(|n| *n == name) { Some(i) => x.vals[i].clone(), None => builtin_method(v, name, vec![]) },
        Value::Sheet(_) => cell(v, name),
        Value::Nil => fail(format!("cannot read field {} of nil", name)),
        _ => builtin_method(v, name, vec![]),
    }
}
pub fn get_safe(v: &Value, name: &str) -> Value { if v.is_nil() { Value::Nil } else { get(v, name) } }

pub fn map(entries: Vec<(Value, Value)>) -> Value {
    let mut out = Value::Map(Rc::new(vec![]));
    for (k, v) in entries { out = assoc(&out, k, v); }
    out
}
pub fn merge(a: &Value, b: &Value) -> Value {
    match b {
        Value::Map(m) => { let mut out = a.clone(); for (k, v) in m.iter() { out = assoc(&out, k.clone(), v.clone()); } out }
        Value::Obj(o) => { let mut out = a.clone(); for (k, v) in o.borrow().fields.iter() { out = assoc(&out, s(k), v.clone()); } out }
        Value::Nil => a.clone(),
        _ => fail(format!("cannot spread {} into a map", type_name(b))),
    }
}
pub fn assoc(m: &Value, k: Value, v: Value) -> Value {
    match m {
        Value::Map(entries) => {
            let mut e = (**entries).clone();
            match e.iter_mut().find(|(kk, _)| eq(kk, &k)) { Some(slot) => slot.1 = v, None => e.push((k, v)) }
            Value::Map(Rc::new(e))
        }
        Value::Vec(items) => {
            let i = as_int(&k);
            let mut e = (**items).clone();
            if i >= 0 && (i as usize) < e.len() { e[i as usize] = v; } else if i as usize == e.len() { e.push(v); } else { fail(format!("index {} out of bounds for length {}", i, e.len())); }
            Value::Vec(Rc::new(e))
        }
        _ => fail(format!("cannot assoc on {}", type_name(m))),
    }
}
pub fn index(c: &Value, k: &Value) -> Value {
    match c {
        Value::Vec(items) => { let i = as_int(k); if i < 0 { Value::Nil } else { items.get(i as usize).cloned().unwrap_or(Value::Nil) } }
        Value::Map(m) => m.iter().find(|(kk, _)| eq(kk, k)).map(|(_, v)| v.clone()).unwrap_or(Value::Nil),
        Value::Str(st) => { let i = as_int(k); st.chars().nth(i.max(0) as usize).map(|c| s(&c.to_string())).unwrap_or(Value::Nil) }
        Value::Sheet(_) => cell(c, &to_str(k)),
        Value::Obj(_) => get(c, &to_str(k)),
        _ => fail(format!("cannot index {}", type_name(c))),
    }
}
pub fn copy(o: &Value, changes: Vec<(&'static str, Value)>) -> Value {
    if let Value::Obj(ob) = o {
        let ob = ob.borrow();
        let mut fields = ob.fields.clone();
        for (k, v) in changes { match fields.iter_mut().find(|(n, _)| *n == k) { Some(s) => s.1 = v, None => fail(format!("no field {}", k)) } }
        let nv = Value::Obj(Rc::new(RefCell::new(Obj { class: ob.class, fields, inited: ob.inited.clone() })));
        drop(ob);
        check_invariants(&nv);
        return nv;
    }
    fail("copy() needs an object")
}

/// `xs = xs.push(v)` fast path: mutate in place when the vector is not shared (Rc::make_mut)
pub fn push_mut(xs: &mut Value, v: Value) {
    match xs { Value::Vec(items) => Rc::make_mut(items).push(v), other => { let n = builtin_method(other, "push", vec![v]); *other = n; } }
}
/// `this.xs = this.xs.push(v)` fast path: the field is the only owner, so push in place
pub fn field_push_mut(this: &Value, name: &'static str, v: Value) {
    if let Value::Obj(o) = this {
        let mut o = o.borrow_mut();
        if let Some(slot) = o.fields.iter_mut().find(|(n, _)| *n == name) { push_mut(&mut slot.1, v); return; }
    }
    fail(format!("no field {}", name))
}
/// `m = m.assoc(k, v)` fast path for unshared maps
pub fn assoc_mut(m: &mut Value, k: Value, v: Value) {
    match m {
        Value::Map(entries) => {
            let e = Rc::make_mut(entries);
            match e.iter_mut().find(|(kk, _)| eq(kk, &k)) { Some(slot) => slot.1 = v, None => e.push((k, v)) }
        }
        other => { let n = assoc(other, k, v); *other = n; }
    }
}
/// `s = s + x` fast path for unshared strings
pub fn str_append(st: &mut Value, x: &Value) {
    if let Value::Str(cur) = st { let mut o = String::with_capacity(cur.len() + 8); o.push_str(cur); o.push_str(&to_str(x)); *st = s(&o); return; }
    let n = add(st, x); *st = n;
}

pub fn atom(v: Value) -> Value { Value::Atom(Rc::new(RefCell::new(v))) }

// ============================================================ sheets

pub fn sheet(cells: Vec<(&str, Cell)>) -> Value {
    Value::Sheet(Rc::new(RefCell::new(SheetV { cells: cells.into_iter().map(|(n, c)| (n.to_string(), c)).collect(), evaluating: vec![] })))
}
pub fn formula(f: impl Fn(&Value) -> Value + 'static) -> Cell { Cell::Formula(Rc::new(f)) }
fn set_cell(sh: &Rc<RefCell<SheetV>>, name: &str, c: Cell) {
    let mut s = sh.borrow_mut();
    match s.cells.iter_mut().find(|(n, _)| n == name) { Some(slot) => slot.1 = c, None => s.cells.push((name.to_string(), c)) }
}
pub fn cell(sheet: &Value, name: &str) -> Value {
    let Value::Sheet(sh) = sheet else { fail("not a sheet") };
    let f = {
        let s = sh.borrow();
        match s.cells.iter().find(|(n, _)| n == name) {
            None => return Value::Nil,
            Some((_, Cell::Val(v))) => return v.clone(),
            Some((_, Cell::Formula(f))) => f.clone(),
        }
    };
    if sh.borrow().evaluating.iter().any(|n| n == name) { fail(format!("#REF! circular dependency at cell {}", name)); }
    sh.borrow_mut().evaluating.push(name.to_string());
    let r = f(sheet);
    sh.borrow_mut().evaluating.pop();
    r
}
fn split_cell(c: &str) -> (usize, usize) {
    let letters: String = c.chars().take_while(|c| c.is_ascii_uppercase()).collect();
    let col = letters.chars().fold(0, |acc, ch| acc * 26 + (ch as usize - 'A' as usize + 1));
    (col, c[letters.len()..].parse().unwrap())
}
fn col_name(mut col: usize) -> String {
    let mut s = String::new();
    while col > 0 { let r = (col - 1) % 26; s.insert(0, (b'A' + r as u8) as char); col = (col - 1) / 26; }
    s
}
pub fn cell_range(sheet: &Value, from: &str, to: &str) -> Value {
    let (c1, r1) = split_cell(from);
    let (c2, r2) = split_cell(to);
    let (c1, c2) = (c1.min(c2), c1.max(c2));
    let (r1, r2) = (r1.min(r2), r1.max(r2));
    let mut rows = vec![];
    for r in r1..=r2 {
        let row: Vec<Value> = (c1..=c2).map(|c| cell(sheet, &format!("{}{}", col_name(c), r))).collect();
        rows.push(row);
    }
    if c1 == c2 { return vec(rows.into_iter().map(|mut r| r.remove(0)).collect()); }
    if r1 == r2 { return vec(rows.remove(0)); }
    vec(rows.into_iter().map(vec).collect())
}
fn sheet_cells_sorted(sh: &Rc<RefCell<SheetV>>) -> Vec<String> {
    let mut names: Vec<String> = sh.borrow().cells.iter().map(|(n, _)| n.clone()).collect();
    names.sort_by_key(|n| { let (c, r) = split_cell(n); (r, c) });
    names
}

// ============================================================ conversions & display

pub fn type_name(v: &Value) -> String {
    match v {
        Value::Nil => "Nil".into(), Value::Bool(_) => "Bool".into(), Value::Int(_) => "Int".into(), Value::Float(_) => "Float".into(),
        Value::Dec(_) => "Decimal".into(), Value::Qty(q) => format!("Quantity<{}>", unit_show(&q.unit)),
        Value::Str(_) => "String".into(), Value::Vec(_) => "Vec".into(), Value::Map(_) => "Map".into(), Value::Func(_) => "Fn".into(),
        Value::Atom(_) => "Atom".into(), Value::Sheet(_) => "Sheet".into(), Value::Obj(o) => o.borrow().class.to_string(),
        Value::Variant(x) => x.enum_name.to_string(),
    }
}
pub fn truthy(v: &Value) -> bool {
    match v { Value::Bool(b) => *b, Value::Nil => false, other => fail(format!("expected Bool but found {}", type_name(other))) }
}
pub fn as_int(v: &Value) -> i64 {
    match v { Value::Int(i) => *i, Value::Float(f) => *f as i64, Value::Dec(d) => d.round(0, 2).m as i64, other => fail(format!("expected Int but found {}", type_name(other))) }
}
pub fn as_f64(v: &Value) -> f64 {
    match v { Value::Int(i) => *i as f64, Value::Float(f) => *f, Value::Dec(d) => d.to_f64(), Value::Bool(b) => if *b { 1.0 } else { 0.0 }, Value::Nil => 0.0,
        other => fail(format!("expected a number but found {}", type_name(other))) }
}
fn fmt_float(f: f64) -> String {
    if f.is_nan() { return "NaN".into(); }
    if f.is_infinite() { return if f > 0.0 { "Infinity".into() } else { "-Infinity".into() }; }
    if f.fract() == 0.0 && f.abs() < 1e15 { return format!("{}.0", f as i64); }
    format!("{}", f)
}
fn quote(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() { match c { '"' => o.push_str("\\\""), '\\' => o.push_str("\\\\"), '\n' => o.push_str("\\n"), c => o.push(c) } }
    o.push('"');
    o
}
pub fn repr(v: &Value) -> String { if let Value::Str(s) = v { quote(s) } else { to_str(v) } }
pub fn to_str(v: &Value) -> String {
    match v {
        Value::Nil => "nil".into(),
        Value::Bool(b) => b.to_string(),
        Value::Int(i) => i.to_string(),
        Value::Float(f) => fmt_float(*f),
        Value::Dec(d) => d.show(),
        Value::Qty(q) if is_ord(v) => { let n = as_int(&q.amt); format!("{}{}", n, ordinal_suffix(n)) }
        Value::Qty(q) => format!("{} {}", to_str(&q.amt), unit_show(&q.unit)),
        Value::Str(s) => s.to_string(),
        Value::Vec(items) => format!("[{}]", items.iter().map(repr).collect::<Vec<_>>().join(", ")),
        Value::Map(m) => format!("{{{}}}", m.iter().map(|(k, v)| format!("{}: {}", repr(k), repr(v))).collect::<Vec<_>>().join(", ")),
        Value::Func(_) => "<fn>".into(),
        Value::Atom(a) => format!("atom({})", repr(&a.borrow())),
        Value::Variant(x) => {
            if x.vals.is_empty() { x.name.to_string() } else { format!("{}({})", x.name, x.vals.iter().map(repr).collect::<Vec<_>>().join(", ")) }
        }
        Value::Obj(o) => {
            let (cls, fields) = { let b = o.borrow(); (b.class, b.fields.iter().map(|(n, v)| (*n, v.clone())).collect::<Vec<_>>()) };
            if find_method(cls, "toString", None).is_some() { return to_str(&call(v, "toString", vec![])); }
            let ci = class(cls);
            if ci.data {
                let parts: Vec<String> = ci.fields.iter().filter_map(|f| fields.iter().find(|(n, _)| n == f).map(|(n, x)| format!("{}={}", n, repr(x)))).collect();
                format!("{}({})", cls, parts.join(", "))
            } else { format!("<{}>", cls) }
        }
        Value::Sheet(sh) => {
            let names = sheet_cells_sorted(sh);
            let parts: Vec<String> = names.iter().map(|n| format!("{}: {}", n, repr(&cell(v, n)))).collect();
            format!("Sheet{{{}}}", parts.join(", "))
        }
    }
}
pub fn concat(parts: Vec<Value>) -> Value { let mut o = String::new(); for p in &parts { o.push_str(&to_str(p)); } s(&o) }
pub fn println(v: &Value) -> Value { println!("{}", to_str(v)); Value::Nil }
pub fn print(v: &Value) -> Value { print!("{}", to_str(v)); Value::Nil }


// ============================================================ exact decimals

#[derive(Clone, Copy, Debug)]
pub struct Dec { pub m: i128, pub s: u32 }

const DEC_MAX_SCALE: u32 = 18;
fn p10(n: u32) -> Option<i128> { 10i128.checked_pow(n) }

impl Dec {
    pub fn parse(t: &str) -> Dec {
        let neg = t.starts_with('-');
        let t = t.trim_start_matches('-');
        let (ip, fp) = match t.split_once('.') { Some((a, b)) => (a, b), None => (t, "") };
        let m: i128 = format!("{}{}", ip, fp).parse().unwrap_or_else(|_| fail(format!("invalid decimal {}", t)));
        Dec { m: if neg { -m } else { m }, s: fp.len() as u32 }
    }
    pub fn from_i(i: i64) -> Dec { Dec { m: i as i128, s: 0 } }
    pub fn from_f(f: f64) -> Dec { Dec::parse(&format!("{}", f)) }
    fn up(self, s: u32) -> i128 {
        if s < self.s { return self.m / p10(self.s - s).unwrap(); }
        p10(s - self.s).and_then(|k| self.m.checked_mul(k)).unwrap_or_else(|| fail("decimal overflow"))
    }
    pub fn to_f64(self) -> f64 { self.m as f64 / 10f64.powi(self.s as i32) }
    fn norm(mut self, min_scale: u32) -> Dec { while self.s > min_scale && self.m % 10 == 0 { self.m /= 10; self.s -= 1; } self }
    pub fn add(self, o: Dec) -> Dec { let s = self.s.max(o.s); Dec { m: self.up(s).checked_add(o.up(s)).unwrap_or_else(|| fail("decimal overflow")), s } }
    pub fn sub(self, o: Dec) -> Dec { self.add(o.neg()) }
    pub fn neg(self) -> Dec { Dec { m: -self.m, s: self.s } }
    pub fn mul(self, o: Dec) -> Dec {
        let m = self.m.checked_mul(o.m).unwrap_or_else(|| fail("decimal overflow"));
        let r = Dec { m, s: self.s + o.s };
        if r.s > DEC_MAX_SCALE { r.round(DEC_MAX_SCALE as i64, 0) } else { r }
    }
    pub fn div(self, o: Dec) -> Dec {
        if o.m == 0 { fail("division by zero"); }
        let base = self.s.max(o.s);
        for extra in (0..=16u32).rev() {
            let t = base + extra;
            // num = self.m * 10^(t - self.s + o.s)
            let Some(num) = p10(t + o.s - self.s).and_then(|k| self.m.checked_mul(k)) else { continue };
            let mut q = num / o.m;
            let r = num % o.m;
            if r != 0 && (r.abs().checked_mul(2).map_or(true, |x| x >= o.m.abs())) { q += if (num < 0) == (o.m < 0) { 1 } else { -1 }; }
            return Dec { m: q, s: t }.norm(base);
        }
        fail("decimal overflow")
    }
    pub fn rem(self, o: Dec) -> Dec {
        if o.m == 0 { fail("division by zero"); }
        let s = self.s.max(o.s); Dec { m: self.up(s) % o.up(s), s }
    }
    pub fn cmp(self, o: Dec) -> std::cmp::Ordering { let s = self.s.max(o.s); self.up(s).cmp(&o.up(s)) }
    /// mode 0: half away from zero, 1: away from zero (ROUNDUP), 2: toward zero (ROUNDDOWN)
    pub fn round(self, d: i64, mode: u8) -> Dec {
        let d = d.max(0) as u32;
        if d >= self.s { return Dec { m: self.up(d), s: d }; }
        let k = p10(self.s - d).unwrap();
        let (q, r) = (self.m / k, self.m % k);
        let bump = match mode { 0 => r.abs() * 2 >= k, 1 => r != 0, _ => false };
        Dec { m: if bump { q + self.m.signum() } else { q }, s: d }
    }
    pub fn show(self) -> String {
        let neg = self.m < 0;
        let a = self.m.unsigned_abs();
        let k = 10u128.pow(self.s);
        let ip = a / k;
        let fp = a % k;
        let mut o = if neg { "-".to_string() } else { String::new() };
        o.push_str(&ip.to_string());
        if self.s > 0 { o.push('.'); o.push_str(&format!("{:0>w$}", fp, w = self.s as usize)); }
        o
    }
}

pub fn dec(t: &str) -> Value { Value::Dec(Dec::parse(t)) }
fn to_dec(v: &Value) -> Dec {
    match v { Value::Int(i) => Dec::from_i(*i), Value::Dec(d) => *d, Value::Float(f) => Dec::from_f(*f), o => fail(format!("expected a number but found {}", type_name(o))) }
}

// ============================================================ units of measure

const CONVERSIONS: &[(&str, &str, &str)] = &[
    ("W", "W", "1"), ("kW", "W", "1000"), ("MW", "W", "1000000"), ("GW", "W", "1000000000"),
    ("Wh", "Wh", "1"), ("kWh", "Wh", "1000"), ("MWh", "Wh", "1000000"),
    ("mm", "m", "0.001"), ("cm", "m", "0.01"), ("m", "m", "1"), ("km", "m", "1000"),
    ("mg", "g", "0.001"), ("g", "g", "1"), ("kg", "g", "1000"), ("t", "g", "1000000"),
    ("ms", "s", "0.001"), ("s", "s", "1"), ("min", "s", "60"), ("h", "s", "3600"), ("d", "s", "86400"),
    ("mL", "L", "0.001"), ("L", "L", "1"),
    ("B", "B", "1"), ("KB", "B", "1000"), ("MB", "B", "1000000"), ("GB", "B", "1000000000"),
];
type Unit = Vec<(String, i32)>;

pub fn qty(amt: Value, unit: &str) -> Value {
    if !matches!(amt, Value::Int(_) | Value::Dec(_) | Value::Float(_)) { fail(format!("unit {} needs a number, got {}", unit, type_name(&amt))); }
    Value::Qty(Rc::new(QtyV { amt, unit: vec![(unit.to_string(), 1)] }))
}
fn mk_qty(amt: Value, unit: Unit) -> Value { if unit.is_empty() { amt } else { Value::Qty(Rc::new(QtyV { amt, unit })) } }
fn unit_simple(u: &Unit) -> Option<&str> { if u.len() == 1 && u[0].1 == 1 { Some(&u[0].0) } else { None } }
fn unit_same(a: &Unit, b: &Unit) -> bool { let mut x = a.clone(); x.sort(); let mut y = b.clone(); y.sort(); x == y }
fn conv(u: &str) -> Option<(&'static str, Dec)> { CONVERSIONS.iter().find(|c| c.0 == u).map(|c| (c.1, Dec::parse(c.2))) }
/// factor that converts an amount in unit `from` into unit `to`
fn factor(from: &Unit, to: &Unit) -> Option<Dec> {
    if unit_same(from, to) { return Some(Dec::from_i(1)); }
    let (Some(a), Some(b)) = (unit_simple(from), unit_simple(to)) else { return None };
    let (Some((ba, fa)), Some((bb, fb))) = (conv(a), conv(b)) else { return None };
    if ba != bb { return None; }
    Some(fa.div(fb))
}
fn convert_amt(amt: &Value, f: Dec) -> Value {
    if f.m == 1 && f.s == 0 { return amt.clone(); }
    match amt { Value::Float(x) => Value::Float(x * f.to_f64()), _ => Value::Dec(to_dec(amt).mul(f).norm(0)) }
}
pub fn unit_show(u: &Unit) -> String {
    let num: Vec<String> = u.iter().filter(|(_, e)| *e > 0).map(|(n, e)| if *e == 1 { n.clone() } else { format!("{}^{}", n, e) }).collect();
    let den: Vec<String> = u.iter().filter(|(_, e)| *e < 0).map(|(n, e)| if *e == -1 { n.clone() } else { format!("{}^{}", n, -e) }).collect();
    let n = if num.is_empty() { "1".to_string() } else { num.join("·") };
    if den.is_empty() { n } else { format!("{}/{}", n, den.join("/")) }
}
fn unit_combine(a: &Unit, b: &Unit, sign: i32) -> Unit {
    let mut out = a.clone();
    for (n, e) in b { match out.iter_mut().find(|(m, _)| m == n) { Some(slot) => slot.1 += e * sign, None => out.push((n.clone(), e * sign)) } }
    out.retain(|(_, e)| *e != 0);
    out
}
fn qty_align(a: &QtyV, b: &QtyV, op: &str) -> Value {
    match factor(&b.unit, &a.unit) {
        Some(f) => convert_amt(&b.amt, f),
        None => fail(format!("cannot {} {} and {}: incompatible units", op, unit_show(&a.unit), unit_show(&b.unit))),
    }
}
pub fn convert_to(v: &Value, unit: &str) -> Value {
    let Value::Qty(q) = v else { fail(format!("{} has no unit to convert", type_name(v))) };
    let target = vec![(unit.to_string(), 1)];
    match factor(&q.unit, &target) {
        Some(f) => mk_qty(convert_amt(&q.amt, f), target),
        None => fail(format!("cannot convert {} to {}", unit_show(&q.unit), unit)),
    }
}

// ============================================================ operators

pub fn eq(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Nil, Value::Nil) => true,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Int(x), Value::Int(y)) => x == y,
        (Value::Float(_), Value::Int(_) | Value::Float(_) | Value::Dec(_)) | (Value::Int(_) | Value::Dec(_), Value::Float(_)) => as_f64(a) == as_f64(b),
        (Value::Int(_) | Value::Dec(_), Value::Int(_) | Value::Dec(_)) => to_dec(a).cmp(to_dec(b)).is_eq(),
        (Value::Qty(x), Value::Qty(y)) => factor(&y.unit, &x.unit).map_or(false, |f| eq(&x.amt, &convert_amt(&y.amt, f))),
        (Value::Str(x), Value::Str(y)) => x == y,
        (Value::Vec(x), Value::Vec(y)) => x.len() == y.len() && x.iter().zip(y.iter()).all(|(p, q)| eq(p, q)),
        (Value::Map(x), Value::Map(y)) => x.len() == y.len() && x.iter().all(|(k, v)| y.iter().any(|(k2, v2)| eq(k, k2) && eq(v, v2))),
        (Value::Variant(x), Value::Variant(y)) => x.name == y.name && x.enum_name == y.enum_name && x.vals.len() == y.vals.len() && x.vals.iter().zip(y.vals.iter()).all(|(p, q)| eq(p, q)),
        (Value::Obj(x), Value::Obj(y)) => {
            if Rc::ptr_eq(x, y) { return true; }
            let (xb, yb) = (x.borrow(), y.borrow());
            if xb.class != yb.class || !class(xb.class).data { return false; }
            xb.fields.iter().all(|(n, v)| yb.fields.iter().any(|(n2, v2)| n == n2 && eq(v, v2)))
        }
        (Value::Func(x), Value::Func(y)) => Rc::ptr_eq(x, y),
        (Value::Atom(x), Value::Atom(y)) => Rc::ptr_eq(x, y),
        (Value::Sheet(x), Value::Sheet(y)) => Rc::ptr_eq(x, y),
        _ => false,
    }
}
pub fn op_eq(a: &Value, b: &Value) -> Value { Value::Bool(eq(a, b)) }
pub fn op_ne(a: &Value, b: &Value) -> Value { Value::Bool(!eq(a, b)) }

fn num_op(a: &Value, b: &Value, op: &str, fi: fn(i64, i64) -> Option<i64>, fd: fn(Dec, Dec) -> Dec, ff: fn(f64, f64) -> f64) -> Value {
    match (a, b) {
        (Value::Int(x), Value::Int(y)) => match fi(*x, *y) { Some(r) => Value::Int(r), None => fail(format!("integer overflow in {} {} {}", x, op, y)) },
        (Value::Float(_), Value::Int(_) | Value::Float(_) | Value::Dec(_)) | (Value::Int(_) | Value::Dec(_), Value::Float(_)) => Value::Float(ff(as_f64(a), as_f64(b))),
        (Value::Int(_) | Value::Dec(_), Value::Int(_) | Value::Dec(_)) => Value::Dec(fd(to_dec(a), to_dec(b))),
        _ => fail(format!("operator '{}' cannot be applied to {} and {}", op, type_name(a), type_name(b))),
    }
}
fn plain_num(v: &Value) -> bool { matches!(v, Value::Int(_) | Value::Dec(_) | Value::Float(_)) }
fn is_ord(v: &Value) -> bool { matches!(v, Value::Qty(q) if q.unit.len() == 1 && q.unit[0].0 == "ord") }
pub fn ordinal_suffix(n: i64) -> &'static str {
    let n = n.abs();
    if (11..=13).contains(&(n % 100)) { return "th"; }
    match n % 10 { 1 => "st", 2 => "nd", 3 => "rd", _ => "th" }
}
/// ordinals (positions) form an affine space over cardinals (counts)
fn ord_op(a: &Value, b: &Value, op: &str) -> Option<Value> {
    if !is_ord(a) && !is_ord(b) { return None; }
    let amt = |v: &Value| if let Value::Qty(q) = v { q.amt.clone() } else { v.clone() };
    Some(match (op, is_ord(a), is_ord(b)) {
        ("+", true, false) | ("-", true, false) if matches!(b, Value::Int(_)) => mk_qty(arith(&amt(a), b, op), vec![("ord".into(), 1)]),
        ("+", false, true) if matches!(a, Value::Int(_)) => mk_qty(add(a, &amt(b)), vec![("ord".into(), 1)]),
        ("-", true, true) => sub(&amt(a), &amt(b)),
        ("+", true, true) => fail("cannot add two ordinals (positions)"),
        ("+" | "-", _, _) => fail(format!("ordinals only move by whole cardinal numbers, not by {}", type_name(if is_ord(a) { b } else { a }))),
        _ => fail(format!("operator '{}' is not defined for ordinals", op)),
    })
}
fn qty_op(a: &Value, b: &Value, op: &str) -> Option<Value> {
    if let Some(r) = ord_op(a, b, op) { return Some(r); }
    let verb = match op { "-" => "subtract", "*" => "multiply", "/" => "divide", _ => "add" };
    match (a, b) {
        (Value::Qty(x), Value::Qty(y)) => Some(match op {
            "+" | "-" | "%" => { let ya = qty_align(x, y, verb); mk_qty(arith(&x.amt, &ya, op), x.unit.clone()) }
            "*" => {
                match factor(&y.unit, &x.unit) {
                    Some(f) if !unit_same(&x.unit, &y.unit) => { let ya = convert_amt(&y.amt, f); mk_qty(arith(&x.amt, &ya, "*"), unit_combine(&x.unit, &x.unit, 1)) }
                    _ => mk_qty(arith(&x.amt, &y.amt, "*"), unit_combine(&x.unit, &y.unit, 1)),
                }
            }
            "/" => match factor(&y.unit, &x.unit) {
                Some(f) => { let ya = convert_amt(&y.amt, f); arith(&x.amt, &ya, "/") }
                None => mk_qty(arith(&x.amt, &y.amt, "/"), unit_combine(&x.unit, &y.unit, -1)),
            },
            _ => fail(format!("operator '{}' cannot be applied to quantities", op)),
        }),
        (Value::Qty(x), n) if plain_num(n) => Some(match op {
            "*" | "/" => mk_qty(arith(&x.amt, n, op), x.unit.clone()),
            _ => fail(format!("cannot {} a plain number and {}; give the number a unit", verb, to_str(a))),
        }),
        (n, Value::Qty(y)) if plain_num(n) => Some(match op {
            "*" => mk_qty(arith(n, &y.amt, "*"), y.unit.clone()),
            "/" => mk_qty(arith(n, &y.amt, "/"), unit_combine(&vec![], &y.unit, -1)),
            _ => fail(format!("cannot {} a plain number and {}; give the number a unit", verb, to_str(b))),
        }),
        _ => None,
    }
}
fn arith(a: &Value, b: &Value, op: &str) -> Value {
    match op { "+" => add(a, b), "-" => sub(a, b), "*" => mul(a, b), "/" => div(a, b), _ => rem(a, b) }
}
pub fn add(a: &Value, b: &Value) -> Value {
    if let Some(r) = qty_op(a, b, "+") { return r; }
    match (a, b) {
        (Value::Str(_), _) | (_, Value::Str(_)) => s(&(to_str(a) + &to_str(b))),
        (Value::Vec(x), Value::Vec(y)) => { let mut v = (**x).clone(); v.extend(y.iter().cloned()); vec(v) }
        _ => num_op(a, b, "+", |x, y| x.checked_add(y), Dec::add, |x, y| x + y),
    }
}
pub fn sub(a: &Value, b: &Value) -> Value { if let Some(r) = qty_op(a, b, "-") { return r; } num_op(a, b, "-", |x, y| x.checked_sub(y), Dec::sub, |x, y| x - y) }
pub fn mul(a: &Value, b: &Value) -> Value { if let Some(r) = qty_op(a, b, "*") { return r; } num_op(a, b, "*", |x, y| x.checked_mul(y), Dec::mul, |x, y| x * y) }
pub fn div(a: &Value, b: &Value) -> Value {
    if let Some(r) = qty_op(a, b, "/") { return r; }
    if let (Value::Int(_), Value::Int(0)) = (a, b) { fail("division by zero"); }
    num_op(a, b, "/", |x, y| x.checked_div(y), Dec::div, |x, y| x / y)
}
pub fn rem(a: &Value, b: &Value) -> Value {
    if let Some(r) = qty_op(a, b, "%") { return r; }
    if let (Value::Int(_), Value::Int(0)) = (a, b) { fail("division by zero"); }
    num_op(a, b, "%", |x, y| x.checked_rem(y), Dec::rem, |x, y| x % y)
}
pub fn pow(a: &Value, b: &Value) -> Value {
    match (a, b) {
        (Value::Int(x), Value::Int(y)) if *y >= 0 => x.checked_pow(*y as u32).map(Value::Int).unwrap_or_else(|| fail("integer overflow in ^")),
        (Value::Dec(x), Value::Int(y)) if *y >= 0 => { let mut r = Dec::from_i(1); for _ in 0..*y { r = r.mul(*x); } Value::Dec(r) }
        (Value::Qty(q), Value::Int(y)) => mk_qty(pow(&q.amt, b), q.unit.iter().map(|(n, e)| (n.clone(), e * *y as i32)).collect()),
        _ => Value::Float(as_f64(a).powf(as_f64(b))),
    }
}
pub fn neg(a: &Value) -> Value {
    match a {
        Value::Int(i) => Value::Int(-i), Value::Float(f) => Value::Float(-f), Value::Dec(d) => Value::Dec(d.neg()),
        Value::Qty(q) => mk_qty(neg(&q.amt), q.unit.clone()),
        o => fail(format!("cannot negate {}", type_name(o))),
    }
}
pub fn not(a: &Value) -> Value { Value::Bool(!truthy(a)) }
pub fn amp(a: &Value, b: &Value) -> Value { s(&(to_str(a) + &to_str(b))) }
fn cmp(a: &Value, b: &Value) -> std::cmp::Ordering {
    match (a, b) {
        (Value::Int(x), Value::Int(y)) => x.cmp(y),
        (Value::Float(_), Value::Int(_) | Value::Float(_) | Value::Dec(_)) | (Value::Int(_) | Value::Dec(_), Value::Float(_)) => as_f64(a).partial_cmp(&as_f64(b)).unwrap_or(std::cmp::Ordering::Equal),
        (Value::Int(_) | Value::Dec(_), Value::Int(_) | Value::Dec(_)) => to_dec(a).cmp(to_dec(b)),
        (Value::Qty(x), Value::Qty(y)) => { let ya = qty_align(x, y, "compare"); cmp(&x.amt, &ya) }
        (Value::Str(x), Value::Str(y)) => x.cmp(y),
        (Value::Bool(x), Value::Bool(y)) => x.cmp(y),
        _ => fail(format!("cannot compare {} with {}", type_name(a), type_name(b))),
    }
}
pub fn lt(a: &Value, b: &Value) -> Value { Value::Bool(cmp(a, b).is_lt()) }
pub fn le(a: &Value, b: &Value) -> Value { Value::Bool(cmp(a, b).is_le()) }
pub fn gt(a: &Value, b: &Value) -> Value { Value::Bool(cmp(a, b).is_gt()) }
pub fn ge(a: &Value, b: &Value) -> Value { Value::Bool(cmp(a, b).is_ge()) }

pub fn not_null(v: Value, what: &str) -> Value { if v.is_nil() { fail(format!("null pointer: {} is nil", what)) } else { v } }
/// `expr?` — Rust-style early return: Ok(v) => v, Err/nil => return it
pub fn try_branch(v: Value) -> Result<Value, Value> {
    match &v {
        Value::Variant(x) if x.enum_name == "Result" => if x.name == "Ok" { Ok(x.vals[0].clone()) } else { Err(v) },
        Value::Nil => Err(v),
        _ => Ok(v),
    }
}
pub fn range(a: &Value, b: &Value) -> Value { vec((as_int(a)..as_int(b)).map(Value::Int).collect()) }
pub fn iter(v: &Value) -> Vec<Value> {
    match v {
        Value::Vec(items) => (**items).clone(),
        Value::Map(m) => m.iter().map(|(k, v)| vec(vec![k.clone(), v.clone()])).collect(),
        Value::Str(st) => st.chars().map(|c| s(&c.to_string())).collect(),
        Value::Nil => vec![],
        other => fail(format!("{} is not iterable", type_name(other))),
    }
}
pub fn assert(c: &Value, msg: &Value) -> Value { if !truthy(c) { fail(format!("assertion failed: {}", to_str(msg))) } Value::Nil }

// ============================================================ builtin methods

fn arg(args: &[Value], i: usize) -> Value { args.get(i).cloned().unwrap_or(Value::Nil) }

pub fn builtin_method(recv: &Value, name: &str, args: Vec<Value>) -> Value {
    let a0 = || arg(&args, 0);
    let a1 = || arg(&args, 1);
    match (recv, name) {
        (_, "toString") => return s(&to_str(recv)),
        (_, "equals") => return Value::Bool(eq(recv, &a0())),
        (_, "isNil") => return Value::Bool(recv.is_nil()),
        _ => {}
    }
    match recv {
        Value::Str(st) => match name {
            "length" | "count" => Value::Int(st.chars().count() as i64),
            "code" => Value::Int(st.chars().next().map_or(0, |c| c as i64)),
            "upper" | "uppercase" => s(&st.to_uppercase()),
            "lower" | "lowercase" => s(&st.to_lowercase()),
            "trim" => s(st.trim()),
            "contains" => Value::Bool(st.contains(&*to_str(&a0()))),
            "startsWith" => Value::Bool(st.starts_with(&*to_str(&a0()))),
            "endsWith" => Value::Bool(st.ends_with(&*to_str(&a0()))),
            "split" => vec(st.split(&*to_str(&a0())).map(s).collect()),
            "chars" => iter(recv).into(),
            "repeat" => s(&st.repeat(as_int(&a0()).max(0) as usize)),
            "replace" => s(&st.replace(&*to_str(&a0()), &to_str(&a1()))),
            "isEmpty" => Value::Bool(st.is_empty()),
            "reverse" => s(&st.chars().rev().collect::<String>()),
            "substring" => { let c: Vec<char> = st.chars().collect(); let a = as_int(&a0()).max(0) as usize; let b = if args.len() > 1 { as_int(&a1()) as usize } else { c.len() }; s(&c[a.min(c.len())..b.min(c.len())].iter().collect::<String>()) }
            "toInt" => st.trim().parse::<i64>().map(Value::Int).unwrap_or(Value::Nil),
            "toFloat" => st.trim().parse::<f64>().map(Value::Float).unwrap_or(Value::Nil),
            "toDecimal" => if st.trim().parse::<f64>().is_ok() && !st.contains(['e', 'E']) { dec(st.trim()) } else { Value::Nil },
            _ => no_method(recv, name),
        },
        Value::Qty(q) => match name {
            "amount" | "cardinal" => q.amt.clone(),
            "index" if is_ord(recv) => sub(&q.amt, &Value::Int(1)), // zero-based index of a position
            "unit" => s(&unit_show(&q.unit)),
            "to" => convert_to(recv, &to_str(&a0())),
            "abs" => mk_qty(builtin_method(&q.amt, "abs", vec![]), q.unit.clone()),
            "round" => mk_qty(sheets("ROUND", vec![q.amt.clone(), if args.is_empty() { Value::Int(0) } else { a0() }]), q.unit.clone()),
            _ => no_method(recv, name),
        },
        Value::Int(_) | Value::Float(_) | Value::Dec(_) => match name {
            "withUnit" => qty(recv.clone(), &to_str(&a0())),
            "ordinal" => qty(Value::Int(as_int(recv)), "ord"),
            "toDecimal" => Value::Dec(to_dec(recv)),
            "toFloat" => Value::Float(as_f64(recv)),
            "toInt" => Value::Int(as_f64(recv).trunc() as i64),
            "abs" => match recv { Value::Int(i) => Value::Int(i.abs()), Value::Dec(d) => Value::Dec(if d.m < 0 { d.neg() } else { *d }), _ => Value::Float(as_f64(recv).abs()) },
            "pow" => pow(recv, &a0()),
            "sqrt" => Value::Float(as_f64(recv).sqrt()),
            "round" => sheets("ROUND", vec![recv.clone(), if args.is_empty() { Value::Int(0) } else { a0() }]),
            "max" => if cmp(recv, &a0()).is_ge() { recv.clone() } else { a0() },
            "min" => if cmp(recv, &a0()).is_le() { recv.clone() } else { a0() },
            _ => no_method(recv, name),
        },
        Value::Vec(items) => vec_method(recv, items, name, &args),
        Value::Map(m) => match name {
            "get" => index(recv, &a0()),
            "assoc" => assoc(recv, a0(), a1()),
            "dissoc" => { let k = a0(); Value::Map(Rc::new(m.iter().filter(|(kk, _)| !eq(kk, &k)).cloned().collect())) }
            "keys" => vec(m.iter().map(|(k, _)| k.clone()).collect()),
            "values" => vec(m.iter().map(|(_, v)| v.clone()).collect()),
            "entries" => vec(iter(recv)),
            "count" | "size" => Value::Int(m.len() as i64),
            "isEmpty" => Value::Bool(m.is_empty()),
            "containsKey" | "has" => { let k = a0(); Value::Bool(m.iter().any(|(kk, _)| eq(kk, &k))) }
            "merge" => merge(recv, &a0()),
            "update" => { let k = a0(); let cur = index(recv, &k); assoc(recv, k, apply(&a1(), vec![cur])) }
            _ => { // record-style field access through a function-valued field
                if let Some((_, f @ Value::Func(_))) = m.iter().find(|(k, _)| matches!(k, Value::Str(x) if &**x == name)) { return apply(f, args); }
                no_method(recv, name)
            }
        },
        Value::Atom(a) => match name {
            "deref" | "get" => a.borrow().clone(),
            "reset" => { *a.borrow_mut() = a0(); a0() }
            "swap" => { let cur = a.borrow().clone(); let mut fargs = vec![cur]; fargs.extend(args.iter().skip(1).cloned()); let nv = apply(&a0(), fargs); *a.borrow_mut() = nv.clone(); nv }
            _ => no_method(recv, name),
        },
        Value::Variant(x) if x.enum_name == "Result" => match name {
            "isOk" => Value::Bool(x.name == "Ok"),
            "isErr" => Value::Bool(x.name == "Err"),
            "unwrap" => if x.name == "Ok" { x.vals[0].clone() } else { fail(format!("called unwrap() on Err({})", repr(&x.vals[0]))) },
            "unwrapOr" => if x.name == "Ok" { x.vals[0].clone() } else { a0() },
            "map" => if x.name == "Ok" { ok(apply(&a0(), vec![x.vals[0].clone()])) } else { recv.clone() },
            _ => no_method(recv, name),
        },
        Value::Sheet(sh) => match name {
            "get" => cell(recv, &to_str(&a0())),
            "set" => { set_cell(sh, &to_str(&a0()), Cell::Val(a1())); recv.clone() }
            "cells" => vec(sheet_cells_sorted(sh).into_iter().map(|n| s(&n)).collect()),
            _ => cell(recv, name),
        },
        Value::Func(_) => match name { "call" | "invoke" => apply(recv, args), _ => no_method(recv, name) },
        Value::Nil => fail(format!("cannot call {}() on nil", name)),
        _ => no_method(recv, name),
    }
}

impl From<Vec<Value>> for Value { fn from(v: Vec<Value>) -> Value { vec(v) } }

fn no_method(recv: &Value, name: &str) -> Value { fail(format!("{} has no method {}", type_name(recv), name)) }

fn vec_method(recv: &Value, items: &Rc<Vec<Value>>, name: &str, args: &[Value]) -> Value {
    let a0 = || arg(args, 0);
    let a1 = || arg(args, 1);
    match name {
        "count" | "size" | "length" => Value::Int(items.len() as i64),
        "isEmpty" => Value::Bool(items.is_empty()),
        "get" => index(recv, &a0()),
        "first" => items.first().cloned().unwrap_or(Value::Nil),
        "last" => items.last().cloned().unwrap_or(Value::Nil),
        "conj" | "push" | "plus" => { let mut v = (**items).clone(); v.push(a0()); vec(v) }
        "assoc" => assoc(recv, a0(), a1()),
        "concat" => add(recv, &a0()),
        "map" => { let f = a0(); vec(items.iter().map(|x| apply(&f, vec![x.clone()])).collect()) }
        "mapIndexed" => { let f = a0(); vec(items.iter().enumerate().map(|(i, x)| apply(&f, vec![Value::Int(i as i64), x.clone()])).collect()) }
        "filter" => { let f = a0(); vec(items.iter().filter(|x| truthy(&apply(&f, vec![(*x).clone()]))).cloned().collect()) }
        "remove" => { let f = a0(); vec(items.iter().filter(|x| !truthy(&apply(&f, vec![(*x).clone()]))).cloned().collect()) }
        "reduce" | "fold" => { let f = a1(); items.iter().fold(a0(), |acc, x| apply(&f, vec![acc, x.clone()])) }
        "each" | "forEach" => { let f = a0(); for x in items.iter() { apply(&f, vec![x.clone()]); } Value::Nil }
        "any" => { let f = a0(); Value::Bool(items.iter().any(|x| truthy(&apply(&f, vec![x.clone()])))) }
        "all" => { let f = a0(); Value::Bool(items.iter().all(|x| truthy(&apply(&f, vec![x.clone()])))) }
        "find" => { let f = a0(); items.iter().find(|x| truthy(&apply(&f, vec![(*x).clone()]))).cloned().unwrap_or(Value::Nil) }
        "sum" => sheets("SUM", vec![recv.clone()]),
        "min" => items.iter().cloned().reduce(|a, b| if cmp(&b, &a).is_lt() { b } else { a }).unwrap_or(Value::Nil),
        "max" => items.iter().cloned().reduce(|a, b| if cmp(&b, &a).is_gt() { b } else { a }).unwrap_or(Value::Nil),
        "join" => s(&items.iter().map(to_str).collect::<Vec<_>>().join(&if args.is_empty() { String::new() } else { to_str(&a0()) })),
        "contains" => { let x = a0(); Value::Bool(items.iter().any(|y| eq(y, &x))) }
        "indexOf" => { let x = a0(); Value::Int(items.iter().position(|y| eq(y, &x)).map_or(-1, |p| p as i64)) }
        "reverse" | "reversed" => vec(items.iter().rev().cloned().collect()),
        "sort" | "sorted" => { let mut v = (**items).clone(); v.sort_by(cmp); vec(v) }
        "sortBy" => { let f = a0(); let mut v: Vec<(Value, Value)> = items.iter().map(|x| (apply(&f, vec![x.clone()]), x.clone())).collect(); v.sort_by(|a, b| cmp(&a.0, &b.0)); vec(v.into_iter().map(|p| p.1).collect()) }
        "take" => vec(items.iter().take(as_int(&a0()).max(0) as usize).cloned().collect()),
        "drop" => vec(items.iter().skip(as_int(&a0()).max(0) as usize).cloned().collect()),
        "distinct" => { let mut v: Vec<Value> = vec![]; for x in items.iter() { if !v.iter().any(|y| eq(y, x)) { v.push(x.clone()); } } vec(v) }
        "flatten" => vec(items.iter().flat_map(|x| if let Value::Vec(i) = x { (**i).clone() } else { vec![x.clone()] }).collect()),
        "zip" => { let o = iter(&a0()); vec(items.iter().zip(o).map(|(a, b)| vec(vec![a.clone(), b])).collect()) }
        _ => no_method(recv, name),
    }
}

// ============================================================ Google Sheets functions

fn flat(args: &[Value]) -> Vec<Value> {
    let mut out = vec![];
    for a in args { match a { Value::Vec(v) => out.extend(flat(v)), x => out.push(x.clone()) } }
    out
}
fn nums(args: &[Value]) -> Vec<Value> { flat(args).into_iter().filter(|v| matches!(v, Value::Int(_) | Value::Float(_) | Value::Dec(_) | Value::Qty(_))).collect() }
fn sum_vals(vs: &[Value]) -> Value { let mut it = vs.iter(); match it.next() { None => Value::Int(0), Some(f) => it.fold(f.clone(), |a, b| add(&a, b)) } }
fn criteria(c: &Value) -> Box<dyn Fn(&Value) -> bool> {
    if let Value::Str(st) = c {
        for op in [">=", "<=", "<>", ">", "<", "="] {
            if let Some(rest) = st.strip_prefix(op) {
                let r = rest.trim();
                let target = if !r.is_empty() && r.chars().all(|c| c.is_ascii_digit() || c == '.' || c == '-') && r.parse::<f64>().is_ok() { dec(r) } else { s(rest) };
                let op = op.to_string();
                return Box::new(move |v: &Value| {
                    let comparable = matches!((&target, v), (Value::Dec(_), Value::Int(_) | Value::Float(_) | Value::Dec(_)) | (Value::Str(_), Value::Str(_)));
                    if !comparable { return op == "<>"; }
                    let o = cmp(v, &target);
                    match op.as_str() { ">=" => o.is_ge(), "<=" => o.is_le(), "<>" => !o.is_eq(), ">" => o.is_gt(), "<" => o.is_lt(), _ => o.is_eq() }
                });
            }
        }
    }
    let t = c.clone();
    Box::new(move |v: &Value| eq(v, &t))
}
fn round_to(x: &Value, d: i64, mode: u8) -> Value {
    match x {
        Value::Int(_) if d >= 0 => x.clone(),
        Value::Float(f) => { let m = 10f64.powi(d as i32); let y = f * m; Value::Float(match mode { 0 => y.round(), 1 => y.abs().ceil() * y.signum(), _ => y.trunc() } / m) }
        Value::Qty(q) => mk_qty(round_to(&q.amt, d, mode), q.unit.clone()),
        _ => Value::Dec(to_dec(x).round(d, mode)),
    }
}

fn avg(n: &[Value]) -> Value {
    let total = sum_vals(n);
    let total = if let Value::Int(i) = total { Value::Dec(Dec::from_i(i)) } else { total };
    div(&total, &Value::Int(n.len() as i64))
}

pub fn sheets(name: &str, args: Vec<Value>) -> Value {
    let a = |i: usize| arg(&args, i);
    let na = || s("#N/A");
    match name {
        "SUM" => sum_vals(&nums(&args)),
        "PRODUCT" => nums(&args).iter().fold(Value::Int(1), |a, b| mul(&a, b)),
        "AVERAGE" => { let n = nums(&args); if n.is_empty() { err(s("#DIV/0!")) } else { avg(&n) } }
        "MEDIAN" => { let mut n = nums(&args); if n.is_empty() { return err(na()); } n.sort_by(cmp); let l = n.len(); if l % 2 == 1 { n[l / 2].clone() } else { avg(&n[l / 2 - 1..l / 2 + 1]) } }
        "MIN" => nums(&args).into_iter().reduce(|a, b| if cmp(&b, &a).is_lt() { b } else { a }).unwrap_or(Value::Int(0)),
        "MAX" => nums(&args).into_iter().reduce(|a, b| if cmp(&b, &a).is_gt() { b } else { a }).unwrap_or(Value::Int(0)),
        "COUNT" => Value::Int(nums(&args).len() as i64),
        "COUNTA" => Value::Int(flat(&args).iter().filter(|v| !v.is_nil() && !matches!(v, Value::Str(x) if x.is_empty())).count() as i64),
        "ROUND" => round_to(&a(0), if a(1).is_nil() { 0 } else { as_int(&a(1)) }, 0),
        "ROUNDUP" => round_to(&a(0), if a(1).is_nil() { 0 } else { as_int(&a(1)) }, 1),
        "ROUNDDOWN" => round_to(&a(0), if a(1).is_nil() { 0 } else { as_int(&a(1)) }, 2),
        "INT" => match a(0) { Value::Float(f) => Value::Int(f.floor() as i64), v => { let d = to_dec(&v); let t = d.round(0, 2); Value::Int((if d.m < 0 && t.cmp(d).is_ne() { t.m - 1 } else { t.m }) as i64) } },
        "ABS" => builtin_method(&a(0), "abs", vec![]),
        "SQRT" => Value::Float(as_f64(&a(0)).sqrt()),
        "UNIT" => qty(a(0), &to_str(&a(1))),
        "CONVERT" => convert_to(&a(0), &to_str(&a(1))),
        "POWER" => pow(&a(0), &a(1)),
        "MOD" => { let (x, y) = (a(0), a(1)); let r = rem(&x, &y); if !matches!(r, Value::Float(_)) && cmp(&r, &Value::Int(0)).is_ne() && (cmp(&r, &Value::Int(0)).is_lt() != cmp(&y, &Value::Int(0)).is_lt()) { add(&r, &y) } else { r } }
        "LEN" => Value::Int(to_str(&a(0)).chars().count() as i64),
        "UPPER" => s(&to_str(&a(0)).to_uppercase()),
        "LOWER" => s(&to_str(&a(0)).to_lowercase()),
        "TRIM" => s(&to_str(&a(0)).split_whitespace().collect::<Vec<_>>().join(" ")),
        "LEFT" => { let n = if args.len() > 1 { as_int(&a(1)) } else { 1 }; s(&to_str(&a(0)).chars().take(n.max(0) as usize).collect::<String>()) }
        "RIGHT" => { let n = if args.len() > 1 { as_int(&a(1)) } else { 1 }; let c: Vec<char> = to_str(&a(0)).chars().collect(); s(&c[c.len().saturating_sub(n.max(0) as usize)..].iter().collect::<String>()) }
        "MID" => { let c: Vec<char> = to_str(&a(0)).chars().collect(); let st = (as_int(&a(1)) - 1).max(0) as usize; let n = as_int(&a(2)).max(0) as usize; s(&c.iter().skip(st).take(n).collect::<String>()) }
        "CONCATENATE" => concat(flat(&args)),
        "TEXTJOIN" => { let sep = to_str(&a(0)); let skip = truthy(&a(1)); let parts: Vec<String> = flat(&args[2.min(args.len())..]).iter().filter(|v| !(skip && (v.is_nil() || matches!(v, Value::Str(x) if x.is_empty())))).map(to_str).collect(); s(&parts.join(&sep)) }
        "SPLIT" => vec(to_str(&a(0)).split(&*to_str(&a(1))).filter(|x| !x.is_empty()).map(|x| x.parse::<i64>().map(Value::Int).unwrap_or_else(|_| if x.contains('.') && x.parse::<f64>().is_ok() { dec(x) } else { s(x) })).collect()),
        "SUBSTITUTE" => s(&to_str(&a(0)).replace(&*to_str(&a(1)), &to_str(&a(2)))),
        "SUMIF" | "COUNTIF" | "AVERAGEIF" => {
            let range = flat(&[a(0)]);
            let pred = criteria(&a(1));
            let vals = if args.len() > 2 { flat(&[a(2)]) } else { range.clone() };
            let picked: Vec<Value> = range.iter().zip(vals.iter()).filter(|(r, _)| pred(r)).map(|(_, v)| v.clone()).collect();
            match name {
                "COUNTIF" => Value::Int(picked.len() as i64),
                "SUMIF" => sum_vals(&nums(&picked)),
                _ => { let n = nums(&picked); if n.is_empty() { err(s("#DIV/0!")) } else { avg(&n) } }
            }
        }
        "FILTER" => {
            let items = iter(&a(0));
            let conds = iter(&a(1));
            let out: Vec<Value> = items.into_iter().zip(conds).filter(|(_, c)| truthy(c)).map(|(v, _)| v).collect();
            if out.is_empty() { err(na()) } else { vec(out) }
        }
        "SORT" => { let asc = args.len() < 2 || !matches!(a(args.len() - 1), Value::Bool(false)); let mut v = iter(&a(0)); v.sort_by(cmp); if !asc { v.reverse(); } vec(v) }
        "UNIQUE" => builtin_method(&a(0), "distinct", vec![]),
        "SEQUENCE" => {
            let rows = as_int(&a(0)); let cols = if args.len() > 1 { as_int(&a(1)) } else { 1 };
            let start = if args.len() > 2 { as_int(&a(2)) } else { 1 }; let step = if args.len() > 3 { as_int(&a(3)) } else { 1 };
            let mut k = start;
            let mut out = vec![];
            for _ in 0..rows { let mut row = vec![]; for _ in 0..cols { row.push(Value::Int(k)); k += step; } out.push(if cols == 1 { row.pop().unwrap() } else { vec(row) }); }
            vec(out)
        }
        "VLOOKUP" => {
            let key = a(0); let col = as_int(&a(2)) - 1;
            for row in iter(&a(1)) { let r = iter(&row); if r.first().map_or(false, |k| eq(k, &key)) { return r.get(col.max(0) as usize).cloned().unwrap_or_else(|| err(s("#REF!"))); } }
            err(na())
        }
        "INDEX" => { let row = index(&a(0), &Value::Int(as_int(&a(1)) - 1)); if args.len() > 2 { index(&row, &Value::Int(as_int(&a(2)) - 1)) } else { row } }
        "MATCH" => { let key = a(0); match iter(&a(1)).iter().position(|v| eq(v, &key)) { Some(p) => Value::Int(p as i64 + 1), None => err(na()) } }
        "ISBLANK" => Value::Bool(a(0).is_nil() || matches!(a(0), Value::Str(x) if x.is_empty())),
        "ISNUMBER" => Value::Bool(matches!(a(0), Value::Int(_) | Value::Float(_) | Value::Dec(_))),
        "ISTEXT" => Value::Bool(matches!(a(0), Value::Str(_))),
        "ISERROR" => Value::Bool(match a(0) { Value::Nil => true, Value::Variant(x) => x.enum_name == "Result" && x.name == "Err", _ => false }),
        "MAP" => { let f = a(1); vec(iter(&a(0)).into_iter().map(|x| apply(&f, vec![x])).collect()) }
        "REDUCE" => { let f = a(2); iter(&a(1)).into_iter().fold(a(0), |acc, x| apply(&f, vec![acc, x])) }
        "readFile" => match std::fs::read_to_string(to_str(&a(0))) { Ok(t) => ok(s(&t)), Err(e) => err(s(&e.to_string())) },
        "writeFile" => match std::fs::write(to_str(&a(0)), to_str(&a(1))) { Ok(_) => ok(Value::Nil), Err(e) => err(s(&e.to_string())) },
        "args" => vec(std::env::args().skip(1).map(|x| s(&x)).collect()),
        "exec" => {
            let argv: Vec<String> = iter(&a(1)).iter().map(to_str).collect();
            match std::process::Command::new(to_str(&a(0))).args(argv).status() { Ok(st) => Value::Int(st.code().unwrap_or(1) as i64), Err(_) => Value::Int(127) }
        }
        "mkdirs" => match std::fs::create_dir_all(to_str(&a(0))) { Ok(_) => ok(Value::Nil), Err(e) => err(s(&e.to_string())) },
        "exit" => std::process::exit(as_int(&a(0)) as i32),
        "eprintln" => { eprintln!("{}", to_str(&a(0))); Value::Nil }
        "__check" => { if !truthy(&a(0)) { fail(to_str(&a(1))); } Value::Nil }
        "__variant" => {
            let (prev, cur) = (a(0), a(1));
            let Value::Int(c) = cur else { fail(format!("{} must be an Int, found {}", to_str(&a(2)), type_name(&cur))) };
            if c < 0 { fail(format!("{} must stay non-negative (now {})", to_str(&a(2)), c)); }
            if let Value::Int(p) = prev { if c >= p { fail(format!("{} must decrease on every iteration (was {}, now {})", to_str(&a(2)), p, c)); } }
            cur
        }
        "TRUE" => Value::Bool(true),
        "FALSE" => Value::Bool(false),
        _ => fail(format!("unknown function {}", name)),
    }
}

#[allow(unused)]
fn _unused() { let mut s = String::new(); let _ = write!(s, ""); }
