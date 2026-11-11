//! Chimera recursive-descent parser.

use crate::ast::*;
use crate::diag::{Diag, Span};
use crate::lexer::{lex, StrPart, Tk, Token};

type PResult<T> = Result<T, Diag>;

pub fn is_cell(s: &str) -> bool {
    let letters = s.chars().take_while(|c| c.is_ascii_uppercase()).count();
    letters >= 1 && letters <= 3 && s.len() > letters && s[letters..].chars().all(|c| c.is_ascii_digit())
}

pub fn parse(src: &str) -> PResult<Program> {
    let mut p = Parser { t: lex(src, 0)?, i: 0, src, in_sheet: false };
    let mut decls = Vec::new();
    while !p.at_eof() {
        if p.is_op(";") { p.bump(); continue; }
        decls.push(p.decl()?);
    }
    Ok(Program { decls })
}

struct Parser<'a> {
    t: Vec<Token>,
    i: usize,
    src: &'a str,
    in_sheet: bool,
}

impl<'a> Parser<'a> {
    // ---------- token helpers ----------
    fn cur(&self) -> &Token { &self.t[self.i] }
    fn peek(&self, n: usize) -> &Token { &self.t[(self.i + n).min(self.t.len() - 1)] }
    fn at_eof(&self) -> bool { matches!(self.cur().tk, Tk::Eof) }
    fn tok_is_op(t: &Token, v: &str) -> bool { matches!(&t.tk, Tk::Op(o) if *o == v) }
    fn is_op(&self, v: &str) -> bool { Self::tok_is_op(self.cur(), v) }
    fn is_kw(&self, v: &str) -> bool { matches!(&self.cur().tk, Tk::Kw(k) if *k == v) }
    fn bump(&mut self) -> Token { let t = self.cur().clone(); if self.i < self.t.len() - 1 { self.i += 1; } t }
    fn here(&self) -> Span { self.cur().span }
    fn prev_end(&self) -> usize { self.t[self.i.saturating_sub(1)].span.end }
    fn sp(&self, start: Span) -> Span { Span::new(start.start, self.prev_end().max(start.start)) }
    fn desc(&self) -> String {
        match &self.cur().tk {
            Tk::Ident(s) => s.clone(), Tk::Kw(k) => k.to_string(), Tk::Int(s) | Tk::Float(s) => s.clone(),
            Tk::Decimal(s) => s.clone(), Tk::Str(_) => "string".into(), Tk::Op(o) => o.to_string(), Tk::Eof => "end of file".into(),
        }
    }
    fn err<T>(&self, msg: impl Into<String>) -> PResult<T> { Err(Diag::new(msg, self.here())) }
    fn expect_op(&mut self, v: &str) -> PResult<Token> {
        if !self.is_op(v) { return self.err(format!("expected '{}' but found '{}'", v, self.desc())); }
        Ok(self.bump())
    }
    fn expect_kw(&mut self, v: &str) -> PResult<Token> {
        if !self.is_kw(v) { return self.err(format!("expected '{}' but found '{}'", v, self.desc())); }
        Ok(self.bump())
    }
    fn ident(&mut self) -> PResult<String> {
        if let Tk::Ident(s) = &self.cur().tk { let s = s.clone(); self.bump(); return Ok(s); }
        self.err(format!("expected identifier but found '{}'", self.desc()))
    }
    /// identifier or soft keyword (member names, record keys)
    fn name(&mut self) -> PResult<String> {
        match &self.cur().tk {
            Tk::Ident(s) => { let s = s.clone(); self.bump(); Ok(s) }
            Tk::Kw(k) => { let s = k.to_string(); self.bump(); Ok(s) }
            _ => self.err(format!("expected name but found '{}'", self.desc())),
        }
    }
    fn cur_ident(&self) -> Option<&str> { if let Tk::Ident(s) = &self.cur().tk { Some(s) } else { None } }

    // ---------- declarations ----------
    fn decl(&mut self) -> PResult<Decl> {
        if self.is_kw("fun") { return Ok(Decl::Fun(self.fun(false)?)); }
        if self.is_kw("class") || self.is_kw("data") || self.is_kw("abstract") || self.is_kw("open") || self.is_kw("trait") {
            return Ok(Decl::Class(self.class_decl()?));
        }
        if self.is_kw("enum") { return Ok(Decl::Enum(self.enum_decl()?)); }
        if self.is_kw("type") { return Ok(Decl::TypeAlias(self.type_alias()?)); }
        if self.is_kw("struct") { return Ok(Decl::TypeAlias(self.struct_decl()?)); }
        if self.is_kw("generic") { return Ok(Decl::Generic(self.generic_decl()?)); }
        if self.is_kw("method") || self.is_kw("before") || self.is_kw("after") || self.is_kw("around") {
            return Ok(Decl::Method(self.method_decl()?));
        }
        self.err(format!("expected a declaration (fun, class, trait, enum, type, struct, generic, method) but found '{}'", self.desc()))
    }

