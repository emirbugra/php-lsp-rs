//! Walks a file and turns every declaration into a [`Symbol`].

use super::doc_members::DocClass;
use super::{Bases, ParamInfo, Symbol, SymbolKind};
use crate::docblock::{self, DocBlock};
use crate::istr::{istrs, IStr};
use crate::names::NameCtx;
use php_syntax::*;

pub(super) struct SymbolCollector<'a> {
    pub(super) src: &'a str,
    pub(super) ctx: NameCtx,
    pub(super) out: Vec<Symbol>,
    /// `@template` names currently in scope (class, then function).
    pub(super) templates_in_scope: Vec<String>,
}

pub(super) fn vis_str(v: Visibility) -> &'static str {
    match v {
        Visibility::Public => "public",
        Visibility::Protected => "protected",
        Visibility::Private => "private",
    }
}

/// `s` on one line, cut to `n` characters.
pub(super) fn truncate(s: &str, n: usize) -> String {
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if s.chars().count() > n {
        let t: String = s.chars().take(n).collect();
        format!("{t}…")
    } else {
        s
    }
}

/// From the constant's name to the end of its value.
fn const_range(it: &ConstItem) -> Span {
    Span::new(it.name.span.start as usize, it.value.span.end.max(it.name.span.end) as usize)
}

/// The body calls `func_get_args()`, `func_get_arg()` or `func_num_args()` (any letter case).
fn reads_all_args(body: &str) -> bool {
    let b = body.as_bytes();
    let pat = b"func_";
    b.windows(pat.len()).enumerate().any(|(i, w)| {
        w.eq_ignore_ascii_case(pat) && {
            let rest = &b[i + pat.len()..];
            let starts = |p: &[u8]| rest.len() >= p.len() && rest[..p.len()].eq_ignore_ascii_case(p);
            starts(b"get_arg") || starts(b"num_args")
        }
    })
}

fn is_deprecated(doc: &Option<DocBlock>) -> bool {
    doc.as_ref().map(|d| d.is_deprecated()).unwrap_or(false)
}

impl<'a> SymbolCollector<'a> {
    pub(super) fn new(src: &'a str) -> Self {
        SymbolCollector { src, ctx: NameCtx::default(), out: Vec::new(), templates_in_scope: Vec::new() }
    }

    fn fqn(&self, name: &str) -> String {
        if self.ctx.namespace.is_empty() {
            name.to_string()
        } else {
            format!("{}\\{}", self.ctx.namespace, name)
        }
    }

    fn doc_block(&self, doc: Option<Span>) -> Option<DocBlock> {
        doc.map(|s| docblock::parse(s.text(self.src)))
    }

    pub(super) fn stmts(&mut self, stmts: &[Stmt]) {
        for s in stmts {
            self.stmt(s);
        }
    }

    fn stmt(&mut self, s: &Stmt) {
        match &s.kind {
            StmtKind::Namespace { name, body } => {
                let ns = name.as_ref().map(|n| n.text.as_str()).unwrap_or("");
                self.ctx.set_namespace(ns);
                if let Some(b) = body {
                    self.stmts(b);
                    self.ctx.set_namespace("");
                }
            }
            StmtKind::Use(items) => {
                for i in items {
                    self.ctx.add_use(i.kind, &i.name, i.alias.as_deref());
                }
            }
            StmtKind::Function(f) => self.function(f),
            StmtKind::Class(c) => self.class(c),
            StmtKind::Const(items) => {
                for it in items {
                    let mut sym = Symbol::new(
                        SymbolKind::Const,
                        it.name.text.clone(),
                        self.fqn(&it.name.text),
                        const_range(it),
                        it.name.span,
                    );
                    sym.detail =
                        IStr::from(format!("const {} = {}", it.name.text, truncate(it.value.span.text(self.src), 60)));
                    sym.doc = it.doc;
                    self.out.push(sym);
                }
            }
            StmtKind::Control { header, blocks, .. } => {
                header.iter().for_each(|e| self.define_calls_in(e));
                for b in blocks {
                    self.stmts(b);
                }
            }
            StmtKind::Block(b) => self.stmts(b),
            StmtKind::Try { body, catches, finally } => {
                self.stmts(body);
                for c in catches {
                    self.stmts(&c.body);
                }
                if let Some(f) = finally {
                    self.stmts(f);
                }
            }
            StmtKind::Expr(e) => self.define_calls_in(e),
            _ => {}
        }
    }

