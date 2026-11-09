//! "Extract variable" and "Inline variable".
//!
//! Extract: the selected expression (or, with no selection, the call / member access under the
//! cursor) goes into a new variable declared just before the statement it is in; a second action
//! also replaces the other copies of it further down the same block.
//!
//! Inline: a variable assigned once (`$x = expr;`) and only read afterwards in the same block is
//! replaced by its value everywhere, and the assignment goes away.
//!
//! Both stay away from code where the move would change what runs: loop conditions, `elseif`,
//! arrow functions, braceless bodies, strings with the variable inside, `compact()` / `$$x` …

use crate::convert::*;
use crate::server::Server;
use lsp_types::*;
use php_syntax::walk::{walk_expr, walk_function, walk_stmts, Visitor};
use php_syntax::*;
use std::collections::HashMap;

/// (title, edits as (span, new text)).
pub type Action = (String, Vec<(Span, String)>);

impl Server {
    pub(super) fn refactor_actions(
        &mut self,
        uri: &str,
        range: Range,
        only: &Option<Vec<CodeActionKind>>,
        out: &mut Vec<CodeActionOrCommand>,
    ) {
        let Some(doc) = self.docs.get(uri) else { return };
        if doc.source.is_some() {
            return; // Blade: the PHP text is generated
        }
        let (text, lines) = (&doc.text, &doc.lines);
        let sel = Span::new(offset_of(text, lines, range.start), offset_of(text, lines, range.end));
        let allowed = |k: &CodeActionKind| match only {
            None => true,
            Some(o) => o.iter().any(|x| k.as_str().starts_with(x.as_str())),
        };
        let mut found: Vec<(CodeActionKind, Action)> = Vec::new();
        if allowed(&CodeActionKind::REFACTOR_EXTRACT) {
            for a in extract_variable(text, &doc.parse.stmts, sel) {
                found.push((CodeActionKind::REFACTOR_EXTRACT, a));
            }
        }
        if allowed(&CodeActionKind::REFACTOR_INLINE) {
            if let Some(a) = inline_variable(text, &doc.parse.stmts, sel.start as usize) {
                found.push((CodeActionKind::REFACTOR_INLINE, a));
            }
        }
        for (kind, (title, edits)) in found {
            let edits: Vec<TextEdit> =
                edits.into_iter().map(|(s, t)| TextEdit { range: range_of(text, lines, s), new_text: t }).collect();
            let mut changes = HashMap::new();
            changes.insert(lsp_uri(uri), edits);
            out.push(CodeActionOrCommand::CodeAction(CodeAction {
                title,
                kind: Some(kind),
                edit: Some(WorkspaceEdit { changes: Some(changes), ..Default::default() }),
                ..Default::default()
            }));
        }
    }
}

fn range_of(text: &str, lines: &LineIndex, s: Span) -> Range {
    range(text, lines, s)
}

// ---------------------------------------------------------------------------------------------
// shared: scopes, statement lists, expression context

/// The innermost function body around `at` (`None`: top-level code). `Err` when `at` is in an
/// arrow function, which has no statements to put a declaration in.
struct FindScope<'a> {
    at: Span,
    found: Option<&'a FunctionDecl>,
    in_arrow: bool,
}

impl<'a> Visitor<'a> for FindScope<'a> {
    fn visit_function(&mut self, f: &'a FunctionDecl) {
        if f.span.start <= self.at.start && self.at.end <= f.span.end {
            if let Some(b) = &f.arrow_body {
                if b.span.start <= self.at.start && self.at.end <= b.span.end {
                    self.in_arrow = true;
                }
            } else if f.body.is_some() {
                self.found = Some(f);
                self.in_arrow = false;
            }
        }
        walk_function(self, f);
    }
}

fn scope_of(stmts: &[Stmt], at: Span) -> (Option<&FunctionDecl>, bool) {
    let mut f = FindScope { at, found: None, in_arrow: false };
    walk_stmts(&mut f, stmts);
    (f.found, f.in_arrow)
}

