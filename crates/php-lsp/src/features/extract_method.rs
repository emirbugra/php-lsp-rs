//! "Extract method": the selected statements become a new private method (a function when they
//! are in a plain function) called where they were.
//!
//! - parameters: the variables the statements use that already exist before them;
//! - return value: the variables they set that are read afterwards (one: `return $x;` and
//!   `$x = $this->extracted(…);`, several: `return [$a, $b];` and `[$a, $b] = …`);
//! - in a loop, a variable read earlier in the loop body counts as read afterwards (next round).
//!
//! Left alone: selections that are not whole statements of one block, and statements that jump
//! out of the selection (`return`, `break` / `continue` of an outer loop, `yield`, `goto`,
//! `global` / `static`), or use `compact()` / `extract()` / `$$x`.

use super::refactor::{body_of, child_lists, context, scope_of, Action};
use php_syntax::walk::{walk_expr, walk_function, walk_stmt, walk_stmts, Visitor};
use php_syntax::*;

const NAME: &str = "extracted";

pub fn extract_method(src: &str, stmts: &[Stmt], sel: Span) -> Option<Action> {
    if sel.start == sel.end {
        return None;
    }
    // the selection without surrounding whitespace
    let t = &src[sel.start as usize..sel.end as usize];
    let start = sel.start as usize + (t.len() - t.trim_start().len());
    let end = sel.start as usize + t.trim_end().len();
    let sel = Span::new(start, end.max(start));

    let (scope, in_arrow) = scope_of(stmts, sel);
    let scope = scope?;
    if in_arrow {
        return None;
    }
    let body = body_of(stmts, Some(scope));
    let (run, loop_span) = select_run(body, sel, None)?;
    let first = &run[0];
    let last = &run[run.len() - 1];
    let run_span = Span::new(first.span.start as usize, last.span.end as usize);

    // nothing may jump out of the statements
    let mut jumps = Jumps { src, loops: 0, bad: false };
    walk_stmts(&mut jumps, run);
    if jumps.bad {
        return None;
    }

    // variables before / inside / after the statements
    let mut vars = Vars { uses: Vec::new(), hidden: Vec::new() };
    walk_stmts(&mut vars, body);
    // `compact()` / `$$x` in or after the statements may read variables by name
    if vars.hidden.iter().any(|h| h.end > run_span.start) {
        return None;
    }
    let ctx = context(body);
    let mut known: Vec<String> = scope.params.iter().map(|p| p.name.clone()).collect();
    known.extend(scope.uses.iter().map(|u| u.name.clone()));
    let inside = |s: Span| run_span.start <= s.start && s.end <= run_span.end;
    let before = |s: Span| s.end <= run_span.start && !loop_span.is_some_and(|l: Span| l.start <= s.start);
    let after = |s: Span| {
        s.start >= run_span.end || loop_span.is_some_and(|l: Span| l.start <= s.start && s.end <= run_span.start)
    };

    let mut params: Vec<String> = Vec::new();
    let mut returns: Vec<String> = Vec::new();
    for (name, span) in &vars.uses {
        if !inside(*span) || name == "this" {
            continue;
        }
        let exists = known.contains(name) || vars.uses.iter().any(|(n, s)| n == name && before(*s));
        if exists && !params.contains(name) {
            params.push(name.clone());
        }
        let written = ctx.is_lvalue(*span);
        if written && !returns.contains(name) && vars.uses.iter().any(|(n, s)| n == name && after(*s)) {
            returns.push(name.clone());
        }
    }
    let uses_this = vars.uses.iter().any(|(n, s)| n == "this" && inside(*s)) || mentions_self(src, run_span);

    // where the new method / function goes, and how it is called
    let host = host_of(stmts, run_span)?;
    let (decl_end, indent, is_static) = match &host {
        Host::Method { decl, modifiers, .. } => {
            let start = member_start(src, decl.span.start as usize);
            (decl.span.end as usize, indent_at(src, start), modifiers.is_static)
        }
        Host::Function(f) => (f.span.end as usize, indent_at(src, f.span.start as usize), false),
    };
    if matches!(host, Host::Function(_)) && uses_this {
        return None;
    }
    let taken = |n: &str| match &host {
        Host::Method { class, .. } => class.members.iter().any(|m| match m {
            Member::Method { decl, .. } => decl.name.as_ref().is_some_and(|i| i.text.eq_ignore_ascii_case(n)),
            _ => false,
        }),
        Host::Function(_) => contains_word(src, &format!("function {n}")),
    };
    let name =
        if taken(NAME) { (2..).map(|i| format!("{NAME}{i}")).find(|n| !taken(n)).unwrap() } else { NAME.to_string() };

    // the statements, indented one level below the new declaration
    let unit = indent_unit(src, scope, &indent);
    let body_indent = format!("{indent}{unit}");
    let line_start = src[..run_span.start as usize].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let code = reindent(&src[line_start..run_span.end as usize], &body_indent);

    let param_list = params.iter().map(|p| format!("${p}")).collect::<Vec<_>>().join(", ");
    let ret = match returns.len() {
        0 => String::new(),
        1 => format!("\n{body_indent}return ${};", returns[0]),
        _ => format!(
            "\n{body_indent}return [{}];",
            returns.iter().map(|r| format!("${r}")).collect::<Vec<_>>().join(", ")
        ),
    };
    let brace_next_line = brace_on_next_line(src, scope);
    let open = if brace_next_line { format!("\n{indent}{{") } else { " {".to_string() };
    let header = match &host {
        Host::Method { .. } => {
            format!("private {}function {name}({param_list})", if is_static { "static " } else { "" })
        }
        Host::Function(_) => format!("function {name}({param_list})"),
    };
    let new_decl = format!("\n\n{indent}{header}{open}\n{code}{ret}\n{indent}}}");

    let callee = match (&host, is_static) {
        (Host::Method { .. }, false) => format!("$this->{name}({param_list})"),
        (Host::Method { .. }, true) => format!("self::{name}({param_list})"),
        (Host::Function(_), _) => format!("{name}({param_list})"),
    };
    let call = match returns.len() {
        0 => format!("{callee};"),
        1 => format!("${} = {callee};", returns[0]),
        _ => format!("[{}] = {callee};", returns.iter().map(|r| format!("${r}")).collect::<Vec<_>>().join(", ")),
    };
    Some(("Extract method".to_string(), vec![(run_span, call), (Span::new(decl_end, decl_end), new_decl)]))
}

