//! "Add return type", "Add type" (parameters, properties) and "Add PHPDoc" on the declaration
//! under the cursor. The types come from `php_index::suggest`; this file finds the declaration,
//! renders class names (adding `use` imports) and builds the edits.

use crate::convert::*;
use crate::server::Server;
use lsp_types::*;
use php_index::{Analysis, PhpVersion, Place, SuggestedType, SymbolKind, TypeSuggester};
use php_syntax::walk::{walk_class, walk_function, Visitor};
use php_syntax::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::OnceLock;

/// The declaration the cursor is on.
enum Target<'a> {
    Function(&'a FunctionDecl),
    /// `shared`: one declaration of several properties (`public $a, $b;`), left alone.
    Property {
        prop: &'a PropertyDecl,
        shared: bool,
    },
}

struct FindTarget<'a> {
    src: &'a str,
    offset: usize,
    found: Option<Target<'a>>,
}

impl<'a> Visitor<'a> for FindTarget<'a> {
    fn visit_class(&mut self, c: &'a ClassDecl) {
        for m in &c.members {
            let Member::Property(p) = m else { continue };
            if p.span.contains(self.offset) {
                let same_decl = |m: &Member| matches!(m, Member::Property(q) if q.span.start == p.span.start);
                let shared = c.members.iter().filter(|m| same_decl(m)).count() > 1;
                self.found = Some(Target::Property { prop: p, shared });
            }
        }
        walk_class(self, c);
    }

    fn visit_function(&mut self, f: &'a FunctionDecl) {
        if signature(self.src, f).is_some_and(|s| s.start <= self.offset && self.offset <= s.header_end) {
            self.found = Some(Target::Function(f));
        }
        walk_function(self, f);
    }
}

/// Where things go in a function's signature.
struct Signature {
    /// Start of the declaration (modifiers included, attributes not).
    start: usize,
    /// Just after the parameter list (or the closure's `use (…)`): where `: type` goes.
    ret_at: usize,
    /// End of the header: the body's `{`, the arrow's `=>` or the `;` of an abstract method.
    header_end: usize,
}

fn signature(src: &str, f: &FunctionDecl) -> Option<Signature> {
    let from = f.name.as_ref().map(|n| n.span.end).unwrap_or(f.span.start) as usize;
    let open = from + src.get(from..)?.find('(')?;
    let mut ret_at = matching_paren(src, open)? + 1;
    if !f.uses.is_empty() {
        let rest = &src[ret_at..];
        let use_kw = ret_at + (rest.len() - rest.trim_start().len());
        if src[use_kw..].len() >= 3 && src[use_kw..use_kw + 3].eq_ignore_ascii_case("use") {
            let uopen = use_kw + src[use_kw..].find('(')?;
            ret_at = matching_paren(src, uopen)? + 1;
        }
    }
    let after = f.ret.as_ref().map(|r| r.span.end as usize).unwrap_or(ret_at);
    let rest = &src[after..];
    let next = after + (rest.len() - rest.trim_start().len());
    let header_end = if src[next..].starts_with("=>") { next + 2 } else { (next + 1).min(src.len()) };
    Some(Signature { start: f.span.start as usize, ret_at, header_end })
}

/// Offset of the `)` closing the `(` at `open`; strings and comments are skipped.
fn matching_paren(src: &str, open: usize) -> Option<usize> {
    let b = src.as_bytes();
    let mut depth = 0usize;
    let mut i = open;
    while i < b.len() {
        match b[i] {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            q @ (b'\'' | b'"') => {
                i += 1;
                while i < b.len() && b[i] != q {
                    if b[i] == b'\\' {
                        i += 1;
                    }
                    i += 1;
                }
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                i = src[i + 2..].find("*/").map(|j| i + 2 + j + 1)?;
            }
            b'#' if b.get(i + 1) != Some(&b'[') => i = src[i..].find('\n').map(|j| i + j)?,
            b'/' if b.get(i + 1) == Some(&b'/') => i = src[i..].find('\n').map(|j| i + j)?,
            _ => {}
        }
        i += 1;
    }
    None
}

/// Where a parameter's type goes: before `&`, `...` and `$name`.
fn param_type_at(src: &str, p: &Param) -> usize {
    let before = &src[..p.span.start as usize];
    let pos = before.trim_end_matches(|c: char| c.is_whitespace() || c == '&' || c == '.').len();
    let rest = &src[pos..];
    pos + (rest.len() - rest.trim_start().len())
}

fn line_start(src: &str, offset: usize) -> usize {
    src[..offset].rfind('\n').map(|i| i + 1).unwrap_or(0)
}

