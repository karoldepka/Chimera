//! Semantic analysis: class linearization (C3), override/abstract rules, name
//! resolution, null safety, record/utility types, contracts and match exhaustiveness.
//!
//! The checker is *gradual*: when a type cannot be inferred it becomes `Any` and is
//! not reported. Everything that is known statically is checked.

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::ast::*;
use crate::diag::{Diag, Span};
use crate::units;

// ====================================================================== types

#[derive(Debug, Clone, PartialEq)]
pub enum Ty {
    Any,
    Unit,
    Int,
    Float,
    Decimal,
    /// a number with a unit of measure
    Qty(units::Unit),
    Str,
    Bool,
    Nil,
    Nullable(Box<Ty>),
    Record(Vec<RField>),
    Class(String),
    Enum(String),
    Vec(Box<Ty>),
    Map(Box<Ty>, Box<Ty>),
    Fn(Vec<Ty>, Box<Ty>),
    Lit(String),
    Union(Vec<Ty>),
    Sheet,
    Atom,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RField {
    pub name: String,
    pub ty: Ty,
    pub optional: bool,
    pub readonly: bool,
}

impl Ty {
    pub fn show(&self) -> String {
        match self {
            Ty::Any => "Any".into(), Ty::Unit => "Unit".into(), Ty::Int => "Int".into(), Ty::Float => "Float".into(), Ty::Decimal => "Decimal".into(),
            Ty::Qty(u) => format!("Quantity<{}>", units::show(u)),
            Ty::Str => "String".into(), Ty::Bool => "Bool".into(), Ty::Nil => "Nil".into(),
            Ty::Nullable(t) => format!("{}?", t.show()),
            Ty::Record(fs) => format!("{{ {} }}", fs.iter().map(|f| format!("{}{}{}: {}",
                if f.readonly { "readonly " } else { "" }, f.name, if f.optional { "?" } else { "" }, f.ty.show())).collect::<Vec<_>>().join(", ")),
            Ty::Class(n) | Ty::Enum(n) => n.clone(),
            Ty::Vec(t) => format!("Vec<{}>", t.show()),
            Ty::Map(k, v) => format!("Map<{}, {}>", k.show(), v.show()),
            Ty::Fn(ps, r) => format!("({}) -> {}", ps.iter().map(|p| p.show()).collect::<Vec<_>>().join(", "), r.show()),
            Ty::Lit(s) => format!("\"{}\"", s),
            Ty::Union(ts) => ts.iter().map(|t| t.show()).collect::<Vec<_>>().join(" | "),
            Ty::Sheet => "Sheet".into(), Ty::Atom => "Atom".into(),
        }
    }
    fn nullable(self) -> Ty {
        match self { Ty::Nullable(_) | Ty::Any | Ty::Nil => self, t => Ty::Nullable(Box::new(t)) }
    }
    fn non_null(&self) -> Ty {
        match self { Ty::Nullable(t) => (**t).clone(), t => t.clone() }
    }
}

// ====================================================================== program info (shared with codegen)

#[derive(Debug, Clone)]
pub struct FieldInfo {
    pub name: String,
    pub mutable: bool,
    pub ty: Ty,
    pub owner: String,
}

#[derive(Debug, Clone)]
pub struct MethodSig {
    pub owner: String,
    pub params: Vec<(String, Ty, bool)>, // name, type, has default
    pub ret: Ty,
    pub is_abstract: bool,
}

#[derive(Debug, Clone)]
pub struct ClassInfo {
    pub name: String,
    pub is_trait: bool,
    pub is_data: bool,
    pub is_abstract: bool,
    pub mro: Vec<String>,
    pub fields: Vec<FieldInfo>,       // all fields, own + inherited
    pub own_fields: Vec<String>,      // in declaration order: ctor val/var params, then body fields
    pub own_methods: Vec<String>,
    pub methods: HashMap<String, MethodSig>, // resolved through the MRO
    pub has_invariants: bool,         // any class in the MRO declares invariants
    pub decl: usize,
}

#[derive(Debug, Clone)]
pub struct EnumInfo {
    pub name: String,
    pub variants: Vec<(String, Vec<String>)>,
}

#[derive(Debug, Clone)]
pub struct FunInfo {
    pub params: Vec<(String, Ty, bool)>,
    pub ret: Ty,
}

#[derive(Debug, Default)]
pub struct Info {
    pub classes: HashMap<String, ClassInfo>,
    pub class_order: Vec<String>,
    pub enums: HashMap<String, EnumInfo>,
    pub variant_of: HashMap<String, String>,
    pub funs: HashMap<String, FunInfo>,
    pub generics: HashMap<String, usize>,
    aliases: HashMap<String, (Vec<String>, TypeNode)>,
}

impl Info {
    pub fn is_class(&self, n: &str) -> bool { self.classes.contains_key(n) }
    pub fn variant_fields(&self, v: &str) -> Option<&Vec<String>> {
        let e = self.variant_of.get(v)?;
        self.enums[e].variants.iter().find(|(n, _)| n == v).map(|(_, f)| f)
    }
}

/// Global builtin functions (lowercase core + Google Sheets functions).
pub const BUILTINS: &[&str] = &[
    "println", "print", "str", "atom", "range", "Ok", "Err", "panic", "assert",
    // Google Sheets (eager)
    "SUM", "AVERAGE", "MIN", "MAX", "COUNT", "COUNTA", "PRODUCT", "MEDIAN", "ROUND", "ROUNDUP", "ROUNDDOWN", "ABS",
    "SQRT", "POWER", "MOD", "INT", "LEN", "UPPER", "LOWER", "TRIM", "LEFT", "RIGHT", "MID", "CONCATENATE", "TEXTJOIN",
    "SPLIT", "SUBSTITUTE", "SUMIF", "COUNTIF", "AVERAGEIF", "FILTER", "SORT", "UNIQUE", "SEQUENCE", "VLOOKUP", "INDEX",
    "MATCH", "ISBLANK", "ISNUMBER", "ISTEXT", "ISERROR", "MAP", "REDUCE", "TRUE", "FALSE", "UNIT", "CONVERT",
    // system (needed for self-hosting)
    "readFile", "writeFile", "args", "exit", "eprintln", "exec", "mkdirs", "embedFile",
    // contract helpers produced by the parser for `check` and loop invariants/variants
    "__check", "__variant",
];

pub fn is_builtin(n: &str) -> bool { BUILTINS.contains(&n) }

// ====================================================================== entry

pub fn check(p: &mut Program) -> Result<Info, Vec<Diag>> {
    let mut c = Checker { info: Info::default(), diags: vec![], ctor_params: HashMap::new(), ctor_types: HashMap::new() };
    c.collect(p);
    if c.diags.is_empty() { c.check_bodies(p); }
    if c.diags.is_empty() { Ok(c.info) } else { Err(c.diags) }
}

struct Checker {
    info: Info,
    diags: Vec<Diag>,
    ctor_params: HashMap<String, Vec<(String, bool)>>,
    ctor_types: HashMap<(String, String), Ty>,
}

#[derive(Clone)]
struct Local {
    ty: Ty,
    mutable: bool,
    lambda_depth: usize,
}

struct Ctx {
    scopes: Vec<HashMap<String, Local>>,
    class: Option<String>,
    in_generic_method: bool,
    in_ensure: bool,
    ret: Option<Ty>,
    lambda_depth: usize,
    loop_depth: usize,
}

impl Ctx {
    fn new(class: Option<String>) -> Ctx {
        Ctx { scopes: vec![HashMap::new()], class, in_generic_method: false, in_ensure: false, ret: None, lambda_depth: 0, loop_depth: 0 }
    }
    fn lookup(&self, n: &str) -> Option<&Local> { self.scopes.iter().rev().find_map(|s| s.get(n)) }
    fn declare(&mut self, n: &str, ty: Ty, mutable: bool) {
        let d = self.lambda_depth;
        self.scopes.last_mut().unwrap().insert(n.to_string(), Local { ty, mutable, lambda_depth: d });
    }
}

impl Checker {
    fn err(&mut self, msg: impl Into<String>, span: Span) { self.diags.push(Diag::new(msg, span)); }

