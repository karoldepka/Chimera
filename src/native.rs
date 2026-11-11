//! Native lowering for the Rust backend.
//!
//! A function whose parameters and return type are all `Int`, `Float` or `Bool`, and whose body
//! only uses arithmetic, comparisons, locals, `if`, loops and calls to other native functions,
//! is compiled to a plain Rust function over `i64` / `f64` / `bool` (no `Value` boxing).
//! A thin `Value` wrapper is emitted next to it so the rest of the program calls it unchanged.
//! Integer overflow stays an error (the generated crate builds with `overflow-checks = true`).

use std::collections::{HashMap, HashSet};

use crate::ast::*;
use crate::common::{contract_msg, order_args};
use crate::gen_rust::rid;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum NTy { I64, F64, Bool }

impl NTy {
    pub fn rust(self) -> &'static str { match self { NTy::I64 => "i64", NTy::F64 => "f64", NTy::Bool => "bool" } }
    fn from_node(t: &Option<TypeNode>) -> Option<NTy> {
        match t {
            Some(TypeNode::Name { name, args, .. }) if args.is_empty() => match name.as_str() {
                "Int" => Some(NTy::I64), "Float" => Some(NTy::F64), "Bool" => Some(NTy::Bool), _ => None,
            },
            _ => None,
        }
    }
    /// Value -> native
    pub fn unbox(self, v: &str) -> String {
        match self { NTy::I64 => format!("rt::as_int(&{})", v), NTy::F64 => format!("rt::as_f64(&{})", v), NTy::Bool => format!("rt::truthy(&{})", v) }
    }
    /// native -> Value
    pub fn boxed(self, v: &str) -> String {
        match self { NTy::I64 => format!("Value::Int({})", v), NTy::F64 => format!("Value::Float({})", v), NTy::Bool => format!("Value::Bool({})", v) }
    }
}

#[derive(Clone, Debug)]
pub struct NSig { pub params: Vec<NTy>, pub ret: NTy }

/// Find the functions that can be lowered natively (fixpoint over the call graph).
pub fn analyze(p: &Program) -> HashMap<String, NSig> {
    let mut cand: HashMap<String, (NSig, &FunDecl)> = HashMap::new();
    for d in &p.decls {
        if let Decl::Fun(f) = d {
            let ps: Option<Vec<NTy>> = f.params.iter().map(|p| NTy::from_node(&p.ty)).collect();
            if let (Some(ps), Some(r)) = (ps, NTy::from_node(&f.ret)) {
                if f.body.is_some() { cand.insert(f.name.clone(), (NSig { params: ps, ret: r }, f)); }
            }
        }
    }
    loop {
        let sigs: HashMap<String, NSig> = cand.iter().map(|(k, v)| (k.clone(), v.0.clone())).collect();
        let bad: Vec<String> = cand.iter().filter(|(_, (sig, f))| !Checker::new(&sigs).fun_ok(f, sig)).map(|(k, _)| k.clone()).collect();
        if bad.is_empty() { return sigs; }
        for b in bad { cand.remove(&b); }
    }
}

struct Checker<'a> {
    sigs: &'a HashMap<String, NSig>,
    scopes: Vec<HashMap<String, NTy>>,
    ret: Option<NTy>,
}