fn indent_at(src: &str, offset: usize) -> String {
    src[line_start(src, offset)..].chars().take_while(|c| *c == ' ' || *c == '\t').collect()
}

/// Line where a docblock for a declaration starting at `start` (with these attributes) goes.
fn doc_line(src: &str, start: usize, attrs: &[Expr]) -> usize {
    let first = attrs.iter().map(|a| a.span.start as usize).min().unwrap_or(start).min(start);
    let attr_open = src[..first].rfind("#[").filter(|&i| src[i + 2..first].trim().is_empty()).unwrap_or(first);
    line_start(src, attr_open.min(start))
}

/// Class names as they should be written at one place in a file: imported alias, short name in
/// the same namespace, a new `use` import, or `\Fqn` when the short name is taken.
struct Names<'a> {
    analysis: &'a Analysis,
    index: &'a php_index::Index,
    /// Class imports of the file: (alias as written, FQN).
    uses: Vec<(String, String)>,
    /// Classes some suggestion imports, so two classes with one short name are not both imported.
    imported: RefCell<Vec<String>>,
}

impl Names<'_> {
    /// The text to write and whether it needs a new `use` import.
    fn name(&self, fqn: &str) -> (String, bool) {
        if let Some((alias, _)) = self.uses.iter().find(|(_, f)| f.eq_ignore_ascii_case(fqn)) {
            return (alias.clone(), false);
        }
        let ns = self.analysis.ctx.namespace.as_str();
        let short = fqn.rsplit('\\').next().unwrap_or(fqn);
        let fqn_ns = fqn[..fqn.len() - short.len()].trim_end_matches('\\');
        if fqn_ns.eq_ignore_ascii_case(ns) {
            return (short.to_string(), false);
        }
        if fqn_ns.is_empty() {
            return (format!("\\{fqn}"), false);
        }
        let same_short = |other: &str| {
            !other.eq_ignore_ascii_case(fqn) && other.rsplit('\\').next().is_some_and(|s| s.eq_ignore_ascii_case(short))
        };
        let alias_taken = self.uses.iter().any(|(a, _)| a.eq_ignore_ascii_case(short));
        let local = if ns.is_empty() { short.to_string() } else { format!("{ns}\\{short}") };
        if alias_taken
            || self.imported.borrow().iter().any(|i| same_short(i))
            || self.index.find_class(&local).is_some()
        {
            return (format!("\\{fqn}"), false);
        }
        let mut imported = self.imported.borrow_mut();
        if !imported.iter().any(|i| i.eq_ignore_ascii_case(fqn)) {
            imported.push(fqn.to_string());
        }
        (short.to_string(), true)
    }
}

fn class_uses(stmts: &[Stmt], out: &mut Vec<(String, String)>) {
    for s in stmts {
        match &s.kind {
            StmtKind::Use(items) => {
                for it in items.iter().filter(|i| i.kind == UseKind::Class) {
                    let alias =
                        it.alias.clone().unwrap_or_else(|| it.name.rsplit('\\').next().unwrap_or(&it.name).into());
                    out.push((alias, it.name.clone()));
                }
            }
            StmtKind::Namespace { body: Some(b), .. } => class_uses(b, out),
            _ => {}
        }
    }
}

/// `php -r 'echo PHP_VERSION;'`, asked once.
fn installed_php() -> Option<PhpVersion> {
    static V: OnceLock<Option<PhpVersion>> = OnceLock::new();
    *V.get_or_init(|| {
        let out = std::process::Command::new("php").args(["-r", "echo PHP_VERSION;"]).output().ok()?;
        PhpVersion::parse(String::from_utf8_lossy(&out.stdout).split('-').next()?)
    })
}

/// Collects the edits and titles of the actions for one declaration.
struct Actions<'a> {
    uri: &'a str,
    names: &'a Names<'a>,
    out: Vec<(String, Vec<TextEdit>)>,
}

impl Actions<'_> {
    fn push(&mut self, title: String, edits: Vec<TextEdit>) {
        self.out.push((title, edits));
    }
}

impl Server {
    /// The PHP version types must work with: the `phpLsp.phpVersion` setting, the project's
    /// composer.json, the installed `php`, or 7.4.
    pub fn php_version(&self, uri: &str) -> PhpVersion {
        if let Some(v) = PhpVersion::parse(&self.config.php_version) {
            return v;
        }
        let path = uri_to_path(uri).unwrap_or_default();
        let project = self
            .composers
            .iter()
            .filter(|c| path.starts_with(&c.root))
            .max_by_key(|c| c.root.components().count())
            .and_then(|c| c.php.as_deref());
        if let Some(p) = project {
            let exact = p.trim().starts_with(|c: char| c.is_ascii_digit()) && !p.contains(['|', ',', ' ', '*']);
            if let Some(v) = if exact { PhpVersion::parse(p) } else { PhpVersion::from_constraint(p) } {
                return v;
            }
        }
        installed_php().unwrap_or(PhpVersion(7, 4))
    }

