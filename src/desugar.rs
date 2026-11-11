//! Desugaring: rewrites surface sugar into the core language before checking.
//!
//! * Sheets lazy functions: IF, IFS, AND, OR, NOT, LET, LAMBDA, SWITCH, IFERROR
//! * Clojure threading: `x -> f(a)` => `f(x, a)`, `x ->> f(a)` => `f(a, x)`
//! * Sheets percent: `15%` => `15 * 0.01`
//! * compound assignment: `x += 1` => `x = x + 1`
//! * Eiffel `old(e)` in ensure clauses gets a slot number; the expression is stored in `FunDecl::olds`

use crate::ast::*;
use crate::diag::{Diag, Span};

pub fn desugar(p: &mut Program) -> Result<(), Diag> {
    let mut globals = std::collections::HashSet::new();
    for d in &p.decls {
        match d {
            Decl::Fun(f) => { globals.insert(f.name.clone()); }
            Decl::Class(c) => { globals.insert(c.name.clone()); }
            Decl::Generic(g) => { globals.insert(g.name.clone()); }
            Decl::Enum(e) => for v in &e.variants { globals.insert(v.name.clone()); },
            _ => {}
        }
    }
    let mut d = Desugar { tmp: 0, globals };
    for decl in &mut p.decls {
        match decl {
            Decl::Fun(f) => d.fun(f)?,
            Decl::Class(c) => {
                for f in &mut c.methods { d.fun(f)?; }
                for fl in &mut c.fields { d.expr(&mut fl.init)?; }
                for inv in &mut c.invariants { d.expr(&mut inv.expr)?; }
                for p in &mut c.params { if let Some(e) = &mut p.default { d.expr(e)?; } }
                for s in &mut c.supers { if let Some(args) = &mut s.args { for a in args { d.expr(&mut a.value)?; } } }
            }
            Decl::Method(m) => d.expr(&mut m.body)?,
            _ => {}
        }
    }
    Ok(())
}

struct Desugar { tmp: usize, globals: std::collections::HashSet<String> }

fn boxed(e: Expr) -> Box<Expr> { Box::new(e) }

impl Desugar {
    fn fun(&mut self, f: &mut FunDecl) -> Result<(), Diag> {
        for p in &mut f.params { if let Some(e) = &mut p.default { self.expr(e)?; } }
        for c in &mut f.requires { self.expr(&mut c.expr)?; }
        for c in &mut f.ensures {
            self.expr(&mut c.expr)?;
            collect_olds(&mut c.expr, &mut f.olds);
        }
        if let Some(b) = &mut f.body { self.expr(b)?; }
        Ok(())
    }

    fn block(&mut self, b: &mut Block) -> Result<(), Diag> {
        for s in &mut b.stmts { self.stmt(s)?; }
        Ok(())
    }

    fn stmt(&mut self, s: &mut Stmt) -> Result<(), Diag> {
        match s {
            Stmt::Let { init, .. } => self.expr(init)?,
            Stmt::Assign { target, op, value, .. } => {
                self.expr(target)?;
                self.expr(value)?;
                if op != "=" {
                    let bop = &op[..1];
                    let span = target.span.to(value.span);
                    let v = std::mem::replace(value, Expr::new(ExprKind::Nil, span));
                    *value = Expr::new(ExprKind::Binary(bop.to_string(), boxed(target.clone()), boxed(v)), span);
                    *op = "=".into();
                }
            }
            Stmt::Expr(e) => self.expr(e)?,
            Stmt::Return(Some(e), _) => self.expr(e)?,
            Stmt::While(c, b) => { self.expr(c)?; self.block(b)?; }
            Stmt::For(_, it, b) => { self.expr(it)?; self.block(b)?; }
            _ => {}
        }
        Ok(())
    }

    fn fresh(&mut self) -> String { self.tmp += 1; format!("__t{}", self.tmp) }