/// The run of whole statements of one block the selection covers, and the innermost loop around
/// it (inside the function).
fn select_run(list: &[Stmt], sel: Span, loop_span: Option<Span>) -> Option<(&[Stmt], Option<Span>)> {
    let covered: Vec<usize> =
        (0..list.len()).filter(|&i| sel.start <= list[i].span.start && list[i].span.end <= sel.end).collect();
    if let (Some(&a), Some(&b)) = (covered.first(), covered.last()) {
        let partial = list.iter().any(|s| {
            let overlaps = s.span.start < sel.end && sel.start < s.span.end;
            let inside = sel.start <= s.span.start && s.span.end <= sel.end;
            overlaps && !inside
        });
        if partial || list[a].span.start != sel.start || list[b].span.end != sel.end {
            return None;
        }
        return Some((&list[a..=b], loop_span));
    }
    let s = list.iter().find(|s| s.span.start <= sel.start && sel.end <= s.span.end)?;
    let inner_loop = match &s.kind {
        StmtKind::Control { kw: "while" | "for" | "foreach" | "do", .. } => Some(s.span),
        _ => loop_span,
    };
    for l in child_lists(s) {
        if l.iter().any(|c| {
            c.span.start <= sel.start && sel.end <= c.span.end || sel.start <= c.span.start && c.span.end <= sel.end
        }) {
            return select_run(l, sel, inner_loop);
        }
    }
    None
}

/// `return`, `yield`, `goto`, `global` / `static`, or `break` / `continue` of a loop outside.
struct Jumps<'s> {
    src: &'s str,
    loops: u32,
    bad: bool,
}