    pub(super) fn type_actions(&mut self, uri: &str, range: Range, out: &mut Vec<CodeActionOrCommand>) {
        let Some(analysis) = self.analyze_doc(uri) else { return };
        let Some(doc) = self.docs.get(uri) else { return };
        let (text, lines) = (&doc.text, &doc.lines);
        let offset = offset_of(text, lines, range.start);
        let mut find = FindTarget { src: text, offset, found: None };
        php_syntax::walk::walk_stmts(&mut find, &doc.parse.stmts);
        let Some(target) = find.found else { return };

        let version = self.php_version(uri);
        let sg = TypeSuggester::new(&self.index, &analysis, text, uri);
        let mut uses = Vec::new();
        class_uses(&doc.parse.stmts, &mut uses);
        let names = Names { analysis: &analysis, index: &self.index, uses, imported: RefCell::new(Vec::new()) };
        let mut actions = Actions { uri, names: &names, out: Vec::new() };
        match target {
            Target::Function(f) => function_actions(&sg, text, lines, f, offset, version, &mut actions),
            Target::Property { prop, shared: false } => property_actions(&sg, text, lines, prop, version, &mut actions),
            Target::Property { shared: true, .. } => {}
        }

        for (title, mut edits) in actions.out {
            let imports = self.imports_for(uri, offset, &analysis, &edits);
            for e in &mut edits {
                e.new_text = unmark(&e.new_text);
            }
            edits.extend(imports);
            let mut changes = HashMap::new();
            changes.insert(lsp_uri(actions.uri), edits);
            out.push(CodeActionOrCommand::CodeAction(CodeAction {
                title,
                kind: Some(CodeActionKind::REFACTOR_REWRITE),
                edit: Some(WorkspaceEdit { changes: Some(changes), ..Default::default() }),
                ..Default::default()
            }));
        }
    }

    /// `use` imports for the class names an action writes (marked by [`marked`] in its edits).
    fn imports_for(&self, uri: &str, offset: usize, analysis: &Analysis, edits: &[TextEdit]) -> Vec<TextEdit> {
        let mut out = Vec::new();
        let mut seen: Vec<String> = Vec::new();
        for e in edits {
            for fqn in e.new_text.split('\0').skip(1).step_by(2) {
                if seen.iter().any(|s| s.eq_ignore_ascii_case(fqn)) {
                    continue;
                }
                seen.push(fqn.to_string());
                if let Some(edit) = self.import_edit(uri, offset, analysis, fqn, SymbolKind::Class) {
                    out.push(edit);
                }
            }
        }
        out
    }
}

fn function_actions(
    sg: &TypeSuggester,
    text: &str,
    lines: &LineIndex,
    f: &FunctionDecl,
    offset: usize,
    version: PhpVersion,
    out: &mut Actions,
) {
    let Some(sig) = signature(text, f) else { return };
    let insert = |at: usize, new_text: String| {
        let (line, character) = lines.position(text, at);
        let p = Position { line, character };
        TextEdit { range: Range { start: p, end: p }, new_text }
    };
    let names = out.names;
    let native = |t: &SuggestedType, place: Place| t.native(version, place, &|c| marked(names, c));

    let ret = sg.return_type(f);
    if let Some(t) = ret.as_ref().and_then(|t| native(t, Place::Return)) {
        out.push(format!("Add return type ': {}'", unmark(&t)), vec![insert(sig.ret_at, format!(": {t}"))]);
    }

    let params: Vec<Option<SuggestedType>> = (0..f.params.len()).map(|i| sg.param_type(f, i)).collect();
    // a promoted constructor parameter is also a property and follows the property rules
    let place = |i: usize| if f.params[i].promoted.is_some() { Place::Property } else { Place::Param };
    let typed: Vec<(usize, String)> =
        params.iter().enumerate().filter_map(|(i, t)| Some((i, native(t.as_ref()?, place(i))?))).collect();
    let on_param = f.params.iter().position(|p| {
        let end = p.default.as_ref().map(|d| d.span.end).unwrap_or(p.span.end) as usize;
        param_type_at(text, p) <= offset && offset <= end
    });
    let param_edit = |i: usize, t: &str| insert(param_type_at(text, &f.params[i]), format!("{t} "));
    match on_param {
        Some(i) => {
            if let Some((_, t)) = typed.iter().find(|(j, _)| *j == i) {
                out.push(format!("Add type '{}' to ${}", unmark(t), f.params[i].name), vec![param_edit(i, t)]);
            }
        }
        None if typed.len() == 1 => {
            let (i, t) = &typed[0];
            out.push(format!("Add type '{}' to ${}", unmark(t), f.params[*i].name), vec![param_edit(*i, t)]);
        }
        None if typed.len() > 1 => {
            let list = typed.iter().map(|(i, _)| format!("${}", f.params[*i].name)).collect::<Vec<_>>().join(", ");
            out.push(format!("Add types to {list}"), typed.iter().map(|(i, t)| param_edit(*i, t)).collect());
        }
        None => {}
    }

    if f.doc.is_none() && f.name.is_some() {
        let at = doc_line(text, sig.start, &f.attrs);
        let indent = indent_at(text, sig.start);
        let doc = function_doc(sg, text, f, &params, ret.as_ref(), names);
        let block = doc.iter().map(|l| format!("{indent} * {l}\n")).collect::<String>();
        out.push("Add PHPDoc".into(), vec![insert(at, format!("{indent}/**\n{block}{indent} */\n"))]);
    }
}

