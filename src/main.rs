#![allow(dead_code)]
//! `chimera` — compiler driver.
//!
//!   chimera check <file.chim>
//!   chimera build <file.chim> --target rust|js [-o <out>]
//!   chimera run   <file.chim> [--target rust|js]

mod ast;
mod common;
mod desugar;
mod diag;
mod gen_js;
mod gen_rust;
mod lexer;
mod native;
mod parser;
mod sema;
mod units;

use std::path::{Path, PathBuf};
use std::process::{exit, Command};

thread_local! { pub static BASE_DIR: std::cell::RefCell<PathBuf> = std::cell::RefCell::new(PathBuf::from(".")); }

/// `embedFile("path")`: read a file at compile time, relative to the main source file.
pub fn embed_file(rel: &str) -> String {
    let p = BASE_DIR.with(|b| b.borrow().join(rel));
    std::fs::read_to_string(&p).unwrap_or_else(|e| { eprintln!("error: embedFile: cannot read {}: {}", p.display(), e); exit(1) })
}

const RT_RS: &str = include_str!("../runtime/rt.rs");
const RT_JS: &str = include_str!("../runtime/rt.js");

fn usage() -> ! {
    eprintln!("Chimera compiler {}\n\nUSAGE:\n  chimera check <file.chim>\n  chimera build <file.chim> --target rust|js [-o <out>]\n  chimera run   <file.chim> [--target rust|js]\n  chimera emit  <file.chim> --target rust|js     (print generated code)\n  chimera tokens <file.chim>                    (dump the token stream)", env!("CARGO_PKG_VERSION"));
    exit(2)
}

/// A source file spliced into the combined program text by `import "file.chim"`.
pub struct Segment { pub start: usize, pub end: usize, pub file: String, pub text: String, pub base: usize }

/// Resolve `import "path"` lines (relative to the importing file) by textual inclusion, each file once.
/// Imported files come first; import lines are blanked so offsets inside each file are preserved.
fn load(path: &Path, seen: &mut Vec<PathBuf>, out: &mut String, segs: &mut Vec<Segment>) {
    let canon = path.canonicalize().unwrap_or(path.to_path_buf());
    if seen.contains(&canon) { return; }
    seen.push(canon);
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| { eprintln!("error: cannot read {}: {}", path.display(), e); exit(1) });
    let mut body = String::new();
    for line in text.split_inclusive('\n') {
        let t = line.trim();
        if t.starts_with("import \"") && t.ends_with('"') {
            let rel = &t[8..t.len() - 1];
            let dir = path.parent().unwrap_or(Path::new("."));
            load(&dir.join(rel), seen, out, segs);
            body.push_str(&" ".repeat(line.trim_end_matches('\n').len()));
            if line.ends_with('\n') { body.push('\n'); }
        } else { body.push_str(line); }
    }
    let start = out.len();
    out.push_str(&body);
    out.push('\n');
    segs.push(Segment { start, end: out.len(), file: path.display().to_string(), text: body, base: start });
}

fn compile(file: &str) -> (ast::Program, sema::Info) {
    let mut src = String::new();
    let mut segs = vec![];
    load(Path::new(file), &mut vec![], &mut src, &mut segs);
    let fail = |d: Vec<diag::Diag>| -> ! {
        for x in &d {
            let seg = segs.iter().find(|g| x.span.start >= g.start && x.span.start < g.end).or(segs.last()).unwrap();
            let local = diag::Diag::new(x.msg.clone(), diag::Span::new(x.span.start - seg.base, x.span.end.saturating_sub(seg.base)));
            eprint!("{}", local.render(&seg.file, &seg.text));
        }
        eprintln!("{} error{} found", d.len(), if d.len() == 1 { "" } else { "s" });
        exit(1)
    };
    let mut prog = parser::parse(&src).unwrap_or_else(|d| fail(vec![d]));
    desugar::desugar(&mut prog).unwrap_or_else(|d| fail(vec![d]));
    let info = sema::check(&mut prog).unwrap_or_else(|d| fail(d));
    let dir = Path::new(file).parent().map(|p| p.to_path_buf()).unwrap_or(PathBuf::from("."));
    BASE_DIR.with(|b| *b.borrow_mut() = dir);
    (prog, info)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 { usage(); }
    let cmd = args[0].as_str();
    let file = &args[1];
    let mut target = "rust".to_string();
    let mut out: Option<String> = None;
    let mut i = 2;
    while i < args.len() {
        match args[i].as_str() {
            "--target" | "-t" => { target = args.get(i + 1).cloned().unwrap_or_else(|| usage()); i += 2; }
            "-o" => { out = args.get(i + 1).cloned(); i += 2; }
            _ => usage(),
        }
    }
    if cmd == "tokens" { dump_tokens(file); return; }
    if target != "rust" && target != "js" { eprintln!("error: unknown target '{}' (use rust or js)", target); exit(2); }
    let (prog, info) = compile(file);
    let stem = Path::new(file).file_stem().unwrap().to_string_lossy().to_string();
    match cmd {
        "check" => println!("{}: ok", file),
        "emit" => {
            if target == "rust" { print!("{}", gen_rust::generate(&prog, &info)); } else { print!("{}", gen_js::generate(&prog, &info)); }
        }
        "build" | "run" => {
            let path = if target == "rust" { build_rust(&prog, &info, out.as_deref(), &stem) } else { build_js(&prog, &info, out.as_deref(), &stem) };
            if cmd == "build" { eprintln!("wrote {}", path.display()); return; }
            let status = if target == "rust" { Command::new(&path).status() } else { Command::new("node").arg(&path).status() };
            match status { Ok(s) => exit(s.code().unwrap_or(1)), Err(e) => { eprintln!("error: cannot run {}: {}", path.display(), e); exit(1) } }
        }
        _ => usage(),
    }
}