impl<'a> Visitor<'a> for Jumps<'_> {
    fn visit_stmt(&mut self, s: &'a Stmt) {
        let kw = s.span.text(self.src).trim_start().to_ascii_lowercase();
        match &s.kind {
            StmtKind::Exprs(_) => {
                let word = |w: &str| {
                    kw.starts_with(w) && !kw[w.len()..].starts_with(|c: char| c.is_alphanumeric() || c == '_')
                };
                if word("return") || word("goto") || word("global") || word("static") {
                    self.bad = true;
                }
                if (word("break") || word("continue")) && self.loops == 0 {
                    self.bad = true;
                }
                walk_stmt(self, s);
            }
            StmtKind::Control { kw: "while" | "for" | "foreach" | "do" | "switch", .. } => {
                self.loops += 1;
                walk_stmt(self, s);
                self.loops -= 1;
            }
            _ => walk_stmt(self, s),
        }
    }

    fn visit_expr(&mut self, e: &'a Expr) {
        if matches!(e.kind, ExprKind::Yield { .. } | ExprKind::YieldFrom(_)) {
            self.bad = true;
        }
        walk_expr(self, e);
    }

    fn visit_function(&mut self, _: &'a FunctionDecl) {}
    fn visit_class(&mut self, _: &'a ClassDecl) {}
}

/// Every variable occurrence in the function body (closure `use` lists count where they are;
/// arrow functions are read through), and where variables are used by name.
struct Vars {
    uses: Vec<(String, Span)>,
    /// Where variables are used by name (`compact()`, `$$x` …).
    hidden: Vec<Span>,
}

impl<'a> Visitor<'a> for Vars {
    fn visit_expr(&mut self, e: &'a Expr) {
        match &e.kind {
            ExprKind::Var(v) => self.uses.push((v.clone(), e.span)),
            ExprKind::VarVar(_) => self.hidden.push(e.span),
            ExprKind::Call { callee, .. } => {
                if let ExprKind::Name(n) = &callee.kind {
                    let l = n.text.trim_start_matches('\\').to_ascii_lowercase();
                    if matches!(
                        l.as_str(),
                        "compact" | "extract" | "get_defined_vars" | "func_get_args" | "func_get_arg" | "func_num_args"
                    ) {
                        self.hidden.push(e.span);
                    }
                }
            }
            _ => {}
        }
        walk_expr(self, e);
    }

    fn visit_function(&mut self, f: &'a FunctionDecl) {
        if f.arrow_body.is_some() {
            walk_function(self, f);
        } else {
            for u in &f.uses {
                self.uses.push((u.name.clone(), u.span));
            }
        }
    }

    fn visit_class(&mut self, _: &'a ClassDecl) {}
}

/// `self::` / `static::` / `parent::` in the statements: they need a method, not a function.
fn mentions_self(src: &str, s: Span) -> bool {
    let t = s.text(src).to_ascii_lowercase();
    ["self::", "static::", "parent::"].iter().any(|k| t.contains(k))
}

enum Host<'a> {
    Method { class: &'a ClassDecl, decl: &'a FunctionDecl, modifiers: &'a Modifiers },
    Function(&'a FunctionDecl),
}

/// The class method (or named top-level function) the statements are in.
fn host_of(stmts: &[Stmt], at: Span) -> Option<Host<'_>> {
    struct H<'a> {
        at: Span,
        found: Option<Host<'a>>,
    }
    impl<'a> Visitor<'a> for H<'a> {
        fn visit_class(&mut self, c: &'a ClassDecl) {
            if c.name.is_some() {
                for m in &c.members {
                    if let Member::Method { decl, modifiers } = m {
                        if decl.span.start <= self.at.start && self.at.end <= decl.span.end {
                            self.found = Some(Host::Method { class: c, decl, modifiers });
                        }
                    }
                }
            }
            php_syntax::walk::walk_class(self, c);
        }
        fn visit_stmt(&mut self, s: &'a Stmt) {
            if let StmtKind::Function(f) = &s.kind {
                if f.span.start <= self.at.start && self.at.end <= f.span.end && self.found.is_none() {
                    self.found = Some(Host::Function(f));
                }
            }
            walk_stmt(self, s);
        }
    }
    let mut h = H { at, found: None };
    walk_stmts(&mut h, stmts);
    h.found
}

/// Start of a member declaration: back over its modifiers (`public static function`).
fn member_start(src: &str, fn_start: usize) -> usize {
    let line_start = src[..fn_start].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let before = &src[line_start..fn_start];
    if before.split_whitespace().all(|w| {
        matches!(w.to_ascii_lowercase().as_str(), "public" | "protected" | "private" | "static" | "final" | "abstract")
    }) {
        line_start + (before.len() - before.trim_start().len())
    } else {
        fn_start
    }
}

fn indent_at(src: &str, at: usize) -> String {
    let line_start = src[..at].rfind('\n').map(|i| i + 1).unwrap_or(0);
    src[line_start..at].chars().take_while(|c| *c == ' ' || *c == '\t').collect()
}

/// One indentation level, from the first statement of the function body (else 4 spaces).
fn indent_unit(src: &str, f: &FunctionDecl, decl_indent: &str) -> String {
    if let Some(s) = f.body.as_ref().and_then(|b| b.first()) {
        let i = indent_at(src, s.span.start as usize);
        if let Some(rest) = i.strip_prefix(decl_indent) {
            if !rest.is_empty() {
                return rest.to_string();
            }
        }
    }
    if decl_indent.contains('\t') {
        "\t".into()
    } else {
        "    ".into()
    }
}