    pub fn expr(&mut self, e: &mut Expr) -> Result<(), Diag> {
        // children first
        match &mut e.kind {
            ExprKind::Str(segs) => for s in segs { if let StrSeg::Expr(x) = s { self.expr(x)?; } },
            ExprKind::Old(x, _) | ExprKind::Unary(_, x) | ExprKind::Percent(x) | ExprKind::Try(x) | ExprKind::NotNull(x) | ExprKind::Is(x, _) | ExprKind::Quantity(x, _) => self.expr(x)?,
            ExprKind::Binary(_, l, r) | ExprKind::Index(l, r) | ExprKind::Range(l, r) => { self.expr(l)?; self.expr(r)?; }
            ExprKind::Thread { left, right, .. } => { self.expr(left)?; self.expr(right)?; }
            ExprKind::Call(c, args) => { self.expr(c)?; for a in args { self.expr(&mut a.value)?; } }
            ExprKind::Member { obj, .. } => self.expr(obj)?,
            ExprKind::Super { args, .. } => for a in args { self.expr(&mut a.value)?; },
            ExprKind::Lambda(ps, b) => { for p in ps { if let Some(d) = &mut p.default { self.expr(d)?; } } self.expr(b)?; }
            ExprKind::If(c, t, el) => { self.expr(c)?; self.block(t)?; if let Some(b) = el { self.block(b)?; } }
            ExprKind::Match(s, arms) => {
                self.expr(s)?;
                for a in arms { if let Some(g) = &mut a.guard { self.expr(g)?; } self.expr(&mut a.body)?; }
            }
            ExprKind::Vec(items) => for i in items { self.expr(i)?; },
            ExprKind::Map(es) => for en in es {
                match en { MapEntry::Spread(x) => self.expr(x)?, MapEntry::Kv(k, v) => { self.expr(k)?; self.expr(v)?; } }
            },
            ExprKind::Block(b) => self.block(b)?,
            ExprKind::Sheet(cells) => for c in cells { self.expr(&mut c.value)?; },
            _ => {}
        }
        let span = e.span;
        let kind = std::mem::replace(&mut e.kind, ExprKind::Nil);
        e.kind = match kind {
            ExprKind::Percent(x) => ExprKind::Binary("*".into(), x, boxed(Expr::new(ExprKind::Decimal("0.01".into()), span))),
            ExprKind::Thread { op, left, right } => {
                let right = *right;
                match right.kind {
                    // `x -> filter(f)` where `filter` is not a function: method call `x.filter(f)`
                    ExprKind::Call(callee, args) if matches!(&callee.kind, ExprKind::Ident(n) if !self.globals.contains(n) && !crate::sema::is_builtin(n) && !is_sheets_lazy(n) && n.starts_with(|c: char| c.is_ascii_lowercase())) => {
                        let ExprKind::Ident(n) = callee.kind else { unreachable!() };
                        let m = Expr::new(ExprKind::Member { obj: left, name: n, safe: false }, span);
                        ExprKind::Call(boxed(m), args)
                    }
                    ExprKind::Call(callee, mut args) => {
                        let a = Arg { name: None, value: *left };
                        if op == "->" { args.insert(0, a); } else { args.push(a); }
                        ExprKind::Call(callee, args)
                    }
                    _ => ExprKind::Call(boxed(right), vec![Arg { name: None, value: *left }]),
                }
            }
            ExprKind::Call(callee, args) => match &callee.kind {
                ExprKind::Ident(n) if is_sheets_lazy(n) => self.sheets_lazy(n.clone(), args, span)?,
                _ => ExprKind::Call(callee, args),
            },
            k => k,
        };
        Ok(())
    }