    fn type_params(&mut self) -> PResult<Vec<String>> {
        let mut out = vec![];
        if self.is_op("<") {
            self.bump();
            while !self.is_op(">") { out.push(self.ident()?); if self.is_op(",") { self.bump(); } }
            self.bump();
        }
        Ok(out)
    }

    fn params(&mut self, allow_field: bool) -> PResult<Vec<Param>> {
        self.expect_op("(")?;
        let mut ps = vec![];
        while !self.is_op(")") {
            let st = self.here();
            let mut field = None;
            if allow_field && (self.is_kw("val") || self.is_kw("var")) { field = Some(self.is_kw("var")); self.bump(); }
            let name = self.ident()?;
            let ty = if self.is_op(":") { self.bump(); Some(self.ty()?) } else { None };
            let default = if self.is_op("=") { self.bump(); Some(self.expr()?) } else { None };
            ps.push(Param { name, ty, default, field, span: self.sp(st) });
            if self.is_op(",") { self.bump(); } else if !self.is_op(")") { return self.err("expected ',' or ')' in parameter list"); }
        }
        self.bump();
        Ok(ps)
    }

    fn contracts(&mut self, kw: &str) -> PResult<Vec<Contract>> {
        self.expect_kw(kw)?;
        let mut out = vec![];
        loop {
            let mut tag = None;
            if self.cur_ident().is_some() && Self::tok_is_op(self.peek(1), ":") { tag = Some(self.ident()?); self.bump(); }
            let es = self.here();
            let expr = self.expr()?;
            let text = self.src[es.start..self.prev_end()].to_string();
            out.push(Contract { tag, expr, text });
            if self.is_op(",") { self.bump(); } else { break; }
        }
        Ok(out)
    }

    fn fun(&mut self, is_override: bool) -> PResult<FunDecl> {
        let st = self.expect_kw("fun")?.span;
        let _tp = self.type_params()?;
        let name = self.name()?;
        let params = self.params(false)?;
        let ret = if self.is_op(":") { self.bump(); Some(self.ty()?) } else { None };
        let mut requires = vec![];
        let mut ensures = vec![];
        while self.is_kw("require") || self.is_kw("ensure") {
            if self.is_kw("require") { requires.extend(self.contracts("require")?); } else { ensures.extend(self.contracts("ensure")?); }
        }
        let body = if self.is_op("=") {
            self.bump();
            Some(self.expr()?)
        } else if self.is_op("{") {
            let bs = self.here();
            let b = self.block()?;
            Some(Expr::new(ExprKind::Block(b), self.sp(bs)))
        } else { None };
        Ok(FunDecl { name, params, ret, requires, ensures, body, is_override, span: self.sp(st), olds: vec![] })
    }

    fn class_decl(&mut self) -> PResult<ClassDecl> {
        let st = self.here();
        let (mut is_data, mut is_abstract, mut is_trait) = (false, false, false);
        loop {
            if self.is_kw("data") { is_data = true; self.bump(); }
            else if self.is_kw("abstract") { is_abstract = true; self.bump(); }
            else if self.is_kw("open") { self.bump(); }
            else { break; }
        }
        if self.is_kw("trait") { is_trait = true; self.bump(); } else { self.expect_kw("class")?; }
        let name = self.ident()?;
        let params = if self.is_op("(") { self.params(true)? } else { vec![] };
        if is_trait && !params.is_empty() { return self.err(format!("trait '{}' cannot have constructor parameters", name)); }
        let mut supers = vec![];
        if self.is_op(":") {
            self.bump();
            loop {
                let ss = self.here();
                let sname = self.ident()?;
                let args = if self.is_op("(") && !self.cur().nl_before { Some(self.args()?) } else { None };
                supers.push(SuperRef { name: sname, args, span: self.sp(ss) });
                if self.is_op(",") { self.bump(); } else { break; }
            }
        }
        let mut c = ClassDecl { name: name.clone(), is_trait, is_data, is_abstract: is_abstract || is_trait, params, supers,
            methods: vec![], fields: vec![], invariants: vec![], span: st };
        if self.is_op("{") {
            self.bump();
            while !self.is_op("}") {
                if self.at_eof() { return self.err("unterminated class body"); }
                if self.is_op(";") { self.bump(); continue; }
                if self.is_kw("override") { self.bump(); c.methods.push(self.fun(true)?); continue; }
                if self.is_kw("abstract") {
                    self.bump();
                    let f = self.fun(false)?;
                    if f.body.is_some() { return Err(Diag::new(format!("abstract fun '{}' cannot have a body", f.name), f.span)); }
                    c.methods.push(f);
                    continue;
                }
                if self.is_kw("fun") { c.methods.push(self.fun(false)?); continue; }
                if self.is_kw("invariant") { let inv = self.contracts("invariant")?; c.invariants.extend(inv); continue; }
                if self.is_kw("val") || self.is_kw("var") {
                    let fs = self.here();
                    let mutable = self.is_kw("var");
                    self.bump();
                    let fname = self.ident()?;
                    let ty = if self.is_op(":") { self.bump(); Some(self.ty()?) } else { None };
                    self.expect_op("=")?;
                    let init = self.expr()?;
                    if is_trait { return Err(Diag::new(format!("trait '{}' cannot declare state (field '{}')", name, fname), fs)); }
                    c.fields.push(FieldDecl { mutable, name: fname, ty, init, span: self.sp(fs) });
                    continue;
                }
                return self.err(format!("unexpected '{}' in {} body", self.desc(), if is_trait { "trait" } else { "class" }));
            }
            self.bump();
        }
        c.span = self.sp(st);
        Ok(c)
    }