    /// `define()` calls, also inside `defined('X') || define('X', 1)` or `if (!defined(..)) define(..)`.
    fn define_calls_in(&mut self, e: &Expr) {
        match &e.kind {
            ExprKind::Binary { lhs, rhs, .. } => {
                self.define_calls_in(lhs);
                self.define_calls_in(rhs);
            }
            ExprKind::Paren(x) | ExprKind::Unary { expr: x, .. } => self.define_calls_in(x),
            ExprKind::Ternary { then, els, .. } => {
                if let Some(t) = then {
                    self.define_calls_in(t);
                }
                self.define_calls_in(els);
            }
            _ => self.define_call(e),
        }
    }

    fn define_call(&mut self, e: &Expr) {
        let ExprKind::Call { callee, args } = &e.kind else { return };
        let ExprKind::Name(n) = &callee.kind else { return };
        if !n.text.trim_start_matches('\\').eq_ignore_ascii_case("define") || args.len() < 2 {
            return;
        }
        let a = &args[0].value;
        let Some(name) = a.string_value(self.src) else { return };
        let mut sym =
            Symbol::new(SymbolKind::Const, name.to_string(), name.trim_start_matches('\\').to_string(), e.span, a.span);
        sym.detail = IStr::from(format!("const {} = {}", name, truncate(args[1].value.span.text(self.src), 60)));
        self.out.push(sym);
    }

    fn function(&mut self, f: &FunctionDecl) {
        let Some(name) = &f.name else { return };
        let doc = self.doc_block(f.doc);
        let templates = self.push_templates(&doc);
        let params = self.params(&f.params, &doc, None, None);
        let (ty, type_classes) = self.return_type(f, &doc, None, None);
        self.pop_templates(templates.len());
        let mut sym = Symbol::new(SymbolKind::Function, name.text.clone(), self.fqn(&name.text), f.span, name.span);
        if !templates.is_empty() {
            sym.bases = Some(Box::new(Bases { templates, ..Default::default() }));
        }
        sym.detail = IStr::from(format!(
            "function {}{}({}){}",
            if f.by_ref { "&" } else { "" },
            name.text,
            Self::param_list(&params),
            ty.as_ref().map(|t| format!(": {t}")).unwrap_or_default()
        ));
        sym.params = params;
        sym.ty = ty.map(IStr::from);
        sym.ty_native = f.ret.is_some();
        sym.type_classes = istrs(type_classes);
        sym.reads_all_args = reads_all_args(f.span.text(self.src));
        sym.doc = f.doc;
        sym.deprecated = is_deprecated(&doc);
        self.out.push(sym);
    }

    fn class(&mut self, c: &ClassDecl) {
        let Some(name) = &c.name else { return };
        let fqn = self.fqn(&name.text);
        let kind = match c.kind {
            ClassKind::Class => SymbolKind::Class,
            ClassKind::Interface => SymbolKind::Interface,
            ClassKind::Trait => SymbolKind::Trait,
            ClassKind::Enum => SymbolKind::Enum,
        };
        let extends: Vec<String> = c.extends.iter().map(|n| self.ctx.resolve_class(&n.text)).collect();
        let implements: Vec<String> = c.implements.iter().map(|n| self.ctx.resolve_class(&n.text)).collect();
        let mut traits = Vec::new();
        for m in &c.members {
            if let Member::TraitUse { names, .. } = m {
                for n in names {
                    traits.push(self.ctx.resolve_class(&n.text));
                }
            }
        }
        let parent = extends.first().cloned();
        let doc = self.doc_block(c.doc);

        let mut sym = Symbol::new(kind, name.text.clone(), fqn.clone(), c.span, name.span);
        sym.detail = IStr::from(class_detail(c, &name.text));
        sym.is_abstract = c.modifiers.is_abstract || c.kind == ClassKind::Interface;
        sym.doc = c.doc;
        sym.deprecated = is_deprecated(&doc);
        let templates = self.push_templates(&doc);
        let inherit: Vec<String> = doc
            .as_ref()
            .map(|d| d.inherits())
            .unwrap_or_default()
            .iter()
            .flat_map(|t| self.doc_atoms(t, Some(&fqn), parent.as_deref()))
            .collect();
        let template_count = templates.len();
        let mut bases = Bases {
            extends: istrs(extends),
            implements: istrs(implements),
            traits: istrs(traits),
            templates,
            inherit: istrs(inherit),
        };
        sym.mixin = doc.as_ref().map(|d| d.tag("mixin").is_some()).unwrap_or(false);
        if c.kind == ClassKind::Enum {
            bases.implements.push(IStr::from("UnitEnum"));
            if c.backing.is_some() {
                bases.implements.push(IStr::from("BackedEnum"));
            }
        }
        sym.bases = Some(Box::new(bases));
        self.out.push(sym);

        for m in &c.members {
            self.member(m, &fqn, parent.as_deref(), c.kind);
        }
        if let Some(d) = &doc {
            let class =
                DocClass { fqn: &fqn, parent: parent.as_deref(), range: c.span, name_span: name.span, doc: c.doc };
            self.doc_members(d, &class);
        }
        self.pop_templates(template_count);
    }