    fn sheets_lazy(&mut self, name: String, args: Vec<Arg>, span: Span) -> Result<ExprKind, Diag> {
        let mut vals: Vec<Expr> = args.into_iter().map(|a| a.value).collect();
        let blk = |e: Expr| Block { span: e.span, stmts: vec![Stmt::Expr(e)] };
        let arity = |n: usize, ok: bool| -> Result<(), Diag> {
            if ok { Ok(()) } else { Err(Diag::new(format!("{} expects {}", name, n), span)) }
        };
        Ok(match name.as_str() {
            "IF" => {
                if vals.len() != 2 && vals.len() != 3 { return Err(Diag::new("IF expects (condition, value_if_true, [value_if_false])", span)); }
                let els = if vals.len() == 3 { Some(blk(vals.pop().unwrap())) } else { Some(blk(Expr::new(ExprKind::Bool(false), span))) };
                let t = vals.pop().unwrap();
                let c = vals.pop().unwrap();
                ExprKind::If(boxed(c), blk(t), els)
            }
            "IFS" => {
                if vals.is_empty() || vals.len() % 2 != 0 { return Err(Diag::new("IFS expects condition/value pairs", span)); }
                let mut acc = Expr::new(ExprKind::Call(boxed(Expr::new(ExprKind::Ident("Err".into()), span)),
                    vec![Arg { name: None, value: Expr::new(ExprKind::Str(vec![StrSeg::Lit("#N/A".into())]), span) }]), span);
                while let (Some(v), Some(c)) = (vals.pop(), vals.pop()) {
                    acc = Expr::new(ExprKind::If(boxed(c), blk(v), Some(blk(acc))), span);
                }
                acc.kind
            }
            "AND" | "OR" => {
                arity(1, !vals.is_empty())?;
                let op = if name == "AND" { "&&" } else { "||" };
                let mut it = vals.into_iter();
                let mut acc = it.next().unwrap();
                for v in it { acc = Expr::new(ExprKind::Binary(op.into(), boxed(acc), boxed(v)), span); }
                acc.kind
            }
            "NOT" => { arity(1, vals.len() == 1)?; ExprKind::Unary("!".into(), boxed(vals.pop().unwrap())) }
            "LAMBDA" => {
                arity(1, !vals.is_empty())?;
                let body = vals.pop().unwrap();
                let mut params = vec![];
                for v in vals {
                    match v.kind {
                        ExprKind::Ident(n) => params.push(Param { name: n, ty: None, default: None, field: None, span: v.span }),
                        _ => return Err(Diag::new("LAMBDA parameters must be plain names", v.span)),
                    }
                }
                ExprKind::Lambda(params, boxed(body))
            }
            "LET" => {
                if vals.len() < 3 || vals.len() % 2 == 0 { return Err(Diag::new("LET expects (name1, value1, [name2, value2, ...], expression)", span)); }
                let body = vals.pop().unwrap();
                let mut stmts = vec![];
                let mut it = vals.into_iter();
                while let (Some(n), Some(v)) = (it.next(), it.next()) {
                    let ExprKind::Ident(name) = n.kind else { return Err(Diag::new("LET names must be plain names", n.span)) };
                    stmts.push(Stmt::Let { mutable: false, name, ty: None, init: v, span: n.span });
                }
                stmts.push(Stmt::Expr(body));
                ExprKind::Block(Block { stmts, span })
            }
            "SWITCH" => {
                if vals.len() < 3 { return Err(Diag::new("SWITCH expects (expression, case1, value1, ..., [default])", span)); }
                let t = self.fresh();
                let mut it = vals.into_iter();
                let subject = it.next().unwrap();
                let rest: Vec<Expr> = it.collect();
                let (pairs, default) = if rest.len() % 2 == 1 {
                    let mut r = rest; let d = r.pop().unwrap(); (r, d)
                } else {
                    (rest, Expr::new(ExprKind::Call(boxed(Expr::new(ExprKind::Ident("Err".into()), span)),
                        vec![Arg { name: None, value: Expr::new(ExprKind::Str(vec![StrSeg::Lit("#N/A".into())]), span) }]), span))
                };
                let mut acc = default;
                let mut pv = pairs;
                while let (Some(v), Some(c)) = (pv.pop(), pv.pop()) {
                    let cond = Expr::new(ExprKind::Binary("==".into(), boxed(Expr::new(ExprKind::Ident(t.clone()), span)), boxed(c)), span);
                    acc = Expr::new(ExprKind::If(boxed(cond), blk(v), Some(blk(acc))), span);
                }
                ExprKind::Block(Block { span, stmts: vec![
                    Stmt::Let { mutable: false, name: t, ty: None, init: subject, span },
                    Stmt::Expr(acc),
                ] })
            }
            "IFERROR" => {
                if vals.len() != 2 && vals.len() != 1 { return Err(Diag::new("IFERROR expects (value, [value_if_error])", span)); }
                let fb = if vals.len() == 2 { vals.pop().unwrap() } else { Expr::new(ExprKind::Str(vec![StrSeg::Lit(String::new())]), span) };
                let v = vals.pop().unwrap();
                let t = self.fresh();
                let tid = || Expr::new(ExprKind::Ident(t.clone()), span);
                let cond = Expr::new(ExprKind::Call(boxed(Expr::new(ExprKind::Ident("ISERROR".into()), span)), vec![Arg { name: None, value: tid() }]), span);
                ExprKind::Block(Block { span, stmts: vec![
                    Stmt::Let { mutable: false, name: t.clone(), ty: None, init: v, span },
                    Stmt::Expr(Expr::new(ExprKind::If(boxed(cond), blk(fb), Some(blk(tid()))), span)),
                ] })
            }
            _ => unreachable!(),
        })
    }
}