impl<'a> Checker<'a> {
    fn new(sigs: &'a HashMap<String, NSig>) -> Self { Checker { sigs, scopes: vec![HashMap::new()], ret: None } }
    fn lookup(&self, n: &str) -> Option<NTy> { self.scopes.iter().rev().find_map(|s| s.get(n).copied()) }

    fn fun_ok(&mut self, f: &FunDecl, sig: &NSig) -> bool {
        for (p, t) in f.params.iter().zip(&sig.params) {
            if let Some(d) = &p.default { if self.ty(d) != Some(*t) { return false; } }
            self.scopes[0].insert(p.name.clone(), *t);
        }
        self.ret = Some(sig.ret);
        if f.requires.iter().any(|c| self.ty(&c.expr) != Some(NTy::Bool)) { return false; }
        if f.olds.iter().any(|o| self.ty(o).is_none()) { return false; }
        let body = f.body.as_ref().unwrap();
        if !self.value_of(body, sig.ret) { return false; }
        self.scopes.push(HashMap::from([("result".to_string(), sig.ret)]));
        let ok = f.ensures.iter().all(|c| self.ty(&c.expr) == Some(NTy::Bool));
        self.scopes.pop();
        ok
    }

    /// expression used as a value of type `want` (blocks may end in `return`)
    fn value_of(&mut self, e: &Expr, want: NTy) -> bool {
        match &e.kind {
            ExprKind::Block(b) => {
                self.scopes.push(HashMap::new());
                let n = b.stmts.len();
                let mut ok = true;
                for (i, s) in b.stmts.iter().enumerate() {
                    if i + 1 == n {
                        ok &= match s {
                            Stmt::Expr(x) => self.value_of(x, want),
                            Stmt::Return(Some(x), _) => self.ty(x) == self.ret,
                            _ => false,
                        };
                    } else { ok &= self.stmt(s); }
                }
                self.scopes.pop();
                ok && n > 0
            }
            ExprKind::If(c, t, Some(el)) => {
                self.ty(c) == Some(NTy::Bool)
                    && self.value_of(&Expr::new(ExprKind::Block(t.clone()), e.span), want)
                    && self.value_of(&Expr::new(ExprKind::Block(el.clone()), e.span), want)
            }
            _ => self.ty(e) == Some(want),
        }
    }

    fn stmt(&mut self, s: &Stmt) -> bool {
        match s {
            Stmt::Let { name, ty, init, .. } => {
                let Some(t) = self.ty(init) else { return false };
                if ty.is_some() && NTy::from_node(ty) != Some(t) { return false; }
                self.scopes.last_mut().unwrap().insert(name.clone(), t);
                true
            }
            Stmt::Assign { target, value, .. } => match &target.kind {
                ExprKind::Ident(n) => self.lookup(n).is_some() && self.ty(value) == self.lookup(n),
                _ => false,
            },
            Stmt::Expr(e) => match &e.kind {
                ExprKind::If(c, t, el) => {
                    self.ty(c) == Some(NTy::Bool) && self.stmts(&t.stmts) && el.as_ref().map_or(true, |b| self.stmts(&b.stmts))
                }
                ExprKind::Block(b) => self.stmts(&b.stmts),
                _ => self.ty(e).is_some(),
            },
            Stmt::Return(Some(x), _) => self.ty(x) == self.ret,
            Stmt::Return(None, _) => false,
            Stmt::While(c, b) => self.ty(c) == Some(NTy::Bool) && self.stmts(&b.stmts),
            Stmt::For(name, it, b) => {
                let ExprKind::Range(a, z) = &it.kind else { return false };
                if self.ty(a) != Some(NTy::I64) || self.ty(z) != Some(NTy::I64) { return false; }
                self.scopes.push(HashMap::from([(name.clone(), NTy::I64)]));
                let ok = self.stmts(&b.stmts);
                self.scopes.pop();
                ok
            }
            Stmt::Break(_) | Stmt::Continue(_) => true,
        }
    }
    fn stmts(&mut self, ss: &[Stmt]) -> bool {
        self.scopes.push(HashMap::new());
        let ok = ss.iter().all(|s| self.stmt(s));
        self.scopes.pop();
        ok
    }

    fn ty(&mut self, e: &Expr) -> Option<NTy> {
        use ExprKind as K;
        match &e.kind {
            K::Int(_) => Some(NTy::I64),
            K::Float(_) => Some(NTy::F64),
            K::Bool(_) => Some(NTy::Bool),
            K::Ident(n) => self.lookup(n),
            K::Result => self.lookup("result"),
            K::Old(x, _) => self.ty(x),
            K::Unary(op, x) => {
                let t = self.ty(x)?;
                match (op.as_str(), t) { ("-", NTy::I64 | NTy::F64) => Some(t), ("!", NTy::Bool) => Some(t), _ => None }
            }
            K::Binary(op, l, r) => {
                let (a, b) = (self.ty(l)?, self.ty(r)?);
                if a != b { return None; }
                match op.as_str() {
                    "+" | "-" | "*" | "/" | "%" if a != NTy::Bool => Some(a),
                    "<" | "<=" | ">" | ">=" if a != NTy::Bool => Some(NTy::Bool),
                    "==" | "!=" => Some(NTy::Bool),
                    "&&" | "||" if a == NTy::Bool => Some(NTy::Bool),
                    _ => None,
                }
            }
            K::If(c, t, Some(el)) => {
                if self.ty(c) != Some(NTy::Bool) { return None; }
                let tb = Expr::new(K::Block(t.clone()), e.span);
                let probe = self.block_ty(&tb)?;
                if self.value_of(&Expr::new(K::Block(el.clone()), e.span), probe) { Some(probe) } else { None }
            }
            K::Block(_) => self.block_ty(e),
            K::Call(c, args) => {
                let K::Ident(n) = &c.kind else { return None };
                if self.lookup(n).is_some() { return None; }
                let sig = self.sigs.get(n)?.clone();
                if args.iter().any(|a| a.name.is_some()) { return None; }
                if args.len() > sig.params.len() { return None; }
                for (a, t) in args.iter().zip(&sig.params) { if self.ty(&a.value) != Some(*t) { return None; } }
                Some(sig.ret)
            }
            _ => None,
        }
    }
    fn block_ty(&mut self, e: &Expr) -> Option<NTy> {
        let ExprKind::Block(b) = &e.kind else { return self.ty(e) };
        b.stmts.last()?;
        // infer from the tail by trying each type
        [NTy::I64, NTy::F64, NTy::Bool].into_iter().find(|t| self.value_of(e, *t))
    }
}

// ============================================================ codegen

pub struct NGen<'a> {
    pub sigs: &'a HashMap<String, NSig>,
    scopes: Vec<HashSet<String>>,
    defaults: HashMap<String, Vec<Param>>,
}