fn body_of<'a>(stmts: &'a [Stmt], f: Option<&'a FunctionDecl>) -> &'a [Stmt] {
    match f {
        Some(f) => f.body.as_deref().unwrap_or(&[]),
        None => stmts,
    }
}

/// Statement lists directly inside `s` (blocks of control statements, try / catch, `{ }`,
/// namespace bodies). Nested functions and classes are their own scope.
fn child_lists(s: &Stmt) -> Vec<&[Stmt]> {
    match &s.kind {
        StmtKind::Namespace { body: Some(b), .. } => vec![b],
        StmtKind::Control { blocks, .. } => blocks.iter().map(|b| &b[..]).collect(),
        StmtKind::Try { body, catches, finally } => {
            let mut v: Vec<&[Stmt]> = vec![body];
            v.extend(catches.iter().map(|c| &c.body[..]));
            if let Some(f) = finally {
                v.push(f);
            }
            v
        }
        StmtKind::Block(b) => vec![b],
        _ => Vec::new(),
    }
}

/// The statement (and its list) a declaration for code at `at` goes before. `None` when a
/// declaration there would run at another time than the code (loop conditions, `elseif`).
fn anchor(list: &[Stmt], at: Span) -> Option<(&[Stmt], usize)> {
    for (i, s) in list.iter().enumerate() {
        if !(s.span.start <= at.start && at.end <= s.span.end) {
            continue;
        }
        for l in child_lists(s) {
            if l.iter().any(|c| c.span.start <= at.start && at.end <= c.span.end) {
                return anchor(l, at);
            }
        }
        match &s.kind {
            StmtKind::Control { kw, header, .. } => {
                let k = header.iter().position(|h| h.span.start <= at.start && at.end <= h.span.end)?;
                if k != 0 || !matches!(*kw, "if" | "foreach" | "switch") {
                    return None;
                }
            }
            StmtKind::Function(_) | StmtKind::Class(_) | StmtKind::Namespace { .. } => return None,
            _ => {}
        }
        return Some((list, i));
    }
    None
}

/// A declaration can go right before `s`: it starts its own line, after `{`, `}`, `;` or `:`.
fn starts_line(src: &str, s: &Stmt) -> Option<String> {
    let start = s.span.start as usize;
    let line_start = src[..start].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let indent = &src[line_start..start];
    if !indent.chars().all(|c| c == ' ' || c == '\t') {
        return None;
    }
    let before = src[..line_start].trim_end();
    let ok = before.is_empty()
        || before.ends_with(['{', '}', ';', ':'])
        || before.ends_with("<?php")
        || before.ends_with("*/")
        || before.lines().last().is_some_and(|l| l.trim_start().starts_with("//") || l.trim_start().starts_with('#'));
    ok.then(|| indent.to_string())
}

/// Spans of expressions whose value is written or that must stay a variable.
#[derive(Default)]
struct Context {
    lvalues: Vec<Span>,
    in_string: Vec<Span>,
}

impl<'a> Visitor<'a> for Context {
    fn visit_expr(&mut self, e: &'a Expr) {
        match &e.kind {
            ExprKind::Assign { target, .. } => self.mark_target(target),
            ExprKind::Unary { op: "++" | "--" | "p++" | "p--" | "&", expr } => self.lvalues.push(expr.span),
            ExprKind::Isset(items) => self.lvalues.extend(items.iter().map(|i| i.span)),
            ExprKind::Empty(x) => self.lvalues.push(x.span),
            ExprKind::Array(items) => {
                for it in items.iter().filter(|i| i.by_ref) {
                    self.lvalues.push(it.value.span);
                }
            }
            ExprKind::Interp(parts) => self.in_string.extend(parts.iter().map(|p| p.span)),
            _ => {}
        }
        walk_expr(self, e);
    }