pub fn is_sheets_lazy(n: &str) -> bool {
    matches!(n, "IF" | "IFS" | "AND" | "OR" | "NOT" | "LAMBDA" | "LET" | "SWITCH" | "IFERROR")
}

fn collect_olds(e: &mut Expr, olds: &mut Vec<Expr>) {
    if let ExprKind::Old(inner, slot) = &mut e.kind {
        *slot = olds.len();
        olds.push((**inner).clone());
        return;
    }
    walk_mut(e, &mut |x| collect_olds(x, olds));
}

/// Apply `f` to each direct child expression.
pub fn walk_mut(e: &mut Expr, f: &mut dyn FnMut(&mut Expr)) {
    match &mut e.kind {
        ExprKind::Str(segs) => for s in segs { if let StrSeg::Expr(x) = s { f(x); } },
        ExprKind::Old(x, _) | ExprKind::Unary(_, x) | ExprKind::Percent(x) | ExprKind::Try(x) | ExprKind::NotNull(x) | ExprKind::Is(x, _) | ExprKind::Quantity(x, _) => f(x),
        ExprKind::Binary(_, l, r) | ExprKind::Index(l, r) | ExprKind::Range(l, r) => { f(l); f(r); }
        ExprKind::Thread { left, right, .. } => { f(left); f(right); }
        ExprKind::Call(c, args) => { f(c); for a in args { f(&mut a.value); } }
        ExprKind::Member { obj, .. } => f(obj),
        ExprKind::Super { args, .. } => for a in args { f(&mut a.value); },
        ExprKind::Lambda(_, b) => f(b),
        ExprKind::If(c, t, el) => { f(c); walk_block_mut(t, f); if let Some(b) = el { walk_block_mut(b, f); } }
        ExprKind::Match(s, arms) => { f(s); for a in arms { if let Some(g) = &mut a.guard { f(g); } f(&mut a.body); } }
        ExprKind::Vec(items) => for i in items { f(i); },
        ExprKind::Map(es) => for en in es { match en { MapEntry::Spread(x) => f(x), MapEntry::Kv(k, v) => { f(k); f(v); } } },
        ExprKind::Block(b) => walk_block_mut(b, f),
        ExprKind::Sheet(cells) => for c in cells { f(&mut c.value); },
        _ => {}
    }
}

fn walk_block_mut(b: &mut Block, f: &mut dyn FnMut(&mut Expr)) {
    for s in &mut b.stmts {
        match s {
            Stmt::Let { init, .. } => f(init),
            Stmt::Assign { target, value, .. } => { f(target); f(value); }
            Stmt::Expr(e) => f(e),
            Stmt::Return(Some(e), _) => f(e),
            Stmt::While(c, b) => { f(c); walk_block_mut(b, f); }
            Stmt::For(_, it, b) => { f(it); walk_block_mut(b, f); }
            _ => {}
        }
    }
}
