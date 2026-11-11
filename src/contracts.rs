//! Design by Contract, after semantic analysis:
//!
//! * **Contract inheritance** (Eiffel / Liskov): a method that overrides or implements an
//!   inherited method (including abstract trait methods) is checked against the contracts of every
//!   declaration of that method in its class's MRO.
//!     - effective precondition  = own `require else` OR each ancestor's precondition
//!       (an original declaration without `require` makes it trivially true);
//!     - effective postcondition = own `ensure then` AND each ancestor's postcondition.
//!   Ancestor contracts are rewritten to the overriding method's parameter names.
//! * **Monitoring levels** (`--contracts all|pre|none`): drop contracts at compile time.

use std::collections::HashMap;

use crate::ast::*;
use crate::diag::Span;
use crate::sema::Info;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Level { All, Pre, None }

impl Level {
    pub fn parse(s: &str) -> Option<Level> {
        match s { "all" => Some(Level::All), "pre" | "require" => Some(Level::Pre), "none" | "off" => Some(Level::None), _ => None }
    }
}

pub fn apply(p: &mut Program, info: &Info, level: Level) {
    let orig: HashMap<String, ClassDecl> = p.decls.iter().filter_map(|d| if let Decl::Class(c) = d { Some((c.name.clone(), c.clone())) } else { None }).collect();
    for decl in &mut p.decls {
        match decl {
            Decl::Class(c) => {
                if let Some(ci) = info.classes.get(&c.name) {
                    for m in &mut c.methods {
                        if m.body.is_some() { inherit(m, &c.name, &ci.mro, &orig); }
                    }
                }
                if level != Level::All { c.invariants.clear(); }
                for m in &mut c.methods { strip(m, level); }
            }
            Decl::Fun(f) => strip(f, level),
            _ => {}
        }
    }
}

fn strip(f: &mut FunDecl, level: Level) {
    if level != Level::All { f.ensures.clear(); f.olds.clear(); }
    if level == Level::None { f.requires.clear(); }
}

fn inherit(m: &mut FunDecl, cname: &str, mro: &[String], orig: &HashMap<String, ClassDecl>) {
    let chain: Vec<(String, FunDecl)> = mro.iter().skip(1)
        .filter_map(|a| orig.get(a).and_then(|c| c.methods.iter().find(|d| d.name == m.name)).map(|d| (a.clone(), d.clone())))
        .collect();
    if chain.is_empty() { return; }
    let _ = cname;

    // ---- preconditions: OR of the blocks
    let trivially_true = (!m.is_override && m.requires.is_empty()) || chain.iter().any(|(_, d)| !d.is_override && d.requires.is_empty());
    if trivially_true {
        m.requires.clear();
    } else {
        let mut blocks: Vec<(Expr, String)> = vec![];
        if !m.requires.is_empty() { blocks.push(conj(&m.requires, None, m)); }
        for (_, d) in &chain { if !d.requires.is_empty() { blocks.push(conj(&d.requires, Some(d), m)); } }
        let own_only = blocks.len() == 1 && !m.requires.is_empty();
        if !own_only {
            let span = blocks[0].0.span;
            let text = blocks.iter().map(|b| format!("({})", b.1)).collect::<Vec<_>>().join(" or ");
            let expr = blocks.into_iter().map(|b| b.0).reduce(|a, b| bin("||", a, b, span)).unwrap();
            m.requires = vec![Contract { tag: None, expr, text, origin: None }];
        }
    }

    // ---- postconditions: AND of everything, ancestors' after the method's own
    for (a, d) in &chain {
        let offset = m.olds.len();
        for o in &d.olds { m.olds.push(rename(o.clone(), d, m)); }
        for e in &d.ensures {
            let shifted = shift_olds(e.expr.clone(), offset);
            m.ensures.push(Contract { tag: e.tag.clone(), expr: rename(shifted, d, m), text: e.text.clone(), origin: Some(format!("{}.{}", a, d.name)) });
        }
    }
}

fn bin(op: &str, l: Expr, r: Expr, span: Span) -> Expr { Expr::new(ExprKind::Binary(op.into(), Box::new(l), Box::new(r)), span) }

/// AND of a contract block, renamed to `m`'s parameters when it comes from ancestor `d`
fn conj(cs: &[Contract], d: Option<&FunDecl>, m: &FunDecl) -> (Expr, String) {
    let span = cs[0].expr.span;
    let e = cs.iter().map(|c| c.expr.clone()).reduce(|a, b| bin("&&", a, b, span)).unwrap();
    let e = match d { Some(d) => rename(e, d, m), None => e };
    (e, cs.iter().map(|c| c.text.clone()).collect::<Vec<_>>().join(" && "))
}

/// Evaluate an ancestor's contract under the overriding method's parameter names:
/// `{ val ancestorParam = ownParam; expr }` (only for names that differ).
fn rename(e: Expr, d: &FunDecl, m: &FunDecl) -> Expr {
    let mut stmts = vec![];
    for (dp, mp) in d.params.iter().zip(m.params.iter()) {
        if dp.name != mp.name {
            stmts.push(Stmt::Let { mutable: false, name: dp.name.clone(), ty: None, init: Expr::new(ExprKind::Ident(mp.name.clone()), e.span), span: e.span });
        }
    }
    if stmts.is_empty() { return e; }
    let span = e.span;
    stmts.push(Stmt::Expr(e));
    Expr::new(ExprKind::Block(Block { stmts, span }), span)
}

fn shift_olds(mut e: Expr, offset: usize) -> Expr {
    fn go(e: &mut Expr, offset: usize) {
        if let ExprKind::Old(_, slot) = &mut e.kind { *slot += offset; return; }
        crate::desugar::walk_mut(e, &mut |x| go(x, offset));
    }
    if offset > 0 { go(&mut e, offset); }
    e
}