    fn visit_stmt(&mut self, s: &'a Stmt) {
        if let StmtKind::Control { kw: "foreach", header, .. } = &s.kind {
            for h in header.iter().skip(1) {
                self.mark_target(h);
            }
        }
        php_syntax::walk::walk_stmt(self, s);
    }
}

impl Context {
    /// `$a`, `$a[1]`, `$a->b`, `[$a, $b]` on the left of `=`: the expression and every base of it.
    fn mark_target(&mut self, t: &Expr) {
        self.lvalues.push(t.span);
        match &t.kind {
            ExprKind::Index { base, .. } => self.mark_target(base),
            ExprKind::Prop { obj, .. } => self.lvalues.push(obj.span),
            ExprKind::Array(items) => items.iter().for_each(|i| self.mark_target(&i.value)),
            ExprKind::Call { callee, args } if matches!(&callee.kind, ExprKind::Name(n) if n.text.eq_ignore_ascii_case("list")) => {
                args.iter().for_each(|a| self.mark_target(&a.value))
            }
            _ => {}
        }
    }

    fn is_lvalue(&self, s: Span) -> bool {
        self.lvalues.contains(&s)
    }

    fn in_string(&self, s: Span) -> bool {
        self.in_string.iter().any(|l| l.start <= s.start && s.end <= l.end)
    }
}

fn context(stmts: &[Stmt]) -> Context {
    let mut c = Context::default();
    walk_stmts(&mut c, stmts);
    c
}

// ---------------------------------------------------------------------------------------------
// extract variable

/// Every expression in `stmts`, outermost first (nested functions included).
struct Exprs<'a>(Vec<&'a Expr>);

impl<'a> Visitor<'a> for Exprs<'a> {
    fn visit_expr(&mut self, e: &'a Expr) {
        self.0.push(e);
        walk_expr(self, e);
    }
}

fn all_exprs(stmts: &[Stmt]) -> Vec<&Expr> {
    let mut v = Exprs(Vec::new());
    walk_stmts(&mut v, stmts);
    v.0
}

/// Expressions in `stmts` without entering nested functions (copies to replace).
struct ScopeExprs<'a>(Vec<&'a Expr>);