    fn enum_decl(&mut self) -> PResult<EnumDecl> {
        let st = self.expect_kw("enum")?.span;
        let name = self.ident()?;
        self.expect_op("{")?;
        let mut variants = vec![];
        while !self.is_op("}") {
            if self.is_op(",") || self.is_op(";") { self.bump(); continue; }
            let vs = self.here();
            let vname = self.ident()?;
            let fields = if self.is_op("(") { self.params(false)? } else { vec![] };
            variants.push(Variant { name: vname, fields, span: self.sp(vs) });
        }
        self.bump();
        Ok(EnumDecl { name, variants, span: self.sp(st) })
    }

    fn type_alias(&mut self) -> PResult<TypeAlias> {
        let st = self.expect_kw("type")?.span;
        let name = self.ident()?;
        let type_params = self.type_params()?;
        self.expect_op("=")?;
        let ty = self.ty()?;
        Ok(TypeAlias { name, type_params, ty, span: self.sp(st) })
    }

    fn struct_decl(&mut self) -> PResult<TypeAlias> {
        let st = self.expect_kw("struct")?.span;
        let name = self.ident()?;
        let type_params = self.type_params()?;
        let ty = self.record_type()?;
        Ok(TypeAlias { name, type_params, ty, span: self.sp(st) })
    }

    fn generic_decl(&mut self) -> PResult<GenericDecl> {
        let st = self.expect_kw("generic")?.span;
        let name = self.ident()?;
        let params = self.params(false)?;
        if self.is_op(":") { self.bump(); self.ty()?; }
        Ok(GenericDecl { name, params, span: self.sp(st) })
    }

    fn method_decl(&mut self) -> PResult<MethodDecl> {
        let t = self.bump();
        let qualifier = match t.tk { Tk::Kw("method") => "primary", Tk::Kw(k) => k, _ => unreachable!() }.to_string();
        let name = self.ident()?;
        let params = self.params(false)?;
        if self.is_op(":") { self.bump(); self.ty()?; }
        let body = if self.is_op("=") {
            self.bump();
            self.expr()?
        } else {
            let bs = self.here();
            let b = self.block()?;
            Expr::new(ExprKind::Block(b), self.sp(bs))
        };
        Ok(MethodDecl { qualifier, name, params, body, span: self.sp(t.span) })
    }

    // ---------- types ----------
    fn ty(&mut self) -> PResult<TypeNode> {
        let first = self.postfix_ty()?;
        if !self.is_op("|") { return Ok(first); }
        let mut opts = vec![first];
        while self.is_op("|") { self.bump(); opts.push(self.postfix_ty()?); }
        Ok(TypeNode::Union(opts))
    }
    fn postfix_ty(&mut self) -> PResult<TypeNode> {
        let mut t = self.primary_ty()?;
        while self.is_op("?") && !self.cur().nl_before { self.bump(); t = TypeNode::Nullable(Box::new(t)); }
        Ok(t)
    }
    fn primary_ty(&mut self) -> PResult<TypeNode> {
        let st = self.here();
        if self.is_op("(") {
            self.bump();
            let mut ps = vec![];
            while !self.is_op(")") { ps.push(self.ty()?); if self.is_op(",") { self.bump(); } }
            self.bump();
            if self.is_op("->") { self.bump(); let r = self.ty()?; return Ok(TypeNode::Fn(ps, Box::new(r))); }
            if ps.len() != 1 { return self.err("expected '->' after function type parameters"); }
            return Ok(ps.pop().unwrap());
        }
        if self.is_op("{") { return self.record_type(); }
        if let Tk::Str(parts) = &self.cur().tk {
            let s: String = parts.iter().map(|p| if let StrPart::Lit(l) = p { l.as_str() } else { "" }).collect();
            self.bump();
            return Ok(TypeNode::StrLit(s));
        }
        if self.is_kw("keyof") { self.bump(); let inner = self.postfix_ty()?; return Ok(TypeNode::KeyOf(Box::new(inner))); }
        let name = self.ident()?;
        let mut args = vec![];
        if self.is_op("<") {
            self.bump();
            while !self.is_op(">") {
                args.push(self.ty()?);
                if self.is_op(",") { self.bump(); } else if !self.is_op(">") { return self.err("expected ',' or '>' in type arguments"); }
            }
            self.bump();
        }
        Ok(TypeNode::Name { name, args, span: self.sp(st) })
    }
    fn record_type(&mut self) -> PResult<TypeNode> {
        self.expect_op("{")?;
        let mut fields = vec![];
        while !self.is_op("}") {
            if self.is_op(",") || self.is_op(";") { self.bump(); continue; }
            let mut readonly = false;
            if self.cur_ident() == Some("readonly") && matches!(self.peek(1).tk, Tk::Ident(_)) { readonly = true; self.bump(); }
            let name = self.name()?;
            let mut optional = false;
            if self.is_op("?") { optional = true; self.bump(); }
            self.expect_op(":")?;
            fields.push(RecordField { name, ty: self.ty()?, optional, readonly });
        }
        self.bump();
        Ok(TypeNode::Record(fields))
    }