/// `@param`, `@return` and `@throws` lines for a function without a docblock.
fn function_doc(
    sg: &TypeSuggester,
    text: &str,
    f: &FunctionDecl,
    params: &[Option<SuggestedType>],
    ret: Option<&SuggestedType>,
    names: &Names,
) -> Vec<String> {
    let doc = |t: &SuggestedType| t.doc(&|c| marked(names, c));
    let mut lines = Vec::new();
    for (p, t) in f.params.iter().zip(params) {
        let ty = match (&p.ty, t) {
            (Some(h), _) => h.span.text(text).to_string(),
            (None, Some(t)) => doc(t),
            (None, None) => "mixed".to_string(),
        };
        lines.push(format!("@param {ty} {}${}", if p.variadic { "..." } else { "" }, p.name));
    }
    let ctor = f.name.as_ref().is_some_and(|n| n.text.eq_ignore_ascii_case("__construct"));
    if !ctor {
        let ty = match (&f.ret, ret) {
            (Some(h), _) => h.span.text(text).to_string(),
            (None, Some(t)) => doc(t),
            (None, None) => sg.inferred_return(f).map(|t| doc(&t)).unwrap_or_else(|| "mixed".to_string()),
        };
        lines.push(format!("@return {ty}"));
    }
    for t in sg.thrown(f) {
        lines.push(format!("@throws {t}"));
    }
    lines
}

fn property_actions(
    sg: &TypeSuggester,
    text: &str,
    lines: &LineIndex,
    p: &PropertyDecl,
    version: PhpVersion,
    out: &mut Actions,
) {
    let insert = |at: usize, new_text: String| {
        let (line, character) = lines.position(text, at);
        let pos = Position { line, character };
        TextEdit { range: Range { start: pos, end: pos }, new_text }
    };
    let names = out.names;
    let suggestion = sg.property_type(p);
    if let Some(s) = &suggestion {
        if let Some(t) = s.ty.native(version, Place::Property, &|c| marked(names, c)) {
            let mut edits = vec![insert(p.name.span.start as usize, format!("{t} "))];
            if s.add_null_default {
                edits.push(insert(p.name.span.end as usize, " = null".to_string()));
            }
            out.push(format!("Add type '{}' to ${}", unmark(&t), p.name.text), edits);
        }
    }
    if p.doc.is_none() {
        let ty = match (&p.ty, &suggestion) {
            (Some(h), _) => h.span.text(text).to_string(),
            (None, Some(s)) => s.ty.doc(&|c| marked(names, c)),
            (None, None) => "mixed".to_string(),
        };
        let start = p.span.start as usize;
        let indent = indent_at(text, start);
        out.push("Add PHPDoc".into(), vec![insert(line_start(text, start), format!("{indent}/** @var {ty} */\n"))]);
    }
}

/// A class name as it will be written, with the FQN attached (`User\0App\User\0`) when the name
/// needs a new `use` import; [`unmark`] strips the marker again.
fn marked(names: &Names, fqn: &str) -> String {
    match names.name(fqn) {
        (written, true) => format!("{written}\0{fqn}\0"),
        (written, false) => written,
    }
}

/// Text without import markers.
fn unmark(s: &str) -> String {
    s.split('\0').step_by(2).collect()
}