fn brace_on_next_line(src: &str, f: &FunctionDecl) -> bool {
    let Some(first) = f.body.as_ref() else { return true };
    let open_search_end = first.first().map(|s| s.span.start as usize).unwrap_or(f.span.end as usize);
    let head = &src[f.span.start as usize..open_search_end];
    let Some(brace) = head.find('{') else { return true };
    let close_paren = head[..brace].rfind(')').unwrap_or(0);
    head[close_paren..brace].contains('\n')
}

/// `text` (whole lines) with its common indentation replaced by `indent`.
fn reindent(text: &str, indent: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let common = lines
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start_matches([' ', '\t']).len())
        .min()
        .unwrap_or(0);
    lines
        .iter()
        .map(|l| if l.trim().is_empty() { String::new() } else { format!("{indent}{}", &l[common.min(l.len())..]) })
        .collect::<Vec<_>>()
        .join("\n")
}

fn contains_word(src: &str, w: &str) -> bool {
    src.to_ascii_lowercase()
        .match_indices(&w.to_ascii_lowercase())
        .any(|(i, _)| !src[i + w.len()..].starts_with(|c: char| c.is_alphanumeric() || c == '_'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(src: &str, from: &str, to: &str) -> Option<String> {
        let a = src.find(from).unwrap();
        let b = src.find(to).unwrap() + to.len();
        let p = php_syntax::parse(src);
        let (_, edits) = extract_method(src, &p.stmts, Span::new(a, b))?;
        let mut e = edits;
        e.sort_by_key(|(s, _)| std::cmp::Reverse((s.start, s.end)));
        let mut out = src.to_string();
        for (s, t) in e {
            out.replace_range(s.start as usize..s.end as usize, &t);
        }
        Some(out)
    }

    #[test]
    fn method_with_params_and_return() {
        let src = "<?php\nclass Cart\n{\n    public function total(array $items, $tax)\n    {\n        $sum = 0;\n        foreach ($items as $i) {\n            $sum += $i->price;\n        }\n        $sum = $sum * $tax;\n        return $sum;\n    }\n}\n";
        let got = run(src, "foreach", "$sum * $tax;").unwrap();
        assert_eq!(
            got,
            "<?php\nclass Cart\n{\n    public function total(array $items, $tax)\n    {\n        $sum = 0;\n        $sum = $this->extracted($items, $sum, $tax);\n        return $sum;\n    }\n\n    private function extracted($items, $sum, $tax)\n    {\n        foreach ($items as $i) {\n            $sum += $i->price;\n        }\n        $sum = $sum * $tax;\n        return $sum;\n    }\n}\n"
        );
    }

    #[test]
    fn several_results_static_and_functions() {
        let src = "<?php\nclass A {\n    public static function f() {\n        $a = 1;\n        $b = 2;\n        echo $a + $b;\n    }\n}\n";
        let got = run(src, "$a = 1;", "$b = 2;").unwrap();
        assert!(got.contains("        [$a, $b] = self::extracted();\n"), "{got}");
        assert!(got.contains("    private static function extracted() {\n        $a = 1;\n        $b = 2;\n        return [$a, $b];\n    }"), "{got}");

        let src = "<?php\nfunction g($x) {\n    $y = $x * 2;\n    echo $y;\n}\n";
        let got = run(src, "$y = $x * 2;", "echo $y;").unwrap();
        assert!(
            got.contains("    extracted($x);\n}\n\nfunction extracted($x) {\n    $y = $x * 2;\n    echo $y;\n}"),
            "{got}"
        );
    }

    #[test]
    fn loop_reads_count_as_after() {
        let src = "<?php\nfunction g($items) {\n    $n = 0;\n    foreach ($items as $i) {\n        echo $n;\n        $n = $n + 1;\n    }\n}\n";
        let got = run(src, "$n = $n + 1;", "$n = $n + 1;").unwrap();
        assert!(got.contains("$n = extracted($n);"), "{got}");
    }

    #[test]
    fn refuses() {
        let src = "<?php\nfunction g($a) {\n    if ($a) {\n        return 1;\n    }\n    foreach ($a as $x) {\n        if ($x) { break; }\n    }\n    echo $a;\n}\n";
        assert!(run(src, "if ($a)", "    }").is_none(), "return");
        assert!(run(src, "if ($x)", "break; }").is_none(), "break of an outer loop");
        assert!(run(src, "foreach", "break; }\n    }").is_some(), "break of its own loop");
        // half a statement
        assert!(run(src, "echo $a", "echo $a").is_none());
        // `compact()` after the statements may read their variables
        let src = "<?php\nfunction g($a) {\n    $b = $a;\n    return compact('b');\n}\n";
        assert!(run(src, "$b = $a;", "$b = $a;").is_none(), "compact");
    }
}