impl<'a> Visitor<'a> for ScopeExprs<'a> {
    fn visit_expr(&mut self, e: &'a Expr) {
        self.0.push(e);
        walk_expr(self, e);
    }
    fn visit_function(&mut self, _: &'a FunctionDecl) {}
    fn visit_class(&mut self, _: &'a ClassDecl) {}
}

pub fn extract_variable(src: &str, stmts: &[Stmt], sel: Span) -> Vec<Action> {
    let exprs = all_exprs(stmts);
    let target = if sel.start == sel.end {
        // innermost call / member access under the cursor
        exprs
            .iter()
            .filter(|e| e.span.start < sel.start && sel.start <= e.span.end)
            .filter(|e| {
                matches!(
                    e.kind,
                    ExprKind::Call { .. }
                        | ExprKind::MethodCall { .. }
                        | ExprKind::StaticCall { .. }
                        | ExprKind::New { .. }
                        | ExprKind::Prop { .. }
                        | ExprKind::StaticProp { .. }
                )
            })
            .min_by_key(|e| e.span.end - e.span.start)
            .copied()
    } else {
        let t = &src[sel.start as usize..sel.end as usize];
        let lead = t.len() - t.trim_start().len();
        let trimmed = t.trim().trim_end_matches(';').trim_end();
        let want = Span::new(sel.start as usize + lead, sel.start as usize + lead + trimmed.len());
        exprs.iter().find(|e| e.span == want).copied()
    };
    let Some(target) = target else { return Vec::new() };
    if matches!(
        target.kind,
        ExprKind::Var(_)
            | ExprKind::Assign { .. }
            | ExprKind::Closure(_)
            | ExprKind::Throw(_)
            | ExprKind::Exit(_)
            | ExprKind::Error
    ) {
        return Vec::new();
    }
    let ctx = context(stmts);
    if ctx.is_lvalue(target.span) || ctx.in_string(target.span) {
        return Vec::new();
    }
    let (scope, in_arrow) = scope_of(stmts, target.span);
    if in_arrow {
        return Vec::new();
    }
    let body = body_of(stmts, scope);
    let Some((list, i)) = anchor(body, target.span) else { return Vec::new() };
    let Some(indent) = starts_line(src, &list[i]) else { return Vec::new() };

    let name = unique_name(&suggest_name(src, target), body, scope);
    let value = target.span.text(src);
    let decl =
        (Span::new(list[i].span.start as usize, list[i].span.start as usize), format!("${name} = {value};\n{indent}"));
    let var = format!("${name}");
    let mut out = vec![("Extract variable".to_string(), vec![decl.clone(), (target.span, var.clone())])];

    // copies of the same expression from this statement on, in the same block
    let norm = |s: &str| s.chars().filter(|c| !c.is_whitespace()).collect::<String>();
    let key = norm(value);
    let mut finder = ScopeExprs(Vec::new());
    walk_stmts(&mut finder, &list[i..]);
    let mut copies: Vec<Span> = Vec::new();
    for e in finder.0 {
        if e.span.start < list[i].span.start || norm(e.span.text(src)) != key {
            continue;
        }
        if copies.iter().any(|c| c.start <= e.span.start && e.span.end <= c.end) {
            continue; // inside a copy already taken
        }
        if ctx.is_lvalue(e.span) || ctx.in_string(e.span) || anchor(&list[i..], e.span).is_none() {
            continue;
        }
        copies.push(e.span);
    }
    if copies.len() > 1 {
        let mut edits = vec![decl];
        edits.extend(copies.iter().map(|c| (*c, var.clone())));
        out.push((format!("Extract variable (replace all {} occurrences)", copies.len()), edits));
    }
    out
}

/// `$user->getProfile()` -> `profile`, `$order->total` -> `total`, `new Invoice` -> `invoice`,
/// `count($items)` -> `count`, `$row['email']` -> `email`.
fn suggest_name(src: &str, e: &Expr) -> String {
    let raw: String = match &e.kind {
        ExprKind::MethodCall { name: MemberName::Ident(i), .. }
        | ExprKind::StaticCall { name: MemberName::Ident(i), .. }
        | ExprKind::Prop { name: MemberName::Ident(i), .. }
        | ExprKind::StaticProp { name: MemberName::Ident(i), .. } => i.text.trim_start_matches('$').to_string(),
        ExprKind::Call { callee, .. } => match &callee.kind {
            ExprKind::Name(n) => n.text.rsplit('\\').next().unwrap_or("").to_string(),
            _ => String::new(),
        },
        ExprKind::New { class: ClassRef::Name(n), .. } => n.text.rsplit('\\').next().unwrap_or("").to_string(),
        ExprKind::Index { index: Some(i), .. } => i.string_value(src).unwrap_or("").to_string(),
        ExprKind::Array(_) => "items".into(),
        ExprKind::Lit if e.string_value(src).is_some() => "text".into(),
        ExprKind::Lit => "number".into(),
        ExprKind::Interp(_) | ExprKind::Binary { op: ".", .. } => "text".into(),
        ExprKind::Binary {
            op: "&&" | "||" | "and" | "or" | "==" | "===" | "!=" | "!==" | "<" | ">" | "<=" | ">=",
            ..
        }
        | ExprKind::InstanceOf { .. }
        | ExprKind::Unary { op: "!", .. } => "condition".into(),
        ExprKind::ClassConst { name, .. } if name.text.eq_ignore_ascii_case("class") => "class".into(),
        ExprKind::ClassConst { name, .. } => name.text.to_ascii_lowercase(),
        _ => String::new(),
    };
    let mut name = raw.as_str();
    for prefix in ["get", "find", "fetch", "load", "create", "make", "build", "to", "as"] {
        if let Some(rest) = name.strip_prefix(prefix) {
            if rest.chars().next().is_some_and(|c| c.is_ascii_uppercase() || c == '_') {
                name = rest.trim_start_matches('_');
                break;
            }
        }
    }
    let name = camel_lower(name);
    if name.is_empty() || !name.chars().next().is_some_and(|c| c.is_alphabetic() || c == '_') {
        return "value".into();
    }
    if matches!(
        name.as_str(),
        "this" | "class" | "list" | "array" | "new" | "function" | "fn" | "static" | "parent" | "self"
    ) {
        return format!("{name}Value");
    }
    name
}

/// `UserName` -> `userName`, `USER_NAME` -> `userName`, `user_name` stays.
fn camel_lower(s: &str) -> String {
    let s: String = s.chars().filter(|c| c.is_alphanumeric() || *c == '_').collect();
    if s.chars().all(|c| !c.is_lowercase()) && s.chars().any(|c| c.is_alphabetic()) {
        // CONSTANT_CASE
        let mut out = String::new();
        for (i, part) in s.split('_').filter(|p| !p.is_empty()).enumerate() {
            let p = part.to_ascii_lowercase();
            if i == 0 {
                out.push_str(&p);
            } else {
                let mut c = p.chars();
                if let Some(f) = c.next() {
                    out.push(f.to_ascii_uppercase());
                    out.push_str(c.as_str());
                }
            }
        }
        return out;
    }
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_lowercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// `name`, else `name2`, `name3` … not used in the function yet.
fn unique_name(name: &str, body: &[Stmt], scope: Option<&FunctionDecl>) -> String {
    let mut used: Vec<String> = all_exprs(body)
        .iter()
        .filter_map(|e| match &e.kind {
            ExprKind::Var(v) => Some(v.clone()),
            _ => None,
        })
        .collect();
    if let Some(f) = scope {
        used.extend(f.params.iter().map(|p| p.name.clone()));
        used.extend(f.uses.iter().map(|u| u.name.clone()));
    }
    if !used.iter().any(|u| u == name) {
        return name.to_string();
    }
    (2..).map(|n| format!("{name}{n}")).find(|c| !used.contains(c)).unwrap()
}

// ---------------------------------------------------------------------------------------------
// inline variable

/// Every `$name` in the scope (not inside nested functions, except arrow functions), plus
/// whether anything else in the scope makes inlining unsafe.
struct VarUses<'a> {
    src: &'a str,
    name: &'a str,
    uses: Vec<Span>,
    unsafe_: bool,
}

impl<'a> Visitor<'a> for VarUses<'_> {
    fn visit_expr(&mut self, e: &'a Expr) {
        match &e.kind {
            ExprKind::Var(v) if v == self.name => self.uses.push(e.span),
            ExprKind::VarVar(_) => self.unsafe_ = true,
            ExprKind::Call { callee, .. } => {
                if let ExprKind::Name(n) = &callee.kind {
                    let l = n.text.trim_start_matches('\\').to_ascii_lowercase();
                    if matches!(l.as_str(), "compact" | "extract" | "get_defined_vars" | "parse_str" | "eval") {
                        self.unsafe_ = true;
                    }
                }
            }
            _ => {}
        }
        walk_expr(self, e);
    }

    fn visit_function(&mut self, f: &'a FunctionDecl) {
        if f.arrow_body.is_some() {
            if f.params.iter().any(|p| p.name == self.name) {
                self.unsafe_ = true;
            }
            walk_function(self, f);
        } else if f.uses.iter().any(|u| u.name == self.name) {
            // a closure takes the variable with `use`: inlining would break the `use` list
            self.unsafe_ = true;
        }
    }

    fn visit_class(&mut self, _: &'a ClassDecl) {}

    fn visit_stmt(&mut self, s: &'a Stmt) {
        // `global $x;`, `static $x;`, `unset($x);` (statement keywords are not in the tree)
        if matches!(s.kind, StmtKind::Exprs(_)) {
            let t = s.span.text(self.src);
            let kw = t.trim_start().to_ascii_lowercase();
            if ["global", "static", "unset"].iter().any(|k| kw.starts_with(k)) && t.contains(&format!("${}", self.name))
            {
                self.unsafe_ = true;
            }
        }
        php_syntax::walk::walk_stmt(self, s);
    }
}

pub fn inline_variable(src: &str, stmts: &[Stmt], offset: usize) -> Option<Action> {
    // the variable under the cursor
    let at = all_exprs(stmts)
        .into_iter()
        .filter(|e| matches!(&e.kind, ExprKind::Var(v) if v != "this") && e.span.contains(offset))
        .min_by_key(|e| e.span.end - e.span.start)?;
    let ExprKind::Var(name) = &at.kind else { return None };
    let (scope, _) = scope_of(stmts, at.span);
    if scope.is_some_and(|f| f.params.iter().any(|p| &p.name == name) || f.uses.iter().any(|u| &u.name == name)) {
        return None;
    }
    let body = body_of(stmts, scope);

    let mut uses = VarUses { src, name, uses: Vec::new(), unsafe_: false };
    walk_stmts(&mut uses, body);
    if uses.unsafe_ {
        return None;
    }

    // exactly one write: a `$name = value;` statement
    let ctx = context(body);
    let writes: Vec<Span> = uses.uses.iter().copied().filter(|u| ctx.is_lvalue(*u)).collect();
    let [write] = writes[..] else { return None };
    let (list, i) = assignment_stmt(body, write)?;
    let StmtKind::Expr(Expr { kind: ExprKind::Assign { value, op: "=", by_ref: false, .. }, .. }) = &list[i].kind
    else {
        return None;
    };
    let block_end = list.last().map(|s| s.span.end).unwrap_or(0);
    let reads: Vec<Span> = uses.uses.iter().copied().filter(|u| *u != write).collect();
    if reads.is_empty() || reads.iter().any(|r| r.start < list[i].span.end || r.end > block_end || ctx.in_string(*r)) {
        return None;
    }
    if value_mentions(value, name) {
        return None;
    }

    let text = value.span.text(src);
    let atomic = matches!(
        value.kind,
        ExprKind::Var(_)
            | ExprKind::Call { .. }
            | ExprKind::MethodCall { .. }
            | ExprKind::StaticCall { .. }
            | ExprKind::Prop { .. }
            | ExprKind::StaticProp { .. }
            | ExprKind::ClassConst { .. }
            | ExprKind::Index { .. }
            | ExprKind::Lit
            | ExprKind::Interp(_)
            | ExprKind::Name(_)
            | ExprKind::Array(_)
            | ExprKind::Paren(_)
            | ExprKind::Match { .. }
    );
    let exprs = all_exprs(body);
    let mut edits = Vec::new();
    for r in &reads {
        // `new Foo` as the object of `->`/`::`/`[]` needs parentheses
        let is_base = exprs.iter().any(|e| match &e.kind {
            ExprKind::MethodCall { obj, .. } | ExprKind::Prop { obj, .. } => obj.span == *r,
            ExprKind::Index { base, .. } => base.span == *r,
            ExprKind::StaticCall { class: ClassRef::Expr(c), .. }
            | ExprKind::StaticProp { class: ClassRef::Expr(c), .. } => c.span == *r,
            _ => false,
        });
        let wrap = (!atomic && !is_whole_stmt(body, *r)) || (is_base && matches!(value.kind, ExprKind::New { .. }));
        edits.push((*r, if wrap { format!("({text})") } else { text.to_string() }));
    }
    edits.push((removal(src, &list[i]), String::new()));
    Some((format!("Inline variable '${name}'"), edits))
}

/// The statement list and index of the expression statement whose assignment target is `write`.
fn assignment_stmt(list: &[Stmt], write: Span) -> Option<(&[Stmt], usize)> {
    for (i, s) in list.iter().enumerate() {
        if !(s.span.start <= write.start && write.end <= s.span.end) {
            continue;
        }
        if let StmtKind::Expr(Expr { kind: ExprKind::Assign { target, .. }, .. }) = &s.kind {
            if target.span == write {
                return Some((list, i));
            }
        }
        for l in child_lists(s) {
            if let Some(r) = assignment_stmt(l, write) {
                return Some(r);
            }
        }
        return None;
    }
    None
}

fn value_mentions(value: &Expr, name: &str) -> bool {
    struct M<'n>(&'n str, bool);
    impl<'a> Visitor<'a> for M<'_> {
        fn visit_expr(&mut self, e: &'a Expr) {
            if matches!(&e.kind, ExprKind::Var(v) if v == self.0) {
                self.1 = true;
            }
            walk_expr(self, e);
        }
    }
    let mut m = M(name, false);
    m.visit_expr(value);
    m.1
}