    // ---------- statements ----------
    fn block(&mut self) -> PResult<Block> {
        let st = self.expect_op("{")?.span;
        let mut stmts = vec![];
        while !self.is_op("}") {
            if self.is_op(";") { self.bump(); continue; }
            if self.at_eof() { return self.err("unterminated block"); }
            stmts.push(self.stmt()?);
        }
        self.bump();
        Ok(Block { stmts, span: self.sp(st) })
    }

    fn stmt(&mut self) -> PResult<Stmt> {
        let st = self.here();
        if self.is_kw("val") || self.is_kw("var") {
            let mutable = self.is_kw("var");
            self.bump();
            let name = self.ident()?;
            let ty = if self.is_op(":") { self.bump(); Some(self.ty()?) } else { None };
            self.expect_op("=")?;
            let init = self.expr()?;
            return Ok(Stmt::Let { mutable, name, ty, init, span: self.sp(st) });
        }
        if self.is_kw("return") {
            self.bump();
            let v = if !self.cur().nl_before && !self.is_op("}") && !self.is_op(";") { Some(self.expr()?) } else { None };
            return Ok(Stmt::Return(v, self.sp(st)));
        }
        if self.is_kw("while") {
            self.bump();
            let c = self.expr()?;
            let b = self.block()?;
            return Ok(Stmt::While(c, b));
        }
        if self.is_kw("for") {
            self.bump();
            let paren = self.is_op("(");
            if paren { self.bump(); }
            let name = self.ident()?;
            self.expect_kw("in")?;
            let it = self.expr()?;
            if paren { self.expect_op(")")?; }
            let b = self.block()?;
            return Ok(Stmt::For(name, it, b));
        }
        if self.is_kw("break") { self.bump(); return Ok(Stmt::Break(st)); }
        if self.is_kw("continue") { self.bump(); return Ok(Stmt::Continue(st)); }
        let e = self.expr()?;
        for op in ["=", "+=", "-=", "*=", "/="] {
            if self.is_op(op) {
                self.bump();
                let value = self.expr()?;
                if !matches!(e.kind, ExprKind::Ident(_) | ExprKind::Member { .. } | ExprKind::Index(..)) {
                    return Err(Diag::new("invalid assignment target", e.span));
                }
                return Ok(Stmt::Assign { target: e, op: op.to_string(), value, span: self.sp(st) });
            }
        }
        Ok(Stmt::Expr(e))
    }

    // ---------- expressions ----------
    pub fn expr(&mut self) -> PResult<Expr> { self.thread() }