    // ------------------------------------------------------------------ collection
    fn collect(&mut self, p: &Program) {
        // builtin Result enum (Rust-style)
        self.info.enums.insert("Result".into(), EnumInfo { name: "Result".into(), variants: vec![
            ("Ok".into(), vec!["value".into()]), ("Err".into(), vec!["error".into()])] });
        self.info.variant_of.insert("Ok".into(), "Result".into());
        self.info.variant_of.insert("Err".into(), "Result".into());

        let mut seen: HashMap<String, Span> = HashMap::new();
        let mut dup = |me: &mut Self, n: &str, s: Span| {
            if seen.contains_key(n) || is_builtin(n) && n != "Ok" && n != "Err" { me.err(format!("'{}' is already defined", n), s); }
            seen.insert(n.to_string(), s);
        };
        for d in &p.decls {
            match d {
                Decl::Enum(e) => {
                    dup(self, &e.name, e.span);
                    let mut vs = vec![];
                    for v in &e.variants {
                        dup(self, &v.name, v.span);
                        self.info.variant_of.insert(v.name.clone(), e.name.clone());
                        vs.push((v.name.clone(), v.fields.iter().map(|f| f.name.clone()).collect()));
                    }
                    self.info.enums.insert(e.name.clone(), EnumInfo { name: e.name.clone(), variants: vs });
                }
                Decl::TypeAlias(t) => { dup(self, &t.name, t.span); self.info.aliases.insert(t.name.clone(), (t.type_params.clone(), t.ty.clone())); }
                Decl::Class(c) => dup(self, &c.name, c.span),
                Decl::Fun(f) => dup(self, &f.name, f.span),
                Decl::Generic(g) => { dup(self, &g.name, g.span); self.info.generics.insert(g.name.clone(), g.params.len()); }
                Decl::Method(_) => {}
            }
        }
        // functions
        for d in &p.decls {
            if let Decl::Fun(f) = d {
                self.contract_rules(f);
                let sig = self.fun_sig(f);
                self.info.funs.insert(f.name.clone(), FunInfo { params: sig.0, ret: sig.1 });
            }
        }
        // validate every type alias (utility types report bad keys etc.)
        for d in &p.decls {
            if let Decl::TypeAlias(t) = d {
                let subst: HashMap<String, Ty> = t.type_params.iter().map(|n| (n.clone(), Ty::Any)).collect();
                let ty = t.ty.clone();
                self.resolve_in(&ty, &subst);
            }
        }
        // constructor signatures
        for d in &p.decls {
            if let Decl::Class(c) = d {
                self.ctor_params.insert(c.name.clone(), c.params.iter().map(|p| (p.name.clone(), p.default.is_some())).collect());
                for prm in &c.params {
                    if let Some(t) = &prm.ty { let rt = self.resolve(t); self.ctor_types.insert((c.name.clone(), prm.name.clone()), rt); }
                }
            }
        }
        // classes, in dependency order
        let decls: HashMap<String, (usize, &ClassDecl)> = p.decls.iter().enumerate()
            .filter_map(|(i, d)| if let Decl::Class(c) = d { Some((c.name.clone(), (i, c))) } else { None }).collect();
        let mut order = vec![];
        let mut state: HashMap<String, u8> = HashMap::new();
        let mut names: Vec<&String> = decls.keys().collect();
        names.sort_by_key(|n| decls[*n].0);
        for n in names { self.visit_class(n, &decls, &mut state, &mut order); }
        for n in order {
            let (i, c) = decls[&n];
            self.build_class(i, c);
            self.info.class_order.push(n);
        }
        // generic methods
        for d in &p.decls {
            if let Decl::Method(m) = d {
                match self.info.generics.get(&m.name) {
                    None => self.err(format!("method for unknown generic function '{}' (declare it with `generic {}(...)`)", m.name, m.name), m.span),
                    Some(&n) if n != m.params.len() => self.err(format!("generic '{}' takes {} parameters but this method has {}", m.name, n, m.params.len()), m.span),
                    _ => {}
                }
                for p in &m.params {
                    if let Some(TypeNode::Name { name, span, .. }) = &p.ty {
                        if !self.is_specializer(name) { self.err(format!("unknown specializer type '{}'", name), *span); }
                    }
                }
            }
        }
    }

    fn is_specializer(&self, n: &str) -> bool {
        self.info.classes.contains_key(n) || self.info.enums.contains_key(n) || self.info.variant_of.contains_key(n)
            || ["Any", "Int", "Float", "Decimal", "Quantity", "String", "Bool", "Nil", "Vec", "Map", "Fn", "Sheet", "Atom", "Number"].contains(&n)
    }

    fn visit_class(&mut self, n: &str, decls: &HashMap<String, (usize, &ClassDecl)>, state: &mut HashMap<String, u8>, order: &mut Vec<String>) {
        match state.get(n) { Some(2) => return, Some(1) => { let s = decls[n].1.span; self.err(format!("cyclic inheritance involving '{}'", n), s); return; } _ => {} }
        state.insert(n.into(), 1);
        for s in &decls[n].1.supers {
            if decls.contains_key(&s.name) { self.visit_class(&s.name, decls, state, order); }
            else { self.err(format!("unknown superclass or trait '{}'", s.name), s.span); }
        }
        state.insert(n.into(), 2);
        order.push(n.into());
    }