impl<'a> NGen<'a> {
    pub fn new(sigs: &'a HashMap<String, NSig>, p: &Program) -> Self {
        let mut defaults = HashMap::new();
        for d in &p.decls { if let Decl::Fun(f) = d { if sigs.contains_key(&f.name) { defaults.insert(f.name.clone(), f.params.clone()); } } }
        NGen { sigs, scopes: vec![], defaults }
    }

    /// native function + Value wrapper (`f_<name>`)
    pub fn function(&mut self, f: &FunDecl, wrapper: &str) -> String {
        let sig = self.sigs[&f.name].clone();
        self.scopes = vec![f.params.iter().map(|p| p.name.clone()).collect()];
        let ps: Vec<String> = f.params.iter().zip(&sig.params).map(|(p, t)| format!("mut {}: {}", rid(&p.name), t.rust())).collect();
        let mut b = String::new();
        for r in &f.requires {
            b.push_str(&format!("    if !({}) {{\n        rt::fail({:?});\n    }}\n", self.expr(&r.expr), contract_msg("precondition", &f.name, r)));
        }
        let body = self.expr(f.body.as_ref().unwrap());
        if f.ensures.is_empty() {
            b.push_str(&format!("    {}\n", body));
        } else {
            for (i, o) in f.olds.iter().enumerate() { b.push_str(&format!("    let __old{} = {};\n", i, self.expr(o))); }
            b.push_str(&format!("    let result: {} = (|| {} {{ {} }})();\n", sig.ret.rust(), format!("-> {}", sig.ret.rust()), body));
            self.scopes[0].insert("result".into());
            for e in &f.ensures {
                b.push_str(&format!("    if !({}) {{\n        rt::fail({:?});\n    }}\n", self.expr(&e.expr), contract_msg("postcondition", &f.name, e)));
            }
            b.push_str("    result\n");
        }
        let native = rid(&f.name);
        let wp: Vec<String> = f.params.iter().map(|p| format!("{}: Value", rid(&p.name))).collect();
        let wa: Vec<String> = f.params.iter().zip(&sig.params).map(|(p, t)| t.unbox(&rid(&p.name))).collect();
        format!(
            "/// native: `{}` compiled to plain Rust ({})\nfn {}({}) -> {} {{\n{}}}\nfn {}({}) -> Value {{\n    {}\n}}\n\n",
            f.name,
            sig.params.iter().map(|t| t.rust()).collect::<Vec<_>>().join(", ") + " -> " + sig.ret.rust(),
            native, ps.join(", "), sig.ret.rust(), b,
            wrapper, wp.join(", "), sig.ret.boxed(&format!("{}({})", native, wa.join(", ")))
        )
    }

