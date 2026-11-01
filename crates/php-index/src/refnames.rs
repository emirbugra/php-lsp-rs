//! Cheap per-file "which names does this file mention" set, used as the candidate filter for
//! find-references / rename (the real verification happens with full semantic analysis).

use php_syntax::walk::*;
use php_syntax::*;
use rustc_hash::FxHasher;
use std::hash::Hasher;

pub fn name_hash(s: &str) -> u32 {
    let mut h = FxHasher::default();
    for b in s.bytes() {
        h.write_u8(b.to_ascii_lowercase());
    }
    h.finish() as u32
}

pub fn last_segment(s: &str) -> &str {
    s.rsplit('\\').next().unwrap_or(s)
}

pub fn collect(src: &str, stmts: &[Stmt]) -> Vec<u32> {
    let mut c = Collector { out: Vec::new() };
    walk_stmts(&mut c, stmts);
    doc_words(src, &mut c.out);
    c.out.sort_unstable();
    c.out.dedup();
    c.out
}

/// Words on the `@tag` lines of every docblock (class names in `@var` / `@param` / `@return` …),
/// so files that mention a class only in comments are still find-references candidates.
fn doc_words(src: &str, out: &mut Vec<u32>) {
    let mut rest = src;
    while let Some(i) = rest.find("/**") {
        let after = &rest[i + 3..];
        let end = after.find("*/").unwrap_or(after.len());
        for line in after[..end].lines() {
            let Some(at) = line.find('@') else { continue };
            let tail = &line[at + 1..];
            let mut words = tail.split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '\\' || c == '$'));
            words.next(); // the tag itself
            for w in words {
                if !w.is_empty() && !w.starts_with('$') && !w.as_bytes()[0].is_ascii_digit() {
                    out.push(name_hash(last_segment(w)));
                }
            }
        }
        rest = &after[end..];
    }
}

struct Collector {
    out: Vec<u32>,
}

impl Collector {
    fn name(&mut self, s: &str) {
        if !s.is_empty() {
            self.out.push(name_hash(last_segment(s)));
        }
    }

    fn hint(&mut self, h: &Option<TypeHint>) {
        if let Some(h) = h {
            for n in &h.names {
                self.name(&n.text);
            }
        }
    }

    fn class_ref(&mut self, c: &ClassRef) {
        if let ClassRef::Name(n) = c {
            self.name(&n.text);
        }
    }

    fn member(&mut self, m: &MemberName) {
        if let MemberName::Ident(i) = m {
            self.name(&i.text);
        }
    }
}

impl<'ast> Visitor<'ast> for Collector {
    fn visit_stmt(&mut self, s: &'ast Stmt) {
        match &s.kind {
            StmtKind::Use(items) => items.iter().for_each(|i| self.name(&i.name)),
            StmtKind::Try { catches, .. } => {
                for c in catches {
                    c.types.iter().for_each(|t| self.name(&t.text));
                }
            }
            StmtKind::Const(items) => items.iter().for_each(|i| self.name(&i.name.text)),
            _ => {}
        }
        walk_stmt(self, s);
    }

    fn visit_class(&mut self, c: &'ast ClassDecl) {
        if let Some(n) = &c.name {
            self.name(&n.text);
        }
        c.extends.iter().chain(c.implements.iter()).for_each(|n| self.name(&n.text));
        self.hint(&c.backing);
        for m in &c.members {
            match m {
                Member::Property(p) => {
                    self.name(&p.name.text);
                    self.hint(&p.ty);
                }
                Member::Const { items, .. } => items.iter().for_each(|i| self.name(&i.name.text)),
                Member::TraitUse { names, .. } => names.iter().for_each(|n| self.name(&n.text)),
                Member::Case { name, .. } => self.name(&name.text),
                Member::Method { .. } => {}
            }
        }
        walk_class(self, c);
    }

    fn visit_function(&mut self, f: &'ast FunctionDecl) {
        if let Some(n) = &f.name {
            self.name(&n.text);
        }
        for p in &f.params {
            self.hint(&p.ty);
            if p.promoted.is_some() {
                self.name(&p.name);
            }
        }
        self.hint(&f.ret);
        walk_function(self, f);
    }

    fn visit_expr(&mut self, e: &'ast Expr) {
        match &e.kind {
            ExprKind::Name(n) => self.name(&n.text),
            ExprKind::New { class, .. } | ExprKind::InstanceOf { class, .. } => self.class_ref(class),
            ExprKind::MethodCall { name, .. } | ExprKind::Prop { name, .. } => self.member(name),
            ExprKind::StaticCall { class, name, .. } | ExprKind::StaticProp { class, name } => {
                self.class_ref(class);
                self.member(name);
            }
            ExprKind::ClassConst { class, name } => {
                self.class_ref(class);
                self.name(&name.text);
            }
            _ => {}
        }
        walk_expr(self, e);
    }
}