    fn member(&mut self, m: &Member, class_fqn: &str, parent: Option<&str>, class_kind: ClassKind) {
        match m {
            Member::Method { decl, modifiers } => self.method(decl, modifiers, class_fqn, parent, class_kind),
            Member::Property(p) => self.property(p, class_fqn, parent),
            Member::Const { items, modifiers, .. } => {
                for it in items {
                    let mut sym = Symbol::member(
                        SymbolKind::ClassConst,
                        it.name.text.clone(),
                        class_fqn,
                        const_range(it),
                        it.name.span,
                    );
                    sym.vis = modifiers.vis.unwrap_or(Visibility::Public);
                    sym.is_static = true;
                    sym.detail = IStr::from(format!(
                        "{} const {} = {}",
                        vis_str(sym.vis),
                        it.name.text,
                        truncate(it.value.span.text(self.src), 60)
                    ));
                    sym.doc = it.doc;
                    self.out.push(sym);
                }
            }
            Member::Case { name, value, doc, span } => {
                let mut sym = Symbol::member(SymbolKind::EnumCase, name.text.clone(), class_fqn, *span, name.span);
                sym.is_static = true;
                sym.type_classes = vec![IStr::from(class_fqn.to_string())];
                sym.detail = match value {
                    Some(v) => IStr::from(format!("case {} = {}", name.text, truncate(v.span.text(self.src), 40))),
                    None => IStr::from(format!("case {}", name.text)),
                };
                sym.doc = *doc;
                self.out.push(sym);
            }
            Member::TraitUse { .. } => {}
        }
    }

    fn method(
        &mut self,
        decl: &FunctionDecl,
        modifiers: &Modifiers,
        class_fqn: &str,
        parent: Option<&str>,
        class_kind: ClassKind,
    ) {
        let Some(name) = &decl.name else { return };
        let doc = self.doc_block(decl.doc);
        let templates = self.push_templates(&doc);
        let params = self.params(&decl.params, &doc, Some(class_fqn), parent);
        let (ty, type_classes) = self.return_type(decl, &doc, Some(class_fqn), parent);
        self.pop_templates(templates.len());
        let mut sym = Symbol::member(SymbolKind::Method, name.text.clone(), class_fqn, decl.span, name.span);
        if !templates.is_empty() {
            sym.bases = Some(Box::new(Bases { templates, ..Default::default() }));
        }
        sym.vis = modifiers.vis.unwrap_or(Visibility::Public);
        sym.is_static = modifiers.is_static;
        sym.is_abstract = modifiers.is_abstract || (class_kind == ClassKind::Interface);
        let mut d = String::new();
        if modifiers.is_abstract {
            d.push_str("abstract ");
        }
        if modifiers.is_final {
            d.push_str("final ");
        }
        d.push_str(vis_str(sym.vis));
        if modifiers.is_static {
            d.push_str(" static");
        }
        d.push_str(&format!(
            " function {}{}({}){}",
            if decl.by_ref { "&" } else { "" },
            name.text,
            Self::param_list(&params),
            ty.as_ref().map(|t| format!(": {t}")).unwrap_or_default()
        ));
        sym.detail = IStr::from(d);
        sym.ty = ty.map(IStr::from);
        sym.ty_native = decl.ret.is_some();
        sym.type_classes = istrs(type_classes);
        sym.reads_all_args = reads_all_args(decl.span.text(self.src));
        sym.doc = decl.doc;
        sym.deprecated = is_deprecated(&doc);
        let ret_classes = sym.type_classes.clone();
        sym.params = params.clone();
        self.out.push(sym);
        self.eloquent_method_members(decl, class_fqn, &params, &ret_classes);
        if name.text.eq_ignore_ascii_case("__construct") {
            self.promoted_properties(decl, class_fqn, &params);
        }
    }