    fn block(&mut self, b: &Block) -> String {
        self.scopes.push(HashSet::new());
        let mut parts = vec![];
        for (i, s) in b.stmts.iter().enumerate() {
            match s {
                Stmt::Expr(e) if i + 1 == b.stmts.len() => parts.push(self.expr(e)),
                _ => parts.push(self.stmt(s)),
            }
        }
        self.scopes.pop();
        format!("{{ {} }}", parts.join(" "))
    }

    fn stmt(&mut self, s: &Stmt) -> String {
        match s {
            Stmt::Let { name, init, .. } => { let v = self.expr(init); self.scopes.last_mut().unwrap().insert(name.clone()); format!("let mut {} = {};", rid(name), v) }
            Stmt::Assign { target, value, .. } => { let ExprKind::Ident(n) = &target.kind else { unreachable!() }; format!("{} = {};", rid(n), self.expr(value)) }
            Stmt::Expr(e) => match &e.kind {
                ExprKind::If(c, t, el) => {
                    let then = self.unit_block(t);
                    let els = el.as_ref().map(|b| format!(" else {}", self.unit_block(b))).unwrap_or_default();
                    format!("if {} {}{}", self.expr(c), then, els)
                }
                _ => format!("{};", self.expr(e)),
            },
            Stmt::Return(Some(x), _) => format!("return {};", self.expr(x)),
            Stmt::Return(None, _) => unreachable!(),
            Stmt::While(c, b) => format!("while {} {}", self.expr(c), self.unit_block(b)),
            Stmt::For(name, it, b) => {
                let ExprKind::Range(a, z) = &it.kind else { unreachable!() };
                let (a, z) = (self.expr(a), self.expr(z));
                self.scopes.push(HashSet::from([name.clone()]));
                let body = self.unit_block(b);
                self.scopes.pop();
                format!("for {} in {}..{} {}", rid(name), a, z, body)
            }
            Stmt::Break(_) => "break;".into(),
            Stmt::Continue(_) => "continue;".into(),
        }
    }
    fn unit_block(&mut self, b: &Block) -> String {
        self.scopes.push(HashSet::new());
        let parts: Vec<String> = b.stmts.iter().map(|s| { let x = self.stmt(s); if x.ends_with(';') || x.ends_with('}') { x } else { format!("{};", x) } }).collect();
        self.scopes.pop();
        format!("{{ {} }}", parts.join(" "))
    }

    /// operands of operators: nested operators get explicit parentheses
    fn operand(&mut self, e: &Expr) -> String {
        let s = self.expr(e);
        if matches!(e.kind, ExprKind::Binary(..) | ExprKind::Unary(..) | ExprKind::If(..)) { format!("({})", s) } else { s }
    }

    fn expr(&mut self, e: &Expr) -> String {
        use ExprKind as K;
        match &e.kind {
            K::Int(i) => if *i < 0 { format!("({}i64)", i) } else { format!("{}i64", i) },
            K::Float(f) => format!("{:?}f64", f),
            K::Bool(b) => b.to_string(),
            K::Ident(n) => rid(n),
            K::Result => "result".into(),
            K::Old(_, slot) => format!("__old{}", slot),
            K::Unary(op, x) => format!("{}{}", op, self.operand(x)),
            K::Binary(op, l, r) => format!("{} {} {}", self.operand(l), op, self.operand(r)),
            K::If(c, t, Some(el)) => format!("if {} {} else {}", self.expr(c), self.block(t), self.block(el)),
            K::Block(b) => self.block(b),
            K::Call(c, args) => {
                let K::Ident(n) = &c.kind else { unreachable!() };
                let params = self.defaults[n].clone();
                let ord = order_args(&params, args);
                let vals: Vec<String> = params.iter().enumerate().map(|(i, p)| match ord.get(i).copied().flatten() {
                    Some(x) => self.expr(x),
                    None => self.expr(p.default.as_ref().unwrap()),
                }).collect();
                format!("{}({})", rid(n), vals.join(", "))
            }
            _ => unreachable!("not in the native subset: {:?}", e.kind),
        }
    }
}