    /// Binary operators must sit on the same line as their left operand, except
    /// chain operators (-> ->> ?: && ||) which may lead a continuation line.
    fn bin_op_here(&self, ops: &[&'static str], leading_ok: bool) -> Option<&'static str> {
        if let Tk::Op(o) = &self.cur().tk {
            if ops.contains(o) && (leading_ok || !self.cur().nl_before) { return Some(o); }
        }
        None
    }
    fn mk_bin(&self, op: &str, l: Expr, r: Expr) -> Expr {
        let span = l.span.to(r.span);
        let op = if op == "<>" { "!=" } else { op };
        Expr::new(ExprKind::Binary(op.to_string(), Box::new(l), Box::new(r)), span)
    }
    fn thread(&mut self) -> PResult<Expr> {
        let mut l = self.or()?;
        while let Some(op) = self.bin_op_here(&["->", "->>"], true) {
            self.bump();
            let r = self.or()?;
            if !matches!(r.kind, ExprKind::Call(..) | ExprKind::Ident(_) | ExprKind::Lambda(..)) {
                return Err(Diag::new(format!("right side of '{}' must be a function call or function", op), r.span));
            }
            let span = l.span.to(r.span);
            l = Expr::new(ExprKind::Thread { op: op.to_string(), left: Box::new(l), right: Box::new(r) }, span);
        }
        Ok(l)
    }
    fn or(&mut self) -> PResult<Expr> {
        let mut l = self.and()?;
        while self.bin_op_here(&["||"], true).is_some() { self.bump(); let r = self.and()?; l = self.mk_bin("||", l, r); }
        Ok(l)
    }
    fn and(&mut self) -> PResult<Expr> {
        let mut l = self.equality()?;
        while self.bin_op_here(&["&&"], true).is_some() { self.bump(); let r = self.equality()?; l = self.mk_bin("&&", l, r); }
        Ok(l)
    }
    fn equality(&mut self) -> PResult<Expr> {
        let mut l = self.comparison()?;
        while let Some(op) = self.bin_op_here(&["==", "!=", "<>"], false) { self.bump(); let r = self.comparison()?; l = self.mk_bin(op, l, r); }
        Ok(l)
    }
    fn comparison(&mut self) -> PResult<Expr> {
        let mut l = self.concat()?;
        loop {
            if self.is_kw("is") && !self.cur().nl_before {
                self.bump();
                let t = self.ty()?;
                let span = self.sp(l.span);
                l = Expr::new(ExprKind::Is(Box::new(l), t), span);
                continue;
            }
            match self.bin_op_here(&["<", "<=", ">", ">="], false) {
                Some(op) => { self.bump(); let r = self.concat()?; l = self.mk_bin(op, l, r); }
                None => break,
            }
        }
        Ok(l)
    }
    /// Google Sheets string concatenation: "a" & 1 & true
    fn concat(&mut self) -> PResult<Expr> {
        let mut l = self.elvis()?;
        while self.bin_op_here(&["&"], false).is_some() { self.bump(); let r = self.elvis()?; l = self.mk_bin("&", l, r); }
        Ok(l)
    }
    fn elvis(&mut self) -> PResult<Expr> {
        let mut l = self.range()?;
        while self.bin_op_here(&["?:"], true).is_some() { self.bump(); let r = self.range()?; l = self.mk_bin("?:", l, r); }
        Ok(l)
    }
    fn range(&mut self) -> PResult<Expr> {
        let l = self.additive()?;
        if self.bin_op_here(&[".."], false).is_some() {
            self.bump();
            let r = self.additive()?;
            let span = l.span.to(r.span);
            return Ok(Expr::new(ExprKind::Range(Box::new(l), Box::new(r)), span));
        }
        Ok(l)
    }
    fn additive(&mut self) -> PResult<Expr> {
        let mut l = self.multiplicative()?;
        while let Some(op) = self.bin_op_here(&["+", "-"], false) { self.bump(); let r = self.multiplicative()?; l = self.mk_bin(op, l, r); }
        Ok(l)
    }
    fn multiplicative(&mut self) -> PResult<Expr> {
        let mut l = self.power()?;
        loop {
            let Some(op) = self.bin_op_here(&["*", "/", "%"], false) else { break };
            if op == "%" && !self.can_start_operand(self.peek(1)) { break; } // postfix percent
            self.bump();
            let r = self.power()?;
            l = self.mk_bin(op, l, r);
        }
        Ok(l)
    }
    fn power(&mut self) -> PResult<Expr> {
        let l = self.unary()?;
        if self.bin_op_here(&["^"], false).is_some() { self.bump(); let r = self.power()?; return Ok(self.mk_bin("^", l, r)); }
        Ok(l)
    }
    fn unary(&mut self) -> PResult<Expr> {
        let st = self.here();
        if self.is_op("-") || self.is_op("!") || self.is_op("+") {
            let op = if let Tk::Op(o) = self.bump().tk { o } else { unreachable!() };
            let e = self.unary()?;
            if op == "+" { return Ok(e); }
            if op == "-" {
                if let ExprKind::Int(v) = e.kind { return Ok(Expr::new(ExprKind::Int(-v), self.sp(st))); }
                if let ExprKind::Float(v) = e.kind { return Ok(Expr::new(ExprKind::Float(-v), self.sp(st))); }
                if let ExprKind::Decimal(v) = &e.kind { return Ok(Expr::new(ExprKind::Decimal(format!("-{}", v)), self.sp(st))); }
            }
            return Ok(Expr::new(ExprKind::Unary(op.to_string(), Box::new(e)), self.sp(st)));
        }
        if self.is_op("@") {
            // Clojure deref: @atom
            self.bump();
            let e = self.unary()?;
            let sp = self.sp(st);
            let m = Expr::new(ExprKind::Member { obj: Box::new(e), name: "deref".into(), safe: false }, sp);
            return Ok(Expr::new(ExprKind::Call(Box::new(m), vec![]), sp));
        }
        self.postfix()
    }

    fn can_start_operand(&self, t: &Token) -> bool {
        if t.nl_before { return false; }
        match &t.tk {
            Tk::Ident(_) | Tk::Int(_) | Tk::Float(_) | Tk::Decimal(_) | Tk::Str(_) => true,
            Tk::Kw(k) => ["true", "false", "nil", "this", "result", "old", "if", "match", "sheet", "super"].contains(k),
            Tk::Op(o) => ["(", "[", "{", "|", "-", "!", "@"].contains(o),
            Tk::Eof => false,
        }
    }

    fn postfix(&mut self) -> PResult<Expr> {
        let st = self.here();
        let mut e = self.primary()?;
        loop {
            if self.is_op("(") && !self.cur().nl_before {
                let args = self.args()?;
                e = Expr::new(ExprKind::Call(Box::new(e), args), self.sp(st));
            } else if self.is_op("[") && !self.cur().nl_before {
                self.bump();
                let idx = self.expr()?;
                self.expect_op("]")?;
                e = Expr::new(ExprKind::Index(Box::new(e), Box::new(idx)), self.sp(st));
            } else if self.is_op(".") || self.is_op("?.") {
                let safe = self.is_op("?.");
                self.bump();
                let name = self.name()?;
                e = Expr::new(ExprKind::Member { obj: Box::new(e), name, safe }, self.sp(st));
            } else if self.is_op("!!") && !self.cur().nl_before {
                self.bump();
                e = Expr::new(ExprKind::NotNull(Box::new(e)), self.sp(st));
            } else if self.is_op("?") && !self.cur().nl_before {
                self.bump();
                e = Expr::new(ExprKind::Try(Box::new(e)), self.sp(st));
            } else if self.is_op("%") && !self.cur().nl_before && !self.can_start_operand(self.peek(1)) {
                // Google Sheets percent literal: 15% == 0.15
                self.bump();
                e = Expr::new(ExprKind::Percent(Box::new(e)), self.sp(st));
            } else {
                break;
            }
        }
        Ok(e)
    }

    fn args(&mut self) -> PResult<Vec<Arg>> {
        self.expect_op("(")?;
        let mut args = vec![];
        while !self.is_op(")") {
            if self.cur_ident().is_some() && Self::tok_is_op(self.peek(1), "=") {
                let name = self.ident()?;
                self.bump();
                args.push(Arg { name: Some(name), value: self.expr()? });
            } else {
                args.push(Arg { name: None, value: self.expr()? });
            }
            if self.is_op(",") || self.is_op(";") { self.bump(); } else if !self.is_op(")") { return self.err("expected ',' or ')' in argument list"); }
        }
        self.bump();
        Ok(args)
    }

    /// After `=>` or a lambda head, `{` opens a block unless it looks like a map literal.
    fn looks_like_map(&self) -> bool {
        let a = self.peek(1);
        let b = self.peek(2);
        if Self::tok_is_op(a, "}") || Self::tok_is_op(a, "...") { return true; }
        matches!(a.tk, Tk::Ident(_) | Tk::Str(_) | Tk::Int(_)) && Self::tok_is_op(b, ":")
    }
    fn block_or_expr(&mut self) -> PResult<Expr> {
        if self.is_op("{") && !self.looks_like_map() {
            let st = self.here();
            let b = self.block()?;
            return Ok(Expr::new(ExprKind::Block(b), self.sp(st)));
        }
        self.expr()
    }

    fn sub_parse(&self, text: &str, offset: usize) -> PResult<Expr> {
        let mut p = Parser { t: lex(text, offset)?, i: 0, src: self.src, in_sheet: self.in_sheet };
        let e = p.expr()?;
        if !p.at_eof() { return p.err("unexpected token in string template"); }
        Ok(e)
    }

    fn primary(&mut self) -> PResult<Expr> {
        let st = self.here();
        let tok = self.cur().clone();
        match &tok.tk {
            Tk::Int(v) => {
                self.bump();
                let n = v.parse::<i64>().map_err(|_| Diag::new("integer literal out of range", st))?;
                let e = Expr::new(ExprKind::Int(n), st);
                return Ok(self.unit_suffix(e));
            }
            Tk::Float(v) => { self.bump(); let e = Expr::new(ExprKind::Float(v.parse().unwrap()), st); return Ok(self.unit_suffix(e)); }
            Tk::Decimal(v) => { self.bump(); let e = Expr::new(ExprKind::Decimal(v.clone()), st); return Ok(self.unit_suffix(e)); }
            Tk::Str(parts) => {
                self.bump();
                let mut segs = vec![];
                for p in parts {
                    match p {
                        StrPart::Lit(l) => segs.push(StrSeg::Lit(l.clone())),
                        StrPart::Expr(text, off) => segs.push(StrSeg::Expr(self.sub_parse(text, *off)?)),
                    }
                }
                return Ok(Expr::new(ExprKind::Str(segs), st));
            }
            Tk::Ident(name) => {
                self.bump();
                if self.in_sheet && is_cell(name) {
                    if self.is_op(":") && !self.cur().nl_before {
                        if let Tk::Ident(to) = &self.peek(1).tk {
                            if is_cell(to) {
                                let to = to.clone();
                                self.bump();
                                self.bump();
                                return Ok(Expr::new(ExprKind::CellRange(name.clone(), to), self.sp(st)));
                            }
                        }
                    }
                    return Ok(Expr::new(ExprKind::CellRef(name.clone()), st));
                }
                return Ok(Expr::new(ExprKind::Ident(name.clone()), st));
            }
            Tk::Kw(k) => match *k {
                "true" | "false" => { self.bump(); return Ok(Expr::new(ExprKind::Bool(*k == "true"), st)); }
                "nil" => { self.bump(); return Ok(Expr::new(ExprKind::Nil, st)); }
                "this" => { self.bump(); return Ok(Expr::new(ExprKind::This, st)); }
                "result" => { self.bump(); return Ok(Expr::new(ExprKind::Result, st)); }
                "old" => {
                    self.bump();
                    self.expect_op("(")?;
                    let e = self.expr()?;
                    self.expect_op(")")?;
                    return Ok(Expr::new(ExprKind::Old(Box::new(e), 0), self.sp(st)));
                }
                "super" => {
                    self.bump();
                    let mut qualifier = None;
                    if self.is_op("<") { self.bump(); qualifier = Some(self.ident()?); self.expect_op(">")?; }
                    self.expect_op(".")?;
                    let name = self.name()?;
                    let args = self.args()?;
                    return Ok(Expr::new(ExprKind::Super { qualifier, name, args }, self.sp(st)));
                }
                // Kotlin: `return`, `break`, `continue` are expressions (`val x = y ?: return nil`)
                "return" | "break" | "continue" => {
                    let s = self.stmt()?;
                    return Ok(Expr::new(ExprKind::Block(Block { stmts: vec![s], span: self.sp(st) }), self.sp(st)));
                }
                "if" => return self.if_expr(),
                "match" => return self.match_expr(),
                "sheet" => return self.sheet_expr(),
                _ => {}
            },
            Tk::Op(o) => match *o {
                "(" => { self.bump(); let e = self.expr()?; self.expect_op(")")?; return Ok(e); }
                "[" => {
                    self.bump();
                    let mut items = vec![];
                    while !self.is_op("]") {
                        items.push(self.expr()?);
                        if self.is_op(",") { self.bump(); } else if !self.is_op("]") { return self.err("expected ',' or ']'"); }
                    }
                    self.bump();
                    return Ok(Expr::new(ExprKind::Vec(items), self.sp(st)));
                }
                "{" => return self.brace_literal(),
                "|" | "||" => return self.lambda(),
                "=" if self.in_sheet => { self.bump(); return self.expr(); } // Sheets: B4 = =SUM(B1:B3)
                _ => {}
            },
            Tk::Eof => {}
        }
        self.err(format!("unexpected '{}'", self.desc()))
    }

    /// A unit name directly after a number literal on the same line: `100 USD`, `2.5 kW`, `30 min`.
    fn unit_suffix(&mut self, e: Expr) -> Expr {
        if let Tk::Ident(u) = &self.cur().tk {
            if !self.cur().nl_before && !(self.in_sheet && is_cell(u)) {
                let u = u.clone();
                let span = e.span.to(self.here());
                self.bump();
                return Expr::new(ExprKind::Quantity(Box::new(e), u), span);
            }
        }
        e
    }

    fn brace_literal(&mut self) -> PResult<Expr> {
        let st = self.expect_op("{")?.span;
        if self.is_op("}") { self.bump(); return Ok(Expr::new(ExprKind::Map(vec![]), self.sp(st))); }
        let is_key = |p: &Self| matches!(p.cur().tk, Tk::Ident(_) | Tk::Kw(_)) && Self::tok_is_op(p.peek(1), ":");
        let is_lit_key = |p: &Self| matches!(p.cur().tk, Tk::Str(_) | Tk::Int(_)) && Self::tok_is_op(p.peek(1), ":");
        if self.is_op("...") || is_key(self) || is_lit_key(self) {
            let mut entries = vec![];
            while !self.is_op("}") {
                if self.is_op("...") {
                    self.bump();
                    entries.push(MapEntry::Spread(self.expr()?));
                } else {
                    let ks = self.here();
                    let key = if is_key(self) {
                        let n = self.name()?;
                        Expr::new(ExprKind::Str(vec![StrSeg::Lit(n)]), ks)
                    } else { self.expr()? };
                    self.expect_op(":")?;
                    entries.push(MapEntry::Kv(key, self.expr()?));
                }
                if self.is_op(",") { self.bump(); } else if !self.is_op("}") { return self.err("expected ',' or '}' in map literal"); }
            }
            self.bump();
            return Ok(Expr::new(ExprKind::Map(entries), self.sp(st)));
        }
        // Google Sheets array literal: {1, 2, 3} row, {1; 2; 3} column, {1, 2; 3, 4} 2-D
        let mut rows: Vec<Vec<Expr>> = vec![vec![]];
        while !self.is_op("}") {
            let e = self.expr()?;
            rows.last_mut().unwrap().push(e);
            if self.is_op(",") { self.bump(); }
            else if self.is_op(";") { self.bump(); rows.push(vec![]); }
            else if !self.is_op("}") { return self.err("expected ',', ';' or '}' in array literal"); }
        }
        self.bump();
        let sp = self.sp(st);
        if rows.len() == 1 { return Ok(Expr::new(ExprKind::Vec(rows.pop().unwrap()), sp)); }
        if rows.iter().all(|r| r.len() == 1) {
            return Ok(Expr::new(ExprKind::Vec(rows.into_iter().map(|mut r| r.pop().unwrap()).collect()), sp));
        }
        Ok(Expr::new(ExprKind::Vec(rows.into_iter().map(|r| Expr::new(ExprKind::Vec(r), sp)).collect()), sp))
    }

    fn lambda(&mut self) -> PResult<Expr> {
        let st = self.here();
        let mut params = vec![];
        if self.is_op("||") { self.bump(); } else {
            self.expect_op("|")?;
            while !self.is_op("|") {
                let ps = self.here();
                let name = self.ident()?;
                let ty = if self.is_op(":") { self.bump(); Some(self.postfix_ty()?) } else { None };
                params.push(Param { name, ty, default: None, field: None, span: self.sp(ps) });
                if self.is_op(",") { self.bump(); } else if !self.is_op("|") { return self.err("expected ',' or '|' in lambda parameters"); }
            }
            self.bump();
        }
        let body = self.block_or_expr()?;
        Ok(Expr::new(ExprKind::Lambda(params, Box::new(body)), self.sp(st)))
    }

    fn if_expr(&mut self) -> PResult<Expr> {
        let st = self.expect_kw("if")?.span;
        let cond = self.expr()?;
        let then = self.block()?;
        let mut els = None;
        if self.is_kw("else") {
            self.bump();
            if self.is_kw("if") {
                let es = self.here();
                let nested = self.if_expr()?;
                els = Some(Block { stmts: vec![Stmt::Expr(nested)], span: self.sp(es) });
            } else {
                els = Some(self.block()?);
            }
        }
        Ok(Expr::new(ExprKind::If(Box::new(cond), then, els), self.sp(st)))
    }

    fn match_expr(&mut self) -> PResult<Expr> {
        let st = self.expect_kw("match")?.span;
        let subject = self.expr()?;
        self.expect_op("{")?;
        let mut arms = vec![];
        while !self.is_op("}") {
            if self.is_op(",") || self.is_op(";") { self.bump(); continue; }
            if self.at_eof() { return self.err("unterminated match"); }
            let a_st = self.here();
            let pat = self.pattern()?;
            let guard = if self.is_kw("if") { self.bump(); Some(self.expr()?) } else { None };
            self.expect_op("=>")?;
            let body = self.block_or_expr()?;
            arms.push(MatchArm { pat, guard, body, span: self.sp(a_st) });
        }
        self.bump();
        Ok(Expr::new(ExprKind::Match(Box::new(subject), arms), self.sp(st)))
    }

    fn pattern(&mut self) -> PResult<Pattern> {
        let st = self.here();
        if let Some(name) = self.cur_ident().map(|s| s.to_string()) {
            self.bump();
            if name == "_" { return Ok(Pattern::Wild); }
            if name.starts_with(|c: char| c.is_ascii_uppercase()) {
                let mut args = vec![];
                if self.is_op("(") {
                    self.bump();
                    while !self.is_op(")") { args.push(self.pattern()?); if self.is_op(",") { self.bump(); } }
                    self.bump();
                }
                return Ok(Pattern::Variant { name, args, span: self.sp(st) });
            }
            return Ok(Pattern::Bind(name));
        }
        let lit_start = matches!(self.cur().tk, Tk::Int(_) | Tk::Float(_) | Tk::Decimal(_) | Tk::Str(_))
            || self.is_kw("nil") || self.is_kw("true") || self.is_kw("false") || self.is_op("-");
        if lit_start { return Ok(Pattern::Lit(self.unary()?)); }
        self.err(format!("invalid pattern '{}'", self.desc()))
    }

    fn sheet_expr(&mut self) -> PResult<Expr> {
        let st = self.expect_kw("sheet")?.span;
        self.expect_op("{")?;
        let prev = self.in_sheet;
        self.in_sheet = true;
        let mut cells = vec![];
        while !self.is_op("}") {
            if self.is_op(";") || self.is_op(",") { self.bump(); continue; }
            if self.at_eof() { return self.err("unterminated sheet"); }
            let cs = self.here();
            let cell = self.ident()?;
            if !is_cell(&cell) { return Err(Diag::new(format!("'{}' is not a cell reference like A1 or B12", cell), cs)); }
            self.expect_op("=")?;
            let value = self.expr()?;
            cells.push(SheetCell { cell, value, span: self.sp(cs) });
        }
        self.bump();
        self.in_sheet = prev;
        Ok(Expr::new(ExprKind::Sheet(cells), self.sp(st)))
    }
}