    /// `public function __construct(private User $user)` also declares the property `$user`.
    fn promoted_properties(&mut self, ctor: &FunctionDecl, class_fqn: &str, infos: &[ParamInfo]) {
        for (p, info) in ctor.params.iter().zip(infos) {
            let Some(vis) = p.promoted else { continue };
            let mut ps = Symbol::member(SymbolKind::Property, p.name.clone(), class_fqn, p.span, p.span);
            ps.vis = vis;
            ps.ty = info.ty;
            ps.ty_native = info.ty_native;
            ps.type_classes = info.classes.clone();
            ps.detail = IStr::from(format!(
                "{} {}${}",
                vis_str(vis),
                info.ty.as_ref().map(|t| format!("{t} ")).unwrap_or_default(),
                p.name
            ));
            self.out.push(ps);
        }
    }

    fn property(&mut self, p: &PropertyDecl, class_fqn: &str, parent: Option<&str>) {
        let doc = self.doc_block(p.doc);
        let mut sym = Symbol::member(SymbolKind::Property, p.name.text.clone(), class_fqn, p.span, p.name.span);
        sym.vis = p.modifiers.vis.unwrap_or(Visibility::Public);
        sym.is_static = p.modifiers.is_static;
        let native: Vec<String> = match &p.ty {
            Some(t) => {
                sym.ty = Some(IStr::from(t.text.clone()));
                sym.ty_native = true;
                self.hint_classes(t, Some(class_fqn), parent)
            }
            None => Vec::new(),
        };
        let mut classes = native.clone();
        if let Some(tag) = doc.as_ref().and_then(|d| d.tag_pref("var")) {
            let (ty, _) = docblock::split_type(&tag.body);
            if !ty.is_empty() {
                classes = Self::refine(native, self.doc_atoms(ty, Some(class_fqn), parent));
                if p.ty.is_none() {
                    sym.ty = Some(IStr::from(ty.to_string()));
                }
            }
        }
        sym.type_classes = istrs(classes);
        let mut d = String::from(vis_str(sym.vis));
        if sym.is_static {
            d.push_str(" static");
        }
        if p.modifiers.is_readonly {
            d.push_str(" readonly");
        }
        if let Some(t) = &sym.ty {
            d.push(' ');
            d.push_str(t);
        }
        d.push_str(&format!(" ${}", p.name.text));
        if let Some(def) = &p.default {
            d.push_str(&format!(" = {}", truncate(def.span.text(self.src), 40)));
        }
        sym.detail = IStr::from(d);
        sym.doc = p.doc;
        sym.deprecated = is_deprecated(&doc);
        self.out.push(sym);
        self.eloquent_array_props(p, class_fqn);
    }
}

/// `abstract class Foo extends Bar implements Baz`
fn class_detail(c: &ClassDecl, name: &str) -> String {
    let mut detail = String::new();
    if c.modifiers.is_abstract {
        detail.push_str("abstract ");
    }
    if c.modifiers.is_final {
        detail.push_str("final ");
    }
    detail.push_str(match c.kind {
        ClassKind::Class => "class ",
        ClassKind::Interface => "interface ",
        ClassKind::Trait => "trait ",
        ClassKind::Enum => "enum ",
    });
    detail.push_str(name);
    if !c.extends.is_empty() {
        detail.push_str(" extends ");
        detail.push_str(&c.extends.iter().map(|n| n.text.as_str()).collect::<Vec<_>>().join(", "));
    }
    if !c.implements.is_empty() {
        detail.push_str(" implements ");
        detail.push_str(&c.implements.iter().map(|n| n.text.as_str()).collect::<Vec<_>>().join(", "));
    }
    detail
}
