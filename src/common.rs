//! Helpers shared by the Rust and JavaScript backends.

use std::collections::{HashMap, HashSet};

use crate::ast::*;
use crate::sema::Info;

/// Lookup tables over the AST that both code generators need.
pub struct Decls<'a> {
    pub funs: HashMap<String, &'a FunDecl>,
    pub classes: HashMap<String, &'a ClassDecl>,
    pub enums: Vec<&'a EnumDecl>,
    pub methods: Vec<&'a MethodDecl>,
}

impl<'a> Decls<'a> {
    pub fn new(p: &'a Program) -> Decls<'a> {
        let mut d = Decls { funs: HashMap::new(), classes: HashMap::new(), enums: vec![], methods: vec![] };
        for decl in &p.decls {
            match decl {
                Decl::Fun(f) => { d.funs.insert(f.name.clone(), f); }
                Decl::Class(c) => { d.classes.insert(c.name.clone(), c); }
                Decl::Enum(e) => d.enums.push(e),
                Decl::Method(m) => d.methods.push(m),
                _ => {}
            }
        }
        d
    }

    /// The method declaration visible on `class` (through its MRO).
    pub fn method(&self, info: &Info, class: &str, name: &str) -> Option<&'a FunDecl> {
        let ci = info.classes.get(class)?;
        let owner = &ci.methods.get(name)?.owner;
        self.classes.get(owner)?.methods.iter().find(|m| m.name == name)
    }
}

/// Order call arguments by parameter; `None` means "use the default".
pub fn order_args<'e>(params: &[Param], args: &'e [Arg]) -> Vec<Option<&'e Expr>> {
    let mut out: Vec<Option<&Expr>> = vec![None; params.len().max(args.iter().filter(|a| a.name.is_none()).count())];
    let mut pos = 0;
    for a in args {
        match &a.name {
            Some(n) => if let Some(i) = params.iter().position(|p| &p.name == n) { out[i] = Some(&a.value); },
            None => { if pos < out.len() { out[pos] = Some(&a.value); } pos += 1; }
        }
    }
    out
}

/// Names referenced anywhere inside an expression (used to compute closure captures).
pub fn free_names(e: &Expr, out: &mut HashSet<String>, uses_this: &mut bool) {
    use ExprKind as K;
    match &e.kind {
        K::Ident(n) => { out.insert(n.clone()); *uses_this = true; } // bare names may be fields
        K::This | K::Super { .. } => *uses_this = true,
        K::Call(c, _) => { if let K::Ident(n) = &c.kind { out.insert(n.clone()); if n == "callNext" { out.insert("__args".into()); } } *uses_this = true; }
        _ => {}
    }
    walk(e, &mut |x| free_names(x, out, uses_this));
}

fn walk(e: &Expr, f: &mut dyn FnMut(&Expr)) {
    use ExprKind as K;
    let blk = |b: &Block, f: &mut dyn FnMut(&Expr)| { for s in &b.stmts { walk_stmt(s, f); } };
    match &e.kind {
        K::Str(segs) => for s in segs { if let StrSeg::Expr(x) = s { f(x); } },
        K::Old(x, _) | K::Unary(_, x) | K::Percent(x) | K::Try(x) | K::NotNull(x) | K::Is(x, _) | K::Quantity(x, _) => f(x),
        K::Binary(_, l, r) | K::Index(l, r) | K::Range(l, r) => { f(l); f(r); }
        K::Thread { left, right, .. } => { f(left); f(right); }
        K::Call(c, args) => { f(c); for a in args { f(&a.value); } }
        K::Member { obj, .. } => f(obj),
        K::Super { args, .. } => for a in args { f(&a.value); },
        K::Lambda(_, b) => f(b),
        K::If(c, t, el) => { f(c); blk(t, f); if let Some(b) = el { blk(b, f); } }
        K::Match(s, arms) => { f(s); for a in arms { if let Some(g) = &a.guard { f(g); } f(&a.body); } }
        K::Vec(items) => for i in items { f(i); },
        K::Map(es) => for en in es { match en { MapEntry::Spread(x) => f(x), MapEntry::Kv(k, v) => { f(k); f(v); } } },
        K::Block(b) => blk(b, f),
        K::Sheet(cells) => for c in cells { f(&c.value); },
        _ => {}
    }
}

fn walk_stmt(s: &Stmt, f: &mut dyn FnMut(&Expr)) {
    match s {
        Stmt::Let { init, .. } => f(init),
        Stmt::Assign { target, value, .. } => { f(target); f(value); }
        Stmt::Expr(x) => f(x),
        Stmt::Return(Some(x), _) => f(x),
        Stmt::While(c, b) => { f(c); for s2 in &b.stmts { walk_stmt(s2, f); } }
        Stmt::For(_, it, b) => { f(it); for s2 in &b.stmts { walk_stmt(s2, f); } }
        _ => {}
    }
}

/// Does a sheet cell value reference other cells (=> it is a formula, recomputed on read)?
pub fn is_formula(e: &Expr) -> bool {
    let mut found = false;
    fn go(e: &Expr, found: &mut bool) {
        if matches!(e.kind, ExprKind::CellRef(_) | ExprKind::CellRange(..) | ExprKind::Call(..) | ExprKind::Ident(_)) { *found = true; return; }
        walk(e, &mut |x| go(x, found));
    }
    go(e, &mut found);
    found
}

pub fn contract_msg(kind: &str, where_: &str, c: &Contract) -> String {
    match &c.tag {
        Some(t) => format!("{} '{}' violated in {}: {}", kind, t, where_, c.text),
        None => format!("{} violated in {}: {}", kind, where_, c.text),
    }
}