fn build_js(prog: &ast::Program, info: &sema::Info, out: Option<&str>, stem: &str) -> PathBuf {
    let path = PathBuf::from(out.map(|s| s.to_string()).unwrap_or_else(|| format!("{}.js", stem)));
    let code = format!("{}\n{}", RT_JS, gen_js::generate(prog, info));
    std::fs::write(&path, code).unwrap_or_else(|e| { eprintln!("error: cannot write {}: {}", path.display(), e); exit(1) });
    path
}

/// Writes a Cargo crate `<dir>/{Cargo.toml, src/main.rs, src/rt.rs}`, formats it with rustfmt
/// (when available) and builds it with `cargo build --release`.
fn build_rust(prog: &ast::Program, info: &sema::Info, out: Option<&str>, stem: &str) -> PathBuf {
    let dir = PathBuf::from(out.map(|s| s.to_string()).unwrap_or_else(|| format!("{}_rs", stem)));
    let name: String = stem.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' }).collect();
    let name = if name.starts_with(|c: char| c.is_ascii_digit()) { format!("p_{}", name) } else { name };
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("Cargo.toml"), format!(
        "# generated by chimera\n[package]\nname = \"{}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\n\n[profile.release]\noverflow-checks = true\n\n[workspace]\n", name)).unwrap();
    std::fs::write(dir.join("src/main.rs"), gen_rust::generate(prog, info)).unwrap();
    std::fs::write(dir.join("src/rt.rs"), RT_RS).unwrap();
    let _ = Command::new("rustfmt").args(["--edition", "2021"]).arg(dir.join("src/main.rs")).stderr(std::process::Stdio::null()).status();
    let st = Command::new("cargo").args(["build", "--release", "-q", "--manifest-path"]).arg(dir.join("Cargo.toml")).status();
    match st {
        Ok(s) if s.success() => dir.join("target/release").join(&name),
        Ok(_) => { eprintln!("error: cargo failed on the generated crate in {}", dir.display()); exit(1) }
        Err(e) => { eprintln!("error: cannot run cargo: {}", e); exit(1) }
    }
}

/// Token dump used to verify the self-hosted lexer: `KIND<TAB>start<TAB>end<TAB>nl<TAB>text` (char offsets)
fn dump_tokens(file: &str) {
    let src = std::fs::read_to_string(file).unwrap_or_else(|e| { eprintln!("error: cannot read {}: {}", file, e); exit(1) });
    let toks = lexer::lex(&src, 0).unwrap_or_else(|d| { eprint!("{}", d.render(file, &src)); exit(1) });
    let ci = |b: usize| src[..b].chars().count();
    for t in toks {
        let kind = match t.tk { lexer::Tk::Ident(_) => "Ident", lexer::Tk::Kw(_) => "Kw", lexer::Tk::Int(_) => "Int", lexer::Tk::Float(_) => "Float",
            lexer::Tk::Decimal(_) => "Decimal", lexer::Tk::Str(_) => "Str", lexer::Tk::Op(_) => "Op", lexer::Tk::Eof => "Eof" };
        let text = src[t.span.start..t.span.end].replace('\\', "\\\\").replace('\n', "\\n").replace('\t', "\\t");
        println!("{}\t{}\t{}\t{}\t{}", kind, ci(t.span.start), ci(t.span.end), t.nl_before, text);
    }
}