fn is_whole_stmt(list: &[Stmt], s: Span) -> bool {
    struct W(Span, bool);
    impl<'a> Visitor<'a> for W {
        fn visit_stmt(&mut self, st: &'a Stmt) {
            if let StmtKind::Expr(e) = &st.kind {
                if e.span == self.0 {
                    self.1 = true;
                }
            }
            php_syntax::walk::walk_stmt(self, st);
        }
    }
    let mut w = W(s, false);
    walk_stmts(&mut w, list);
    w.1
}

/// The statement plus its line when nothing else is on it (and a `@var` docblock right above).
fn removal(src: &str, s: &Stmt) -> Span {
    let mut start = s.doc.map(|d| d.start).unwrap_or(s.span.start) as usize;
    let mut end = s.span.end as usize;
    let line_start = src[..start].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let rest = &src[end..];
    let line_end = rest.find('\n').map(|i| end + i + 1).unwrap_or(src.len());
    if src[line_start..start].trim().is_empty() && src[end..line_end].trim().is_empty() {
        start = line_start;
        end = line_end;
    }
    Span::new(start, end)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(src: &str, edits: &[(Span, String)]) -> String {
        let mut e = edits.to_vec();
        e.sort_by_key(|(s, _)| std::cmp::Reverse((s.start, s.end)));
        let mut out = src.to_string();
        for (s, t) in e {
            out.replace_range(s.start as usize..s.end as usize, &t);
        }
        out
    }

    fn sel(src: &str, needle: &str) -> Span {
        let i = src.find(needle).unwrap();
        Span::new(i, i + needle.len())
    }

    fn extract(src: &str, needle: &str) -> Vec<(String, String)> {
        let p = php_syntax::parse(src);
        extract_variable(src, &p.stmts, sel(src, needle)).into_iter().map(|(t, e)| (t, apply(src, &e))).collect()
    }

    fn inline(src: &str, needle: &str) -> Option<String> {
        let p = php_syntax::parse(src);
        let at = src.find(needle).unwrap() + 1;
        inline_variable(src, &p.stmts, at).map(|(_, e)| apply(src, &e))
    }

    #[test]
    fn extracts_selection() {
        let src = "<?php\nfunction f($user) {\n    if ($user->profile->address->city === 'A') {\n        send($user->profile->address->city);\n    }\n}\n";
        let r = extract(src, "$user->profile->address->city");
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].1, "<?php\nfunction f($user) {\n    $city = $user->profile->address->city;\n    if ($city === 'A') {\n        send($user->profile->address->city);\n    }\n}\n");
        assert_eq!(r[1].0, "Extract variable (replace all 2 occurrences)");
        assert_eq!(r[1].1, "<?php\nfunction f($user) {\n    $city = $user->profile->address->city;\n    if ($city === 'A') {\n        send($city);\n    }\n}\n");
    }

    #[test]
    fn extracts_call_under_cursor() {
        let src = "<?php\nfunction f($repo) {\n    return $repo->getUser(1)->name;\n}\n";
        let p = php_syntax::parse(src);
        let at = src.find("getUser").unwrap() + 2;
        let r = extract_variable(src, &p.stmts, Span::new(at, at));
        assert_eq!(
            apply(src, &r[0].1),
            "<?php\nfunction f($repo) {\n    $user = $repo->getUser(1);\n    return $user->name;\n}\n"
        );
    }

    #[test]
    fn extract_avoids_names_in_use_and_unsafe_places() {
        let src = "<?php\nfunction f($a, $user) {\n    $x = count($a);\n    while (count($a) > 1) {}\n    $y = fn() => count($a);\n    return count($a);\n}\n";
        // loop condition: evaluated every time round
        let p = php_syntax::parse(src);
        let i = src.match_indices("count($a)").nth(1).unwrap().0;
        assert!(extract_variable(src, &p.stmts, Span::new(i, i + 9)).is_empty());
        let i = src.match_indices("count($a)").nth(2).unwrap().0;
        assert!(extract_variable(src, &p.stmts, Span::new(i, i + 9)).is_empty(), "arrow function");
        // a name already taken gets a number
        let src2 = "<?php\nfunction f($user, $repo) {\n    return $repo->getUser();\n}\n";
        assert!(extract(src2, "$repo->getUser()")[0].1.contains("$user2 = $repo->getUser();"));
        // braceless body
        let src3 = "<?php\nfunction f($a) {\n    if ($a)\n        return strlen($a);\n}\n";
        assert!(extract(src3, "strlen($a)").is_empty());
        // assignment target
        let src4 = "<?php\nfunction f($a) {\n    $a->b->c = 1;\n}\n";
        assert!(extract(src4, "$a->b->c").is_empty());
    }

    #[test]
    fn inlines() {
        let src = "<?php\nfunction f($order) {\n    $total = $order->sum();\n    return $total;\n}\n";
        assert_eq!(inline(src, "$total;").unwrap(), "<?php\nfunction f($order) {\n    return $order->sum();\n}\n");
        // from the declaration, several reads, parentheses where needed
        let src = "<?php\nfunction f($a, $b) {\n    $sum = $a + $b;\n    echo $sum * 2;\n    echo $sum;\n}\n";
        assert_eq!(
            inline(src, "$sum =").unwrap(),
            "<?php\nfunction f($a, $b) {\n    echo ($a + $b) * 2;\n    echo ($a + $b);\n}\n"
        );
        let src = "<?php\nfunction f() {\n    $o = new Foo;\n    $o->run();\n}\n";
        assert_eq!(inline(src, "$o->").unwrap(), "<?php\nfunction f() {\n    (new Foo)->run();\n}\n");
    }

    #[test]
    fn inline_refuses_unsafe() {
        // written twice
        assert!(inline("<?php\nfunction f() {\n    $x = 1;\n    $x = 2;\n    echo $x;\n}\n", "$x;").is_none());
        // read inside a string
        assert!(inline("<?php\nfunction f() {\n    $x = a();\n    echo \"v: $x\";\n}\n", "$x =").is_none());
        // assigned in a branch, read after it
        assert!(inline("<?php\nfunction f($c) {\n    if ($c) {\n        $x = a();\n    }\n    echo $x;\n}\n", "$x;")
            .is_none());
        // closure `use`
        assert!(inline(
            "<?php\nfunction f() {\n    $x = a();\n    $g = function () use ($x) { return $x; };\n}\n",
            "$x ="
        )
        .is_none());
        // compact()
        assert!(inline("<?php\nfunction f() {\n    $x = a();\n    return compact('x');\n}\n", "$x =").is_none());
        // a parameter
        assert!(inline("<?php\nfunction f($x) {\n    echo $x;\n}\n", "$x;").is_none());
        // array append
        assert!(inline("<?php\nfunction f() {\n    $x = [];\n    $x[] = 1;\n    return $x;\n}\n", "$x =").is_none());
    }
}