    fn build_class(&mut self, idx: usize, c: &ClassDecl) {
        if c.supers.iter().any(|s| !self.info.classes.contains_key(&s.name)) { return; }
        // C3 linearization
        let mut seqs: Vec<Vec<String>> = c.supers.iter().map(|s| self.info.classes[&s.name].mro.clone()).collect();
        seqs.push(c.supers.iter().map(|s| s.name.clone()).collect());
        let mut mro = vec![c.name.clone()];
        loop {
            seqs.retain(|s| !s.is_empty());
            if seqs.is_empty() { break; }
            let cand = seqs.iter().map(|s| s[0].clone()).find(|h| !seqs.iter().any(|s| s[1..].contains(h)));
            match cand {
                Some(h) => { for s in &mut seqs { if s[0] == h { s.remove(0); } } mro.push(h); }
                None => { self.err(format!("cannot compute a consistent method resolution order (C3) for '{}'", c.name), c.span); return; }
            }
        }
        // supers: traits take no args, classes with required ctor params need args
        let mut class_parents = 0;
        for s in &c.supers {
            let p = self.info.classes[&s.name].clone();
            if c.is_trait && !p.is_trait { self.err(format!("trait '{}' can only extend traits, but '{}' is a class", c.name, s.name), s.span); }
            if p.is_trait && s.args.is_some() { self.err(format!("trait '{}' has no constructor", s.name), s.span); }
            if !p.is_trait { class_parents += 1; }
        }
        let _ = class_parents; // multiple class inheritance is allowed
        // fields
        let mut fields: Vec<FieldInfo> = vec![];
        for anc in mro.iter().skip(1).rev() {
            for f in &self.info.classes[anc].fields {
                if f.owner == *anc && !fields.iter().any(|x| x.name == f.name) { fields.push(f.clone()); }
            }
        }
        let mut own_fields = vec![];
        let mut add_field = |me: &mut Self, name: &str, mutable: bool, ty: Ty, span: Span, fields: &mut Vec<FieldInfo>| {
            if let Some(f) = fields.iter().find(|f| f.name == name) {
                me.err(format!("field '{}' is already defined in '{}'", name, f.owner), span);
                return;
            }
            fields.push(FieldInfo { name: name.into(), mutable, ty, owner: c.name.clone() });
            own_fields.push(name.to_string());
        };
        for p in &c.params {
            if let Some(m) = p.field {
                let ty = p.ty.as_ref().map(|t| self.resolve(t)).unwrap_or(Ty::Any);
                add_field(self, &p.name, m, ty, p.span, &mut fields);
            }
        }
        for f in &c.fields {
            let ty = f.ty.as_ref().map(|t| self.resolve(t)).unwrap_or(Ty::Any);
            add_field(self, &f.name, f.mutable, ty, f.span, &mut fields);
        }
        // methods: resolve through MRO, check override rules & diamond ambiguity
        let mut methods: HashMap<String, MethodSig> = HashMap::new();
        let mut own_methods = vec![];
        for m in &c.methods {
            self.contract_rules(m);
            let (params, ret) = self.fun_sig(m);
            let inherited = mro.iter().skip(1).find(|a| self.info.classes[*a].own_methods.contains(&m.name) || self.info.classes[*a].methods.get(&m.name).map_or(false, |s| s.owner == **a));
            if inherited.is_some() && !m.is_override {
                self.err(format!("'{}' overrides a member of '{}' and needs the 'override' modifier", m.name, inherited.unwrap()), m.span);
            }
            if inherited.is_none() && m.is_override {
                self.err(format!("'{}' overrides nothing", m.name), m.span);
            }
            if fields.iter().any(|f| f.name == m.name) { self.err(format!("'{}' is both a field and a method", m.name), m.span); }
            if own_methods.contains(&m.name) { self.err(format!("method '{}' is defined twice", m.name), m.span); }
            own_methods.push(m.name.clone());
            methods.insert(m.name.clone(), MethodSig { owner: c.name.clone(), params, ret, is_abstract: m.body.is_none() });
        }
        // inherited methods: first concrete in MRO wins; two unrelated concrete providers = ambiguity (Kotlin rule)
        let mut names: Vec<String> = mro.iter().skip(1).flat_map(|a| self.info.classes[a].own_methods.clone()).collect();
        names.sort(); names.dedup();
        for n in names {
            if methods.contains_key(&n) { continue; }
            let providers: Vec<&String> = mro.iter().skip(1).filter(|a| {
                let ci = &self.info.classes[*a];
                ci.own_methods.contains(&n) && !ci.methods[&n].is_abstract
            }).collect();
            // drop providers that are ancestors of another provider (the more derived one overrides)
            let maximal: Vec<&String> = providers.iter().filter(|p| !providers.iter().any(|q| q != *p && self.info.classes[*q].mro.contains(p))).cloned().collect();
            if maximal.len() > 1 {
                self.err(format!("'{}' inherits conflicting implementations of '{}' from {}; override it (call `super<{}>.{}()` to pick one)",
                    c.name, n, maximal.iter().map(|s| format!("'{}'", s)).collect::<Vec<_>>().join(" and "), maximal[0], n), c.span);
                continue;
            }
            let sig = if let Some(p) = maximal.first() { self.info.classes[*p].methods[&n].clone() } else {
                let a = mro.iter().skip(1).find(|a| self.info.classes[*a].own_methods.contains(&n)).unwrap();
                self.info.classes[a].methods[&n].clone()
            };
            methods.insert(n, sig);
        }
        let is_abstract = c.is_abstract;
        if !is_abstract {
            let mut missing: Vec<&String> = methods.iter().filter(|(_, s)| s.is_abstract).map(|(n, _)| n).collect();
            missing.sort();
            if !missing.is_empty() {
                self.err(format!("class '{}' must implement abstract member(s): {}", c.name, missing.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")), c.span);
            }
        }
        let has_invariants = !c.invariants.is_empty() || mro.iter().skip(1).any(|a| self.info.classes[a].has_invariants);
        self.info.classes.insert(c.name.clone(), ClassInfo {
            name: c.name.clone(), is_trait: c.is_trait, is_data: c.is_data, is_abstract, mro, fields, own_fields, own_methods,
            methods, has_invariants, decl: idx,
        });
    }

    /// Eiffel/Liskov rules: an override may only weaken preconditions (`require else`)
    /// and only strengthen postconditions (`ensure then`).
    fn contract_rules(&mut self, f: &FunDecl) {
        if f.is_override {
            if !f.requires.is_empty() && !f.require_else { self.err(format!("'{}' overrides an inherited method, so it may only weaken the precondition: write `require else`", f.name), f.span); }
            if !f.ensures.is_empty() && !f.ensure_then { self.err(format!("'{}' overrides an inherited method, so it may only strengthen the postcondition: write `ensure then`", f.name), f.span); }
        } else {
            if f.require_else { self.err(format!("`require else` is only allowed when overriding, and '{}' overrides nothing", f.name), f.span); }
            if f.ensure_then { self.err(format!("`ensure then` is only allowed when overriding, and '{}' overrides nothing", f.name), f.span); }
        }
    }

    fn fun_sig(&mut self, f: &FunDecl) -> (Vec<(String, Ty, bool)>, Ty) {
        let ps = f.params.iter().map(|p| (p.name.clone(), p.ty.as_ref().map(|t| self.resolve(t)).unwrap_or(Ty::Any), p.default.is_some())).collect();
        let ret = f.ret.as_ref().map(|t| self.resolve(t)).unwrap_or(if f.body.as_ref().map_or(false, |b| matches!(b.kind, ExprKind::Block(_))) { Ty::Unit } else { Ty::Any });
        (ps, ret)
    }

    // ------------------------------------------------------------------ type resolution (incl. TS utility types)
    fn resolve(&mut self, t: &TypeNode) -> Ty { self.resolve_in(t, &HashMap::new()) }

    fn resolve_in(&mut self, t: &TypeNode, subst: &HashMap<String, Ty>) -> Ty {
        match t {
            TypeNode::Nullable(i) => self.resolve_in(i, subst).nullable(),
            TypeNode::StrLit(s) => Ty::Lit(s.clone()),
            TypeNode::Union(ts) => Ty::Union(ts.iter().map(|x| self.resolve_in(x, subst)).collect()),
            TypeNode::Fn(ps, r) => Ty::Fn(ps.iter().map(|x| self.resolve_in(x, subst)).collect(), Box::new(self.resolve_in(r, subst))),
            TypeNode::Record(fs) => Ty::Record(fs.iter().map(|f| RField { name: f.name.clone(), ty: self.resolve_in(&f.ty, subst), optional: f.optional, readonly: f.readonly }).collect()),
            TypeNode::KeyOf(i) => {
                let r = self.resolve_in(i, subst);
                match r { Ty::Record(fs) => Ty::Union(fs.iter().map(|f| Ty::Lit(f.name.clone())).collect()), _ => Ty::Str }
            }
            TypeNode::Name { name, args, span } => {
                if let Some(t) = subst.get(name) { return t.clone(); }
                let a: Vec<Ty> = args.iter().map(|x| self.resolve_in(x, subst)).collect();
                let rec = |me: &mut Self, t: &Ty| -> Option<Vec<RField>> {
                    match t { Ty::Record(fs) => Some(fs.clone()), Ty::Any => None, other => { me.err(format!("'{}' needs a record type, got {}", name, other.show()), *span); None } }
                };
                let keys = |t: &Ty| -> Vec<String> {
                    match t { Ty::Lit(s) => vec![s.clone()], Ty::Union(ts) => ts.iter().filter_map(|t| if let Ty::Lit(s) = t { Some(s.clone()) } else { None }).collect(), _ => vec![] }
                };
                let need = |me: &mut Self, n: usize| -> bool {
                    if a.len() != n { me.err(format!("'{}' expects {} type argument(s)", name, n), *span); false } else { true }
                };
                match name.as_str() {
                    "Int" => Ty::Int, "Float" => Ty::Float, "Decimal" => Ty::Decimal,
                    "Quantity" => match args.first() {
                        Some(TypeNode::Name { name: u, .. }) => Ty::Qty(vec![(u.clone(), 1)]),
                        Some(TypeNode::StrLit(u)) => Ty::Qty(vec![(u.clone(), 1)]),
                        _ => Ty::Any,
                    }, "String" => Ty::Str, "Bool" => Ty::Bool, "Any" => Ty::Any,
                    "Unit" => Ty::Unit, "Nil" => Ty::Nil, "Sheet" => Ty::Sheet, "Atom" => Ty::Atom, "Number" => Ty::Union(vec![Ty::Int, Ty::Decimal, Ty::Float]),
                    "Vec" | "List" => Ty::Vec(Box::new(a.first().cloned().unwrap_or(Ty::Any))),
                    "Map" => Ty::Map(Box::new(a.first().cloned().unwrap_or(Ty::Any)), Box::new(a.get(1).cloned().unwrap_or(Ty::Any))),
                    // ---- TypeScript utility types ----
                    "Partial" | "Required" | "Readonly" | "Mutable" => {
                        if !need(self, 1) { return Ty::Any; }
                        match rec(self, &a[0]) {
                            Some(fs) => Ty::Record(fs.into_iter().map(|mut f| {
                                match name.as_str() { "Partial" => f.optional = true, "Required" => { f.optional = false; f.ty = f.ty.non_null(); } "Readonly" => f.readonly = true, _ => f.readonly = false }
                                f
                            }).collect()),
                            None => Ty::Any,
                        }
                    }
                    "Pick" | "Omit" => {
                        if !need(self, 2) { return Ty::Any; }
                        let ks = keys(&a[1]);
                        match rec(self, &a[0]) {
                            Some(fs) => {
                                for k in &ks { if !fs.iter().any(|f| &f.name == k) { self.err(format!("key \"{}\" does not exist in {}", k, a[0].show()), *span); } }
                                Ty::Record(fs.into_iter().filter(|f| ks.contains(&f.name) == (name == "Pick")).collect())
                            }
                            None => Ty::Any,
                        }
                    }
                    "Record" => {
                        if !need(self, 2) { return Ty::Any; }
                        let ks = keys(&a[0]);
                        if ks.is_empty() { Ty::Map(Box::new(a[0].clone()), Box::new(a[1].clone())) }
                        else { Ty::Record(ks.into_iter().map(|k| RField { name: k, ty: a[1].clone(), optional: false, readonly: false }).collect()) }
                    }
                    "NonNullable" => { if !need(self, 1) { return Ty::Any; } a[0].non_null() }
                    "Exclude" | "Extract" => {
                        if !need(self, 2) { return Ty::Any; }
                        let drop: Vec<Ty> = match &a[1] { Ty::Union(ts) => ts.clone(), t => vec![t.clone()] };
                        let src: Vec<Ty> = match &a[0] { Ty::Union(ts) => ts.clone(), t => vec![t.clone()] };
                        let out: Vec<Ty> = src.into_iter().filter(|t| drop.contains(t) == (name == "Extract")).collect();
                        if out.len() == 1 { out[0].clone() } else { Ty::Union(out) }
                    }
                    "ReturnType" => { if !need(self, 1) { return Ty::Any; } if let Ty::Fn(_, r) = &a[0] { (**r).clone() } else { Ty::Any } }
                    "Result" => Ty::Enum("Result".into()),
                    _ => {
                        if self.info.classes.contains_key(name) { return Ty::Class(name.clone()); }
                        if self.info.enums.contains_key(name) { return Ty::Enum(name.clone()); }
                        if let Some((tps, node)) = self.info.aliases.get(name).cloned() {
                            if tps.len() != a.len() { self.err(format!("type '{}' expects {} type argument(s)", name, tps.len()), *span); return Ty::Any; }
                            let s: HashMap<String, Ty> = tps.into_iter().zip(a).collect();
                            return self.resolve_in(&node, &s);
                        }
                        if name.len() == 1 && name.chars().all(|c| c.is_ascii_uppercase()) { return Ty::Any; } // generic type parameter
                        // classes are collected after aliases; allow forward reference to classes declared anywhere
                        Ty::Class(name.clone())
                    }
                }
            }
        }
    }

    /// Is a value of type `src` usable where `dst` is expected? Only definite mismatches are reported.
    fn assignable(&self, src: &Ty, dst: &Ty) -> Result<(), String> {
        use Ty::*;
        match (src, dst) {
            (Any, _) | (_, Any) | (_, Unit) => Ok(()),
            (Nil, Nullable(_)) | (Nil, Nil) => Ok(()),
            (Nil, t) => Err(format!("nil cannot be assigned to non-nullable type {} (use {}?)", t.show(), t.show())),
            (Nullable(a), Nullable(b)) => self.assignable(a, b),
            (Nullable(a), b) => {
                if self.assignable(a, b).is_ok() { Err(format!("type {} is nullable but {} is not; use ?:, !! or a nil check", src.show(), b.show())) } else { self.assignable(a, b) }
            }
            (a, Nullable(b)) => self.assignable(a, b),
            (Int, Float) | (Int, Decimal) | (Decimal, Float) => Ok(()),
            (Float, Decimal) => Err("Float cannot be used where Decimal is expected (floats are inexact); use a decimal literal like 1.5 or .toDecimal()".into()),
            (Qty(a), Qty(b)) => if units::compatible(a, b) { Ok(()) } else { Err(format!("unit mismatch: expected {} but found {}", units::show(b), units::show(a))) },
            (Qty(a), Int | Float | Decimal) => Err(format!("a quantity in {} cannot be used as a plain number; use .amount", units::show(a))),
            (Int | Float | Decimal, Qty(b)) => Err(format!("a plain number cannot be used as a quantity in {}; write e.g. `5 {}`", units::show(b), units::show(b))),
            (Lit(_), Str) => Ok(()),
            (Lit(a), Lit(b)) if a == b => Ok(()),
            (Union(opts), b) => { for o in opts { self.assignable(o, b)?; } Ok(()) }
            (a, Union(opts)) => if opts.iter().any(|o| self.assignable(a, o).is_ok()) { Ok(()) } else { Err(format!("{} is not one of {}", a.show(), dst.show())) },
            (Class(a), Class(b)) => {
                if a == b || self.info.classes.get(a).map_or(true, |c| c.mro.contains(b)) { Ok(()) } else { Err(format!("{} is not a subtype of {}", a, b)) }
            }
            (Enum(a), Enum(b)) => if a == b { Ok(()) } else { Err(format!("{} is not {}", a, b)) },
            (Record(sf), Record(df)) => {
                for d in df {
                    match sf.iter().find(|s| s.name == d.name) {
                        None if !d.optional => return Err(format!("missing required field '{}'", d.name)),
                        Some(s) => {
                            if s.optional && !d.optional { return Err(format!("field '{}' is optional in the source but required in {}", d.name, dst.show())); }
                            self.assignable(&s.ty, &d.ty).map_err(|e| format!("field '{}': {}", d.name, e))?;
                        }
                        _ => {}
                    }
                }
                Ok(())
            }
            (Map(..), Record(_)) | (Record(_), Map(..)) => Ok(()),
            (Vec(a), Vec(b)) => self.assignable(a, b),
            (Map(_, a), Map(_, b)) => self.assignable(a, b),
            (Fn(..), Fn(..)) => Ok(()),
            (a, b) if a == b => Ok(()),
            (a, b) => Err(format!("expected {} but found {}", b.show(), a.show())),
        }
    }

    fn expect(&mut self, src: &Ty, dst: &Ty, span: Span) {
        if let Err(e) = self.assignable(src, dst) { self.err(format!("type mismatch: {}", e), span); }
    }

    // ------------------------------------------------------------------ bodies
    fn check_bodies(&mut self, p: &mut Program) {
        for d in &p.decls {
            match d {
                Decl::Fun(f) => { let mut cx = Ctx::new(None); self.check_fun(f, &mut cx); }
                Decl::Class(c) => self.check_class(c),
                Decl::Method(m) => {
                    let mut cx = Ctx::new(None);
                    cx.in_generic_method = true;
                    for p in &m.params {
                        let t = match &p.ty { Some(TypeNode::Name { name, .. }) if self.info.classes.contains_key(name) => Ty::Class(name.clone()), _ => Ty::Any };
                        cx.declare(&p.name, t, false);
                    }
                    self.expr(&m.body, &mut cx, None);
                }
                _ => {}
            }
        }
        if !p.decls.iter().any(|d| matches!(d, Decl::Fun(f) if f.name == "main")) {
            self.err("program has no `fun main()`", Span::new(0, 0));
        }
    }

    fn check_class(&mut self, c: &ClassDecl) {
        let mut cx = Ctx::new(Some(c.name.clone()));
        for p in &c.params {
            if let Some(d) = &p.default { self.expr(d, &mut cx, None); }
            let t = p.ty.as_ref().map(|t| self.resolve(t)).unwrap_or(Ty::Any);
            cx.declare(&p.name, t, false);
        }
        for s in &c.supers {
            let parent = self.info.classes[&s.name].clone();
            if parent.is_trait { continue; }
            let pdecl_params: Vec<(String, bool)> = self.class_params(&s.name);
            let args = s.args.clone().unwrap_or_default();
            self.check_args(&format!("constructor of '{}'", s.name), &pdecl_params.iter().map(|(n, d)| (n.clone(), Ty::Any, *d)).collect::<Vec<_>>(), &args, s.span, &mut cx);
        }
        for f in &c.fields {
            let t = self.expr(&f.init, &mut cx, None);
            if let Some(tn) = &f.ty { let d = self.resolve(tn); self.expect(&t, &d, f.init.span); }
        }
        for inv in &c.invariants { let t = self.expr(&inv.expr, &mut cx, None); self.expect(&t, &Ty::Bool, inv.expr.span); }
        for m in &c.methods {
            let mut mcx = Ctx::new(Some(c.name.clone()));
            self.check_fun(m, &mut mcx);
        }
    }

    pub fn class_params(&self, _name: &str) -> Vec<(String, bool)> {
        // filled in by codegen-independent lookup: ClassInfo does not keep params, so look them up in the
        // stored constructor table
        self.ctor_params.get(_name).cloned().unwrap_or_default()
    }

    fn check_fun(&mut self, f: &FunDecl, cx: &mut Ctx) {
        for p in &f.params {
            if let Some(d) = &p.default { self.expr(d, cx, None); }
            let t = p.ty.as_ref().map(|t| self.resolve(t)).unwrap_or(Ty::Any);
            cx.declare(&p.name, t, true);
        }
        for r in &f.requires { let t = self.expr(&r.expr, cx, None); self.expect(&t, &Ty::Bool, r.expr.span); }
        let ret = f.ret.as_ref().map(|t| self.resolve(t));
        cx.ret = ret.clone();
        if let Some(b) = &f.body {
            let t = self.expr(b, cx, ret.as_ref());
            if let Some(r) = &ret {
                let is_block = matches!(b.kind, ExprKind::Block(_));
                if !is_block || !matches!(t, Ty::Unit) { self.expect(&t, r, b.span); }
            }
        }
        for o in &f.olds { self.expr(o, cx, None); }
        cx.in_ensure = true;
        cx.scopes.push(HashMap::new());
        if let Some(r) = &ret { cx.declare("result", r.clone(), false); }
        for e in &f.ensures { let t = self.expr(&e.expr, cx, None); self.expect(&t, &Ty::Bool, e.expr.span); }
        cx.scopes.pop();
        cx.in_ensure = false;
    }

    fn check_args(&mut self, what: &str, params: &[(String, Ty, bool)], args: &[Arg], span: Span, cx: &mut Ctx) {
        let mut filled = vec![false; params.len()];
        let mut pos = 0;
        let mut named = false;
        for a in args {
            let idx = match &a.name {
                Some(n) => {
                    named = true;
                    match params.iter().position(|p| &p.0 == n) {
                        Some(i) => i,
                        None => { self.err(format!("{} has no parameter named '{}'", what, n), a.value.span); self.expr(&a.value, cx, None); continue; }
                    }
                }
                None => {
                    if named { self.err("positional argument after named argument", a.value.span); }
                    let i = pos; pos += 1; i
                }
            };
            if idx >= params.len() {
                self.err(format!("too many arguments for {} (expects {})", what, params.len()), a.value.span);
                self.expr(&a.value, cx, None);
                continue;
            }
            if filled[idx] { self.err(format!("parameter '{}' given twice", params[idx].0), a.value.span); }
            filled[idx] = true;
            let t = self.expr(&a.value, cx, Some(&params[idx].1));
            self.expect(&t, &params[idx].1, a.value.span);
        }
        for (i, p) in params.iter().enumerate() {
            if !filled[i] && !p.2 { self.err(format!("missing argument '{}' for {}", p.0, what), span); }
        }
    }

    fn block(&mut self, b: &Block, cx: &mut Ctx, expected: Option<&Ty>) -> Ty {
        cx.scopes.push(HashMap::new());
        let mut last = Ty::Unit;
        let n = b.stmts.len();
        for (i, s) in b.stmts.iter().enumerate() {
            last = Ty::Unit;
            match s {
                Stmt::Let { mutable, name, ty, init, span } => {
                    let declared = ty.as_ref().map(|t| self.resolve(t));
                    let t = self.expr(init, cx, declared.as_ref());
                    if let Some(d) = &declared { self.expect(&t, d, init.span); }
                    if is_builtin(name) || self.info.classes.contains_key(name) { self.err(format!("'{}' shadows a global name", name), *span); }
                    cx.declare(name, declared.unwrap_or(t), *mutable);
                }
                Stmt::Assign { target, value, span, .. } => self.assign(target, value, *span, cx),
                Stmt::Expr(e) => {
                    last = self.expr(e, cx, if i + 1 == n { expected } else { None });
                    // `if x == nil { return }` smart-casts x to non-null for the rest of the block
                    if let ExprKind::If(c, then, None) = &e.kind {
                        let ends = matches!(then.stmts.last(), Some(Stmt::Return(..) | Stmt::Break(_) | Stmt::Continue(_)))
                            || matches!(then.stmts.last(), Some(Stmt::Expr(x)) if diverges(x));
                        if let (true, (_, Some(name))) = (ends, nil_check(c)) {
                            if let Some(l) = cx.lookup(&name).cloned() {
                                cx.scopes.last_mut().unwrap().insert(name, Local { ty: l.ty.non_null(), ..l });
                            }
                        }
                    }
                }
                Stmt::Return(v, span) => {
                    if cx.lambda_depth == 0 {
                        let t = v.as_ref().map(|e| self.expr(e, cx, cx.ret.clone().as_ref())).unwrap_or(Ty::Unit);
                        if let Some(r) = cx.ret.clone() { if !(matches!(t, Ty::Unit) && v.is_none()) { self.expect(&t, &r, v.as_ref().map_or(*span, |e| e.span)); } }
                    } else if let Some(e) = v { self.expr(e, cx, None); }
                }
                Stmt::While(c, body) => {
                    let t = self.expr(c, cx, None);
                    self.expect(&t, &Ty::Bool, c.span);
                    cx.loop_depth += 1;
                    self.block(body, cx, None);
                    cx.loop_depth -= 1;
                }
                Stmt::For(name, it, body) => {
                    let t = self.expr(it, cx, None);
                    let elem = match t { Ty::Vec(e) => *e, _ => Ty::Any };
                    cx.scopes.push(HashMap::new());
                    cx.declare(name, elem, false);
                    cx.loop_depth += 1;
                    self.block(body, cx, None);
                    cx.loop_depth -= 1;
                    cx.scopes.pop();
                }
                Stmt::Break(s) | Stmt::Continue(s) => { if cx.loop_depth == 0 { self.err("break/continue outside of a loop", *s); } }
            }
        }
        cx.scopes.pop();
        last
    }

    fn assign(&mut self, target: &Expr, value: &Expr, span: Span, cx: &mut Ctx) {
        match &target.kind {
            ExprKind::Ident(n) => {
                if let Some(l) = cx.lookup(n).cloned() {
                    if !l.mutable { self.err(format!("cannot reassign 'val {}' (declare it with 'var')", n), target.span); }
                    if l.lambda_depth < cx.lambda_depth { self.err(format!("cannot assign captured variable '{}' inside a lambda; use an atom: `val {} = atom(...)` and `{}.swap(...)`", n, n, n), target.span); }
                    let t = self.expr(value, cx, Some(&l.ty));
                    self.expect(&t, &l.ty, value.span);
                } else if let Some(f) = self.field_of_current(cx, n) {
                    if !f.mutable { self.err(format!("cannot assign to 'val' field '{}' of '{}'", n, f.owner), target.span); }
                    let t = self.expr(value, cx, Some(&f.ty));
                    self.expect(&t, &f.ty, value.span);
                } else {
                    self.err(format!("unresolved name '{}'", n), target.span);
                }
            }
            ExprKind::Member { obj, name, .. } => {
                let ot = self.expr(obj, cx, None);
                if let ExprKind::Ident(base) = &obj.kind {
                    if let Some(l) = cx.lookup(base).cloned() {
                        if matches!(ot, Ty::Record(_) | Ty::Map(..)) && !l.mutable {
                            self.err(format!("cannot update field '{}' of 'val {}': records are immutable values; declare it with 'var' to rebind", name, base), target.span);
                        }
                    }
                }
                match &ot {
                    Ty::Record(fs) => match fs.iter().find(|f| &f.name == name) {
                        Some(f) if f.readonly => self.err(format!("cannot assign to '{}' because it is a read-only property", name), target.span),
                        None => self.err(format!("property '{}' does not exist on type {}", name, ot.show()), target.span),
                        Some(f) => { let ft = f.ty.clone(); let t = self.expr(value, cx, Some(&ft)); self.expect(&t, &ft, value.span); return; }
                    },
                    Ty::Class(c) => {
                        if let Some(f) = self.info.classes.get(c).and_then(|ci| ci.fields.iter().find(|f| &f.name == name)).cloned() {
                            if !f.mutable { self.err(format!("cannot assign to 'val' field '{}' of '{}'", name, f.owner), target.span); }
                        } else if self.info.classes.contains_key(c) {
                            self.err(format!("class '{}' has no field '{}'", c, name), target.span);
                        }
                    }
                    Ty::Nullable(_) => self.err(format!("cannot assign through nullable receiver of type {}", ot.show()), target.span),
                    _ => {}
                }
                self.expr(value, cx, None);
            }
            ExprKind::Index(obj, idx) => {
                if let ExprKind::Ident(base) = &obj.kind {
                    if let Some(l) = cx.lookup(base) { if !l.mutable { self.err(format!("cannot update 'val {}' (collections are persistent; declare it with 'var' to rebind)", base), target.span); } }
                }
                self.expr(obj, cx, None);
                self.expr(idx, cx, None);
                self.expr(value, cx, None);
            }
            _ => self.err("invalid assignment target", span),
        }
    }

    fn field_of_current(&self, cx: &Ctx, n: &str) -> Option<FieldInfo> {
        let c = cx.class.as_ref()?;
        self.info.classes.get(c)?.fields.iter().find(|f| f.name == n).cloned()
    }
    fn method_of_current(&self, cx: &Ctx, n: &str) -> Option<MethodSig> {
        let c = cx.class.as_ref()?;
        self.info.classes.get(c)?.methods.get(n).cloned()
    }

    fn expr(&mut self, e: &Expr, cx: &mut Ctx, expected: Option<&Ty>) -> Ty {
        use ExprKind as K;
        match &e.kind {
            K::Int(_) => Ty::Int,
            K::Float(_) => Ty::Float,
            K::Decimal(_) => Ty::Decimal,
            K::Quantity(x, u) => {
                let t = self.expr(x, cx, None);
                if !matches!(t, Ty::Int | Ty::Float | Ty::Decimal | Ty::Any) { self.err(format!("unit '{}' must follow a number", u), e.span); }
                Ty::Qty(vec![(u.clone(), 1)])
            }
            K::Str(segs) => {
                for s in segs { if let StrSeg::Expr(x) = s { self.expr(x, cx, None); } }
                if let [StrSeg::Lit(l)] = segs.as_slice() { if matches!(expected, Some(Ty::Lit(_)) | Some(Ty::Union(_))) { return Ty::Lit(l.clone()); } }
                Ty::Str
            }
            K::Bool(_) => Ty::Bool,
            K::Nil => Ty::Nil,
            K::This => match &cx.class { Some(c) => Ty::Class(c.clone()), None => { self.err("'this' outside of a class", e.span); Ty::Any } },
            K::Result => { if !cx.in_ensure { self.err("'result' is only available in ensure clauses", e.span); } cx.lookup("result").map(|l| l.ty.clone()).unwrap_or(Ty::Any) }
            K::Old(x, _) => { if !cx.in_ensure { self.err("'old(...)' is only available in ensure clauses", e.span); } self.expr(x, cx, None) }
            K::Ident(n) => self.ident(n, e.span, cx),
            K::CellRef(_) => Ty::Any,
            K::CellRange(..) => Ty::Vec(Box::new(Ty::Any)),
            K::Binary(op, l, r) => self.binary(op, l, r, cx),
            K::Unary(op, x) => {
                let t = self.expr(x, cx, None);
                if op == "!" { self.expect(&t, &Ty::Bool, x.span); Ty::Bool } else { t }
            }
            K::Percent(x) => { self.expr(x, cx, None); Ty::Decimal }
            K::Thread { .. } => Ty::Any, // removed by desugaring
            K::Call(callee, args) => self.call(callee, args, e.span, cx, expected),
            K::Member { obj, name, safe } => {
                let ot = self.expr(obj, cx, None);
                self.member(&ot, name, *safe, e.span, obj)
            }
            K::Super { qualifier, name, args } => {
                let Some(c) = cx.class.clone() else { self.err("'super' outside of a class", e.span); return Ty::Any; };
                let ci = self.info.classes[&c].clone();
                if let Some(q) = qualifier {
                    if !ci.mro.contains(q) || q == &c { self.err(format!("'{}' is not a supertype of '{}'", q, c), e.span); }
                    else if !self.info.classes[q].methods.contains_key(name) { self.err(format!("'{}' has no method '{}'", q, name), e.span); }
                } else if !ci.mro.iter().skip(1).any(|a| self.info.classes[a].methods.contains_key(name)) {
                    self.err(format!("no supertype of '{}' defines '{}'", c, name), e.span);
                }
                for a in args { self.expr(&a.value, cx, None); }
                Ty::Any
            }
            K::Index(o, i) => {
                let ot = self.expr(o, cx, None);
                self.expr(i, cx, None);
                match ot { Ty::Vec(t) => t.nullable(), Ty::Map(_, v) => v.nullable(), Ty::Nullable(_) => { self.err(format!("cannot index nullable value of type {}; use ?. or a nil check", ot.show()), o.span); Ty::Any } _ => Ty::Any }
            }
            K::Lambda(params, body) => {
                cx.scopes.push(HashMap::new());
                cx.lambda_depth += 1;
                let (pts, _) = match expected { Some(Ty::Fn(p, r)) => (p.clone(), Some(r.clone())), _ => (vec![], None) };
                for (i, p) in params.iter().enumerate() {
                    let t = p.ty.as_ref().map(|t| self.resolve(t)).or_else(|| pts.get(i).cloned()).unwrap_or(Ty::Any);
                    cx.declare(&p.name, t, false);
                }
                let saved_loop = cx.loop_depth;
                cx.loop_depth = 0;
                let rt = self.expr(body, cx, None);
                cx.loop_depth = saved_loop;
                cx.lambda_depth -= 1;
                cx.scopes.pop();
                Ty::Fn(params.iter().map(|_| Ty::Any).collect(), Box::new(rt))
            }
            K::If(c, t, el) => {
                let ct = self.expr(c, cx, None);
                self.expect(&ct, &Ty::Bool, c.span);
                // Kotlin smart casts: `x != nil` narrows x in the then-branch, `x == nil` in the else-branch
                let (then_narrow, else_narrow) = nil_check(c);
                let tt = self.narrowed(then_narrow, cx, |me, cx| me.block(t, cx, expected));
                let et = match el { Some(b) => self.narrowed(else_narrow, cx, |me, cx| me.block(b, cx, expected)), None => Ty::Unit };
                if el.is_none() { return Ty::Unit; }
                join(tt, et)
            }
            K::Match(s, arms) => self.match_expr(s, arms, cx, expected),
            K::Vec(items) => {
                let mut et: Option<Ty> = None;
                let exp_elem = match expected { Some(Ty::Vec(t)) => Some((**t).clone()), _ => None };
                for i in items {
                    let t = self.expr(i, cx, exp_elem.as_ref());
                    if let Some(x) = &exp_elem { self.expect(&t, x, i.span); }
                    et = Some(match et { None => t, Some(p) => join(p, t) });
                }
                Ty::Vec(Box::new(et.unwrap_or(Ty::Any)))
            }
            K::Map(entries) => self.map_lit(entries, e.span, cx, expected),
            K::Try(x) => {
                let t = self.expr(x, cx, None);
                match t { Ty::Nullable(i) => *i, _ => Ty::Any }
            }
            K::NotNull(x) => self.expr(x, cx, None).non_null(),
            K::Range(a, b) => { let ta = self.expr(a, cx, None); let tb = self.expr(b, cx, None); self.expect(&ta, &Ty::Int, a.span); self.expect(&tb, &Ty::Int, b.span); Ty::Vec(Box::new(Ty::Int)) }
            K::Block(b) => self.block(b, cx, expected),
            K::Is(x, t) => {
                self.expr(x, cx, None);
                if let TypeNode::Name { name, span, .. } = t { if !self.is_specializer(name) { self.err(format!("unknown type '{}'", name), *span); } }
                Ty::Bool
            }
            K::Sheet(cells) => {
                let mut seen = HashSet::new();
                for c in cells {
                    if !seen.insert(c.cell.clone()) { self.err(format!("cell {} is defined twice", c.cell), c.span); }
                    cx.scopes.push(HashMap::new());
                    cx.lambda_depth += 1; // formulas are closures
                    self.expr(&c.value, cx, None);
                    cx.lambda_depth -= 1;
                    cx.scopes.pop();
                }
                Ty::Sheet
            }
        }
    }

    fn narrowed(&mut self, n: Option<String>, cx: &mut Ctx, f: impl FnOnce(&mut Self, &mut Ctx) -> Ty) -> Ty {
        if let Some(name) = n {
            if let Some(l) = cx.lookup(&name).cloned() {
                cx.scopes.push(HashMap::new());
                cx.scopes.last_mut().unwrap().insert(name, Local { ty: l.ty.non_null(), ..l });
                let t = f(self, cx);
                cx.scopes.pop();
                return t;
            }
        }
        f(self, cx)
    }

    fn ident(&mut self, n: &str, span: Span, cx: &mut Ctx) -> Ty {
        if let Some(l) = cx.lookup(n) { return l.ty.clone(); }
        if let Some(f) = self.field_of_current(cx, n) { return f.ty; }
        if let Some(m) = self.method_of_current(cx, n) { return Ty::Fn(m.params.iter().map(|p| p.1.clone()).collect(), Box::new(m.ret)); }
        if let Some(f) = self.info.funs.get(n) { return Ty::Fn(f.params.iter().map(|p| p.1.clone()).collect(), Box::new(f.ret.clone())); }
        if let Some(e) = self.info.variant_of.get(n) {
            if self.info.variant_fields(n).map_or(false, |f| f.is_empty()) { return Ty::Enum(e.clone()); }
            return Ty::Fn(vec![], Box::new(Ty::Enum(e.clone())));
        }
        if self.info.classes.contains_key(n) || self.info.generics.contains_key(n) || is_builtin(n) { return Ty::Any; }
        if n == "callNext" { return Ty::Any; }
        self.err(format!("unresolved name '{}'", n), span);
        Ty::Any
    }

    fn binary(&mut self, op: &str, l: &Expr, r: &Expr, cx: &mut Ctx) -> Ty {
        let lt = self.expr(l, cx, None);
        if op == "?:" {
            let rt = self.expr(r, cx, None);
            if diverges(r) { return lt.non_null(); } // `x ?: return y`
            return join(lt.non_null(), rt);
        }
        // smart casts flow into the right operand: `x != nil && x.f`, `x == nil || x.f`
        let narrow = match op { "&&" => nil_check(l).0, "||" => nil_check(l).1, _ => None };
        let rt = self.narrowed(narrow, cx, |me, cx| me.expr(r, cx, None));
        let num = |t: &Ty| matches!(t, Ty::Int | Ty::Float | Ty::Decimal | Ty::Any | Ty::Union(_));
        match op {
            "&&" | "||" => { self.expect(&lt, &Ty::Bool, l.span); self.expect(&rt, &Ty::Bool, r.span); Ty::Bool }
            "==" | "!=" => Ty::Bool,
            "<" | "<=" | ">" | ">=" => {
                match (&lt, &rt) {
                    (Ty::Qty(a), Ty::Qty(b)) if !units::compatible(a, b) => self.err(format!("cannot compare {} with {}: incompatible units", units::show(a), units::show(b)), r.span),
                    (Ty::Qty(a), Ty::Int | Ty::Float | Ty::Decimal) | (Ty::Int | Ty::Float | Ty::Decimal, Ty::Qty(a)) => self.err(format!("cannot compare a quantity in {} with a plain number", units::show(a)), l.span.to(r.span)),
                    _ => {}
                }
                Ty::Bool
            }
            "&" => Ty::Str,
            "+" if matches!(lt, Ty::Str) || matches!(rt, Ty::Str) => Ty::Str,
            "+" | "-" | "*" | "/" | "%" | "^" if matches!(lt, Ty::Qty(_)) || matches!(rt, Ty::Qty(_)) => {
                let plain = |t: &Ty| matches!(t, Ty::Int | Ty::Float | Ty::Decimal);
                let is_ord = |t: &Ty| matches!(t, Ty::Qty(u) if u.len() == 1 && u[0].0 == "ord");
                // ordinals form an affine space over cardinals: position ± count = position, position − position = count
                if is_ord(&lt) || is_ord(&rt) {
                    return match (op, is_ord(&lt), is_ord(&rt)) {
                        ("+", true, false) | ("-", true, false) if matches!(rt, Ty::Int | Ty::Any) => lt.clone(),
                        ("+", false, true) if matches!(lt, Ty::Int | Ty::Any) => rt.clone(),
                        ("-", true, true) => Ty::Int,
                        ("+", true, true) => { self.err("cannot add two ordinals (positions); subtract them to get a count, or add a count to a position", l.span.to(r.span)); Ty::Any }
                        ("+" | "-", _, _) => { self.err(format!("ordinals only move by whole cardinal numbers (Int), not by {}", if is_ord(&lt) { rt.show() } else { lt.show() }), l.span.to(r.span)); Ty::Any }
                        _ => { self.err(format!("operator '{}' is not defined for ordinals (positions); use a cardinal number", op), l.span.to(r.span)); Ty::Any }
                    };
                }
                match (op, &lt, &rt) {
                    ("+" | "-" | "%", Ty::Qty(a), Ty::Qty(b)) => {
                        if !units::compatible(a, b) { self.err(format!("cannot {} {} and {}: incompatible units", if op == "-" { "subtract" } else { "add" }, units::show(a), units::show(b)), r.span); }
                        lt.clone()
                    }
                    ("+" | "-", Ty::Qty(a), t) | ("+" | "-", t, Ty::Qty(a)) if plain(t) => {
                        self.err(format!("cannot {} a plain number and a quantity in {}; give the number a unit", if op == "-" { "subtract" } else { "add" }, units::show(a)), l.span.to(r.span));
                        Ty::Any
                    }
                    ("*", Ty::Qty(a), Ty::Qty(b)) => { let u = units::combine(a, b, 1); if u.is_empty() { Ty::Decimal } else { Ty::Qty(u) } }
                    ("/", Ty::Qty(a), Ty::Qty(b)) => {
                        if units::compatible(a, b) && !units::same(a, b) { return Ty::Decimal; }
                        let u = units::combine(a, b, -1); if u.is_empty() { Ty::Decimal } else { Ty::Qty(u) }
                    }
                    ("*", Ty::Qty(a), _) | ("*", _, Ty::Qty(a)) | ("/", Ty::Qty(a), _) => Ty::Qty(a.clone()),
                    ("/", _, Ty::Qty(b)) => Ty::Qty(units::combine(&vec![], b, -1)),
                    _ => Ty::Any,
                }
            }
            "+" | "-" | "*" | "/" | "%" | "^" => {
                for (t, x) in [(&lt, l), (&rt, r)] {
                    if let Ty::Nullable(_) = t { self.err(format!("operator '{}' cannot be applied to nullable type {}", op, t.show()), x.span); }
                    else if !num(t) && !(op == "+" && matches!(t, Ty::Vec(_))) { self.err(format!("operator '{}' expects numbers but found {}", op, t.show()), x.span); }
                }
                if op == "^" { return Ty::Float; }
                match (&lt, &rt) {
                    (Ty::Int, Ty::Int) => Ty::Int,
                    (Ty::Float, _) | (_, Ty::Float) => Ty::Float,
                    (Ty::Decimal, Ty::Int | Ty::Decimal) | (Ty::Int, Ty::Decimal) => Ty::Decimal,
                    _ => Ty::Any,
                }
            }
            _ => Ty::Any,
        }
    }

    fn member(&mut self, ot: &Ty, name: &str, safe: bool, span: Span, obj: &Expr) -> Ty {
        let base = if safe { ot.non_null() } else {
            if let Ty::Nullable(_) = ot {
                self.err(format!("only safe (?.) or non-null asserted (!!.) access is allowed on a nullable receiver of type {}", ot.show()), obj.span);
            }
            ot.non_null()
        };
        let wrap = |t: Ty| if safe { t.nullable() } else { t };
        match &base {
            Ty::Record(fs) => match fs.iter().find(|f| f.name == name) {
                Some(f) => wrap(if f.optional { f.ty.clone().nullable() } else { f.ty.clone() }),
                None => {
                    if !["get", "assoc", "dissoc", "keys", "values", "count", "containsKey", "merge", "update", "toString", "isEmpty"].contains(&name) {
                        self.err(format!("property '{}' does not exist on type {}", name, base.show()), span);
                    }
                    Ty::Any
                }
            },
            Ty::Class(c) => {
                if let Some(ci) = self.info.classes.get(c) {
                    if let Some(f) = ci.fields.iter().find(|f| f.name == name) { return wrap(f.ty.clone()); }
                    if let Some(m) = ci.methods.get(name) { return Ty::Fn(m.params.iter().map(|p| p.1.clone()).collect(), Box::new(wrap(m.ret.clone()))); }
                    if !["toString", "copy", "equals"].contains(&name) {
                        self.err(format!("'{}' has no field or method '{}'", c, name), span);
                    }
                }
                Ty::Any
            }
            Ty::Str => match name { "length" => Ty::Int, _ => Ty::Any },
            Ty::Qty(_) => match name { "amount" => Ty::Any, "unit" => Ty::Str, _ => Ty::Any },
            _ => Ty::Any,
        }
    }

    fn call(&mut self, callee: &Expr, args: &[Arg], span: Span, cx: &mut Ctx, expected: Option<&Ty>) -> Ty {
        let _ = expected;
        match &callee.kind {
            ExprKind::Ident(n) if cx.lookup(n).is_none() && self.field_of_current(cx, n).is_none() => {
                if n == "callNext" {
                    if !cx.in_generic_method { self.err("callNext() is only available inside generic methods", callee.span); }
                    for a in args { self.expr(&a.value, cx, None); }
                    return Ty::Any;
                }
                if let Some(m) = self.method_of_current(cx, n) {
                    self.check_args(&format!("method '{}'", n), &m.params, args, span, cx);
                    return m.ret;
                }
                if let Some(f) = self.info.funs.get(n).cloned() {
                    self.check_args(&format!("function '{}'", n), &f.params, args, span, cx);
                    return f.ret;
                }
                if let Some(ci) = self.info.classes.get(n).cloned() {
                    if ci.is_abstract { self.err(format!("cannot instantiate {} '{}'", if ci.is_trait { "trait" } else { "abstract class" }, n), callee.span); }
                    let ps: Vec<(String, Ty, bool)> = self.ctor_params.get(n).cloned().unwrap_or_default().into_iter()
                        .map(|(p, d)| { let t = ci.fields.iter().find(|f| f.name == p).map(|f| f.ty.clone()).or_else(|| self.ctor_types.get(&(n.clone(), p.clone())).cloned()).unwrap_or(Ty::Any); (p, t, d) }).collect();
                    self.check_args(&format!("constructor of '{}'", n), &ps, args, span, cx);
                    return Ty::Class(n.clone());
                }
                if let Some(e) = self.info.variant_of.get(n).cloned() {
                    let fields = self.info.variant_fields(n).cloned().unwrap_or_default();
                    if args.len() != fields.len() { self.err(format!("variant '{}' takes {} value(s) but {} were given", n, fields.len(), args.len()), span); }
                    for a in args { self.expr(&a.value, cx, None); }
                    return Ty::Enum(e);
                }
                if let Some(&k) = self.info.generics.get(n) {
                    if args.len() != k { self.err(format!("generic '{}' takes {} argument(s) but {} were given", n, k, args.len()), span); }
                    for a in args { self.expr(&a.value, cx, None); }
                    return Ty::Any;
                }
                if is_builtin(n) {
                    let mut ats = vec![];
                    for a in args { if a.name.is_some() { self.err("builtin functions take positional arguments only", a.value.span); } ats.push(self.expr(&a.value, cx, None)); }
                    return match n.as_str() {
                        "str" | "UPPER" | "LOWER" | "TRIM" | "LEFT" | "RIGHT" | "MID" | "CONCATENATE" | "TEXTJOIN" | "SUBSTITUTE" => Ty::Str,
                        "LEN" | "COUNT" | "COUNTA" | "COUNTIF" => Ty::Int,
                        "ISBLANK" | "ISNUMBER" | "ISTEXT" | "ISERROR" | "TRUE" | "FALSE" => Ty::Bool,
                        "println" | "print" | "eprintln" | "writeFile" | "exit" => Ty::Unit,
                        "readFile" => Ty::Enum("Result".into()),
                        "embedFile" => Ty::Str,
                        "exec" => Ty::Int,
                        "args" => Ty::Vec(Box::new(Ty::Str)),
                        "atom" => Ty::Atom,
                        "range" | "SEQUENCE" => Ty::Vec(Box::new(Ty::Int)),
                        "Ok" | "Err" => Ty::Enum("Result".into()),
                        "SUM" | "MIN" | "MAX" => match ats.first() { Some(Ty::Vec(t)) => match &**t { Ty::Qty(u) => Ty::Qty(u.clone()), _ => Ty::Any }, Some(Ty::Qty(u)) => Ty::Qty(u.clone()), _ => Ty::Any },
                        _ => Ty::Any,
                    };
                }
                self.err(format!("unresolved function '{}'", n), callee.span);
                for a in args { self.expr(&a.value, cx, None); }
                Ty::Any
            }
            ExprKind::Member { obj, name, safe } => {
                let ot = self.expr(obj, cx, None);
                if let Ty::Class(c) = ot.non_null() {
                    if let Some(m) = self.info.classes.get(&c).and_then(|ci| ci.methods.get(name)).cloned() {
                        if let Ty::Nullable(_) = ot { if !*safe { self.err(format!("only safe (?.) or non-null asserted (!!.) calls are allowed on a nullable receiver of type {}", ot.show()), obj.span); } }
                        self.check_args(&format!("method '{}.{}'", c, name), &m.params, args, span, cx);
                        return if *safe { m.ret.nullable() } else { m.ret };
                    }
                    if name == "copy" {
                        let ci = self.info.classes.get(&c).cloned();
                        if let Some(ci) = ci {
                            if !ci.is_data { self.err(format!("copy() is only available on data classes; '{}' is not a data class", c), callee.span); }
                            for a in args {
                                match &a.name {
                                    None => self.err("copy() takes named arguments, e.g. p.copy(x = 1)", a.value.span),
                                    Some(n) if !ci.fields.iter().any(|f| &f.name == n) => self.err(format!("'{}' has no field '{}'", c, n), a.value.span),
                                    _ => {}
                                }
                                self.expr(&a.value, cx, None);
                            }
                        }
                        return Ty::Class(c);
                    }
                }
                let mt = self.member(&ot, name, *safe, callee.span, obj);
                for a in args { self.expr(&a.value, cx, None); }
                match mt { Ty::Fn(_, r) => *r, _ => Ty::Any }
            }
            _ => {
                let ft = self.expr(callee, cx, None);
                for a in args { self.expr(&a.value, cx, None); }
                match ft { Ty::Fn(_, r) => *r, _ => Ty::Any }
            }
        }
    }

    fn map_lit(&mut self, entries: &[MapEntry], span: Span, cx: &mut Ctx, expected: Option<&Ty>) -> Ty {
        let exp_rec = match expected { Some(Ty::Record(fs)) => Some(fs.clone()), Some(Ty::Nullable(t)) => if let Ty::Record(fs) = &**t { Some(fs.clone()) } else { None }, _ => None };
        let mut fields: BTreeMap<String, RField> = BTreeMap::new();
        let mut order: Vec<String> = vec![];
        let mut is_record = true;
        for en in entries {
            match en {
                MapEntry::Spread(x) => {
                    match self.expr(x, cx, None) {
                        Ty::Record(fs) => for f in fs {
                            let keep_required = fields.get(&f.name).map_or(false, |p| !p.optional);
                            if !order.contains(&f.name) { order.push(f.name.clone()); }
                            fields.insert(f.name.clone(), RField { optional: f.optional && !keep_required, ..f });
                        },
                        _ => is_record = false,
                    }
                }
                MapEntry::Kv(k, v) => {
                    let key = match &k.kind { ExprKind::Str(s) => match s.as_slice() { [StrSeg::Lit(l)] => Some(l.clone()), _ => None }, _ => None };
                    if key.is_none() { self.expr(k, cx, None); is_record = false; }
                    let fexp = key.as_ref().and_then(|kk| exp_rec.as_ref().and_then(|fs| fs.iter().find(|f| &f.name == kk).map(|f| f.ty.clone())));
                    let vt = self.expr(v, cx, fexp.as_ref());
                    if let Some(kk) = key {
                        if let Some(fs) = &exp_rec {
                            match fs.iter().find(|f| f.name == kk) {
                                None => self.err(format!("object literal may only specify known properties, and '{}' does not exist in type {}", kk, Ty::Record(fs.clone()).show()), k.span),
                                Some(f) => { let ft = if f.optional { f.ty.clone().nullable() } else { f.ty.clone() }; self.expect(&vt, &ft, v.span); }
                            }
                        }
                        if !order.contains(&kk) { order.push(kk.clone()); }
                        fields.insert(kk.clone(), RField { name: kk, ty: vt, optional: false, readonly: false });
                    }
                }
            }
        }
        if !is_record { return Ty::Map(Box::new(Ty::Any), Box::new(Ty::Any)); }
        let t = Ty::Record(order.iter().map(|n| fields[n].clone()).collect());
        if let Some(fs) = &exp_rec {
            for f in fs {
                if !f.optional && !fields.contains_key(&f.name) {
                    self.err(format!("property '{}' is missing but required in type {}", f.name, Ty::Record(fs.clone()).show()), span);
                }
            }
            return Ty::Record(fs.clone());
        }
        t
    }

    fn match_expr(&mut self, s: &Expr, arms: &[MatchArm], cx: &mut Ctx, expected: Option<&Ty>) -> Ty {
        let st = self.expr(s, cx, None);
        let mut out: Option<Ty> = None;
        let mut catch_all = false;
        let mut covered: HashSet<String> = HashSet::new();
        let mut enum_name: Option<String> = match &st { Ty::Enum(e) => Some(e.clone()), _ => None };
        for a in arms {
            cx.scopes.push(HashMap::new());
            self.pattern(&a.pat, cx, &mut enum_name);
            if a.guard.is_none() {
                match &a.pat {
                    Pattern::Wild | Pattern::Bind(_) => catch_all = true,
                    Pattern::Variant { name, args, .. } if args.iter().all(|p| matches!(p, Pattern::Wild | Pattern::Bind(_))) => { covered.insert(name.clone()); }
                    _ => {}
                }
            }
            if let Some(g) = &a.guard { let gt = self.expr(g, cx, None); self.expect(&gt, &Ty::Bool, g.span); }
            let t = self.expr(&a.body, cx, expected);
            cx.scopes.pop();
            out = Some(match out { None => t, Some(p) => join(p, t) });
        }
        if !catch_all {
            if let Some(en) = &enum_name {
                let missing: Vec<String> = self.info.enums[en].variants.iter().map(|v| v.0.clone()).filter(|v| !covered.contains(v)).collect();
                if !missing.is_empty() {
                    self.err(format!("non-exhaustive match on {}: missing {}", en, missing.join(", ")), s.span);
                }
            }
        }
        out.unwrap_or(Ty::Unit)
    }

    fn pattern(&mut self, p: &Pattern, cx: &mut Ctx, enum_name: &mut Option<String>) {
        match p {
            Pattern::Wild => {}
            Pattern::Bind(n) => cx.declare(n, Ty::Any, false),
            Pattern::Lit(e) => { self.expr(e, cx, None); }
            Pattern::Variant { name, args, span } => {
                match self.info.variant_of.get(name).cloned() {
                    Some(e) => {
                        match enum_name { None => *enum_name = Some(e.clone()), Some(x) if *x != e => self.err(format!("variant '{}' belongs to '{}', not '{}'", name, e, x), *span), _ => {} }
                        let n = self.info.variant_fields(name).map_or(0, |f| f.len());
                        if args.len() != n { self.err(format!("pattern '{}' expects {} field(s) but has {}", name, n, args.len()), *span); }
                    }
                    None => self.err(format!("unknown enum variant '{}'", name), *span),
                }
                let mut inner = None;
                for a in args { self.pattern(a, cx, &mut inner); }
            }
        }
    }
}

fn diverges(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Block(b) => matches!(b.stmts.last(), Some(Stmt::Return(..)) | Some(Stmt::Break(_)) | Some(Stmt::Continue(_))),
        ExprKind::Call(c, _) => matches!(&c.kind, ExprKind::Ident(n) if n == "panic"),
        _ => false,
    }
}

fn join(a: Ty, b: Ty) -> Ty {
    if a == b { return a; }
    match (&a, &b) {
        (Ty::Nil, t) | (t, Ty::Nil) => t.clone().nullable(),
        (Ty::Int, Ty::Float) | (Ty::Float, Ty::Int) | (Ty::Decimal, Ty::Float) | (Ty::Float, Ty::Decimal) => Ty::Float,
        (Ty::Int, Ty::Decimal) | (Ty::Decimal, Ty::Int) => Ty::Decimal,
        (Ty::Qty(x), Ty::Qty(y)) if units::compatible(x, y) => a.clone(),
        (Ty::Lit(_), Ty::Str) | (Ty::Str, Ty::Lit(_)) | (Ty::Lit(_), Ty::Lit(_)) => Ty::Str,
        _ => Ty::Any,
    }
}

/// For `x != nil` returns (Some(x), None); for `x == nil` returns (None, Some(x)).
fn nil_check(c: &Expr) -> (Option<String>, Option<String>) {
    if let ExprKind::Binary(op, l, r) = &c.kind {
        let pick = |a: &Expr, b: &Expr| if let (ExprKind::Ident(n), ExprKind::Nil) = (&a.kind, &b.kind) { Some(n.clone()) } else { None };
        let n = pick(l, r).or_else(|| pick(r, l));
        if let Some(n) = n {
            if op == "!=" { return (Some(n), None); }
            if op == "==" { return (None, Some(n)); }
        }
        if op == "&&" {
            let (a, _) = nil_check(l);
            let (b, _) = nil_check(r);
            return (a.or(b), None);
        }
        if op == "||" {
            let (_, a) = nil_check(l);
            let (_, b) = nil_check(r);
            return (None, a.or(b));
        }
    }
    (None, None)
}
