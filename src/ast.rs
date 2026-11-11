//! Chimera abstract syntax tree.

use crate::diag::Span;

// ---------- types ----------
#[derive(Debug, Clone)]
pub enum TypeNode {
    Name { name: String, args: Vec<TypeNode>, span: Span },
    Nullable(Box<TypeNode>),
    Record(Vec<RecordField>),
    StrLit(String),
    Union(Vec<TypeNode>),
    Fn(Vec<TypeNode>, Box<TypeNode>),
    KeyOf(Box<TypeNode>),
}

#[derive(Debug, Clone)]
pub struct RecordField {
    pub name: String,
    pub ty: TypeNode,
    pub optional: bool,
    pub readonly: bool,
}

// ---------- patterns ----------
#[derive(Debug, Clone)]
pub enum Pattern {
    Wild,
    Bind(String),
    Lit(Expr),
    Variant { name: String, args: Vec<Pattern>, span: Span },
}

// ---------- expressions ----------
#[derive(Debug, Clone)]
pub struct Arg {
    pub name: Option<String>,
    pub value: Expr,
}

#[derive(Debug, Clone)]
pub enum MapEntry {
    Spread(Expr),
    Kv(Expr, Expr),
}

#[derive(Debug, Clone)]
pub enum StrSeg {
    Lit(String),
    Expr(Expr),
}

#[derive(Debug, Clone)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum ExprKind {
    Int(i64),
    Float(f64),
    /// exact decimal, kept as source text (e.g. "1.0999")
    Decimal(String),
    /// value with a unit of measure: `100 USD`, `2.5 kW`
    Quantity(Box<Expr>, String),
    Str(Vec<StrSeg>),
    Bool(bool),
    Nil,
    Ident(String),
    This,
    Result,
    Old(Box<Expr>, usize),
    Binary(String, Box<Expr>, Box<Expr>),
    Unary(String, Box<Expr>),
    Percent(Box<Expr>),
    Call(Box<Expr>, Vec<Arg>),
    Member { obj: Box<Expr>, name: String, safe: bool },
    Super { qualifier: Option<String>, name: String, args: Vec<Arg> },
    Index(Box<Expr>, Box<Expr>),
    Lambda(Vec<Param>, Box<Expr>),
    If(Box<Expr>, Block, Option<Block>),
    Match(Box<Expr>, Vec<MatchArm>),
    Vec(Vec<Expr>),
    Map(Vec<MapEntry>),
    Try(Box<Expr>),
    NotNull(Box<Expr>),
    Thread { op: String, left: Box<Expr>, right: Box<Expr> },
    Range(Box<Expr>, Box<Expr>),
    Block(Block),
    Is(Box<Expr>, TypeNode),
    Sheet(Vec<SheetCell>),
    CellRef(String),
    CellRange(String, String),
}

impl Expr {
    pub fn new(kind: ExprKind, span: Span) -> Expr { Expr { kind, span } }
}

#[derive(Debug, Clone)]
pub struct SheetCell {
    pub cell: String,
    pub value: Expr,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct MatchArm {
    pub pat: Pattern,
    pub guard: Option<Expr>,
    pub body: Expr,
    pub span: Span,
}

// ---------- statements ----------
#[derive(Debug, Clone)]
pub enum Stmt {
    Let { mutable: bool, name: String, ty: Option<TypeNode>, init: Expr, span: Span },
    Assign { target: Expr, op: String, value: Expr, span: Span },
    Expr(Expr),
    Return(Option<Expr>, Span),
    While(Expr, Block),
    For(String, Expr, Block),
    Break(Span),
    Continue(Span),
}

#[derive(Debug, Clone)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    pub span: Span,
}

// ---------- declarations ----------
#[derive(Debug, Clone)]
pub struct Param {
    pub name: String,
    pub ty: Option<TypeNode>,
    pub default: Option<Expr>,
    /// `val` / `var` on a class constructor parameter
    pub field: Option<bool>, // Some(true) = var, Some(false) = val
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct Contract {
    pub tag: Option<String>,
    pub expr: Expr,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct FunDecl {
    pub name: String,
    pub params: Vec<Param>,
    pub ret: Option<TypeNode>,
    pub requires: Vec<Contract>,
    pub ensures: Vec<Contract>,
    pub body: Option<Expr>,
    pub is_override: bool,
    pub span: Span,
    /// `old(...)` expressions collected from ensure clauses, indexed by slot
    pub olds: Vec<Expr>,
}

#[derive(Debug, Clone)]
pub struct FieldDecl {
    pub mutable: bool,
    pub name: String,
    pub ty: Option<TypeNode>,
    pub init: Expr,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct SuperRef {
    pub name: String,
    pub args: Option<Vec<Arg>>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct ClassDecl {
    pub name: String,
    pub is_trait: bool,
    pub is_data: bool,
    pub is_abstract: bool,
    pub params: Vec<Param>,
    pub supers: Vec<SuperRef>,
    pub methods: Vec<FunDecl>,
    pub fields: Vec<FieldDecl>,
    pub invariants: Vec<Contract>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct Variant {
    pub name: String,
    pub fields: Vec<Param>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct EnumDecl {
    pub name: String,
    pub variants: Vec<Variant>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct TypeAlias {
    pub name: String,
    pub type_params: Vec<String>,
    pub ty: TypeNode,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct GenericDecl {
    pub name: String,
    pub params: Vec<Param>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct MethodDecl {
    pub qualifier: String, // primary | before | after | around
    pub name: String,
    pub params: Vec<Param>,
    pub body: Expr,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum Decl {
    Fun(FunDecl),
    Class(ClassDecl),
    Enum(EnumDecl),
    TypeAlias(TypeAlias),
    Generic(GenericDecl),
    Method(MethodDecl),
}

#[derive(Debug, Clone)]
pub struct Program {
    pub decls: Vec<Decl>,
}
