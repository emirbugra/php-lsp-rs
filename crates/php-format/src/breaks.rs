//! Line breaks between statements: the first formatting pass. It decides how many line breaks
//! the gaps between tokens get, from the statements and blocks the indentation engine recorded:
//!
//! - minimum blank lines (`BLANK_LINES_*`): after `<?php`, around `namespace`, imports, classes,
//!   methods, properties, constants, enum cases, after functions, before `return`;
//! - one statement per line (`KEEP_MULTIPLE_EXPRESSIONS_IN_ONE_LINE`, `CASE_STATEMENT_ON_NEW_LINE`);
//! - where `{` goes (`CLASS_BRACE_STYLE`, `METHOD_BRACE_STYLE`, `BRACE_STYLE`, …) and blocks opened
//!   up (`{ return 1; }` → three lines, `KEEP_SIMPLE_*_IN_ONE_LINE`);
//! - `} else`, `} catch`, `} finally`, `} while` (`ELSE_ON_NEW_LINE`, …).
//!
//! Only whitespace between tokens changes. Indentation and spaces are left to the later passes.

use crate::indent::{self, Body, Item};
use crate::options::FormatOptions;
use crate::space::Edit;
use crate::FormatError;
use php_syntax::lexer::{Tok, Token};
use rustc_hash::FxHashMap;

/// Brace placement values of PhpStorm's `*_BRACE_STYLE` options.
const END_OF_LINE: usize = 1;
const NEXT_LINE_IF_WRAPPED: usize = 5;

/// Line breaks wanted in one gap.
#[derive(Clone, Copy, Debug)]
struct Need {
    min: usize,
    max: usize,
}

impl Default for Need {
    fn default() -> Self {
        Need { min: 0, max: usize::MAX }
    }
}

/// What a statement / member is, for the blank-line rules.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    /// `namespace X;` (`braced`: `namespace X { … }`)
    Namespace {
        braced: bool,
    },
    /// `use …;` at the top: 0 classes, 1 functions, 2 constants
    Import(u8),
    /// `use SomeTrait;` in a class
    TraitUse,
    ClassLike,
    /// A named function at the top
    Function,
    Method,
    Property,
    Const,
    EnumCase,
    Return,
    /// `else`, `elseif`, `catch`, `finally`, the `while` of `do … while`
    Continuation,
    /// `case …:` / `default:`
    Label,
    Other,
}

/// A statement after merging attribute lines into the declaration below them.
#[derive(Clone, Copy, Debug)]
struct Stmt {
    first: usize,
    last: usize,
    kind: Kind,
}

struct Ctx<'a> {
    src: &'a str,
    toks: &'a [Token],
    o: &'a FormatOptions,
    /// Line of each token's start and end.
    start_line: Vec<usize>,
    end_line: Vec<usize>,
    needs: FxHashMap<usize, Need>,
}

const MODIFIERS: &[&str] = &["public", "protected", "private", "static", "abstract", "final", "readonly", "var"];

impl Ctx<'_> {
    fn text(&self, i: usize) -> &str {
        self.toks[i].span.text(self.src)
    }

    fn lower(&self, i: usize) -> String {
        self.text(i).to_ascii_lowercase()
    }

    fn is_trivia(&self, i: usize) -> bool {
        matches!(self.toks[i].kind, Tok::Comment | Tok::OpenTag)
    }

    /// Significant tokens of `first..=last`.
    fn sig(&self, first: usize, last: usize) -> impl Iterator<Item = usize> + '_ {
        (first..=last).filter(|&i| !self.is_trivia(i))
    }

    /// Is token `i` the first thing on its line?
    fn first_on_line(&self, i: usize) -> bool {
        i == 0 || self.end_line[i - 1] < self.start_line[i]
    }

    fn need(&mut self, gap: usize, f: impl FnOnce(&mut Need)) {
        // gaps touching HTML or PHP tags are not ours
        let touches_tags = |t: &Token| matches!(t.kind, Tok::InlineHtml | Tok::CloseTag | Tok::OpenTagEcho);
        if gap == 0 || gap >= self.toks.len() || touches_tags(&self.toks[gap - 1]) || touches_tags(&self.toks[gap]) {
            return;
        }
        f(self.needs.entry(gap).or_default());
    }

    fn blank_lines(&mut self, gap: usize, n: usize) {
        if n > 0 {
            self.need(gap, |nd| nd.min = nd.min.max(n + 1));
        }
    }

    fn newline(&mut self, gap: usize) {
        self.need(gap, |nd| nd.min = nd.min.max(1));
    }

    fn join(&mut self, gap: usize) {
        self.need(gap, |nd| nd.max = 0);
    }

    fn classify(&self, item: &Item) -> Kind {
        let toks: Vec<usize> = self.sig(item.first, item.last).collect();
        // skip attributes and modifiers (`private(set)` too)
        let mut k = 0;
        let mut modifiers = false;
        while k < toks.len() {
            let t = toks[k];
            if self.text(t) == "#[" {
                let mut depth = 0;
                while k < toks.len() {
                    match self.text(toks[k]) {
                        "#[" | "[" => depth += 1,
                        "]" => depth -= 1,
                        _ => {}
                    }
                    k += 1;
                    if depth == 0 {
                        break;
                    }
                }
                continue;
            }
            if self.toks[t].kind == Tok::Ident && MODIFIERS.contains(&self.lower(t).as_str()) {
                modifiers = true;
                k += 1;
                if k + 2 < toks.len() && self.text(toks[k]) == "(" && self.text(toks[k + 2]) == ")" {
                    k += 3;
                }
                continue;
            }
            break;
        }
        let in_class = matches!(item.body, Body::Class { .. });
        let top = matches!(item.body, Body::Root | Body::Namespace);
        let Some(&head) = toks.get(k) else { return Kind::Other };
        let next = toks.get(k + 1).copied();
        let kw = if self.toks[head].kind == Tok::Ident { self.lower(head) } else { String::new() };
        match kw.as_str() {
            "namespace" if top && next.is_none_or(|j| self.text(j) != "(") => {
                Kind::Namespace { braced: toks.iter().any(|&j| self.text(j) == "{") }
            }
            "use" if in_class => Kind::TraitUse,
            "use" if top => match next.map(|j| self.lower(j)).as_deref() {
                Some("function") => Kind::Import(1),
                Some("const") => Kind::Import(2),
                _ => Kind::Import(0),
            },
            "class" | "interface" | "trait" if top || !in_class => Kind::ClassLike,
            "enum" if next.is_some_and(|j| self.toks[j].kind == Tok::Ident) => Kind::ClassLike,
            "function" if in_class => Kind::Method,
            "function" => {
                let name = match next {
                    Some(j) if self.text(j) == "&" => toks.get(k + 2).copied(),
                    other => other,
                };
                if name.is_some_and(|j| self.toks[j].kind == Tok::Ident) {
                    Kind::Function
                } else {
                    Kind::Other
                }
            }
            "const" if in_class || top => Kind::Const,
            "case" if in_class => Kind::EnumCase,
            "case" | "default" if self.text(item.last) == ":" => Kind::Label,
            "return" => Kind::Return,
            "else" | "elseif" | "catch" | "finally" => Kind::Continuation,
            // `public $x`, `public ?int $x`, `var $x`
            _ if in_class && (modifiers || self.toks[head].kind == Tok::Variable) => Kind::Property,
            _ => Kind::Other,
        }
    }

    /// First token of statement `s` including the comments on their own lines right above it
    /// (blank lines go in front of those), but not before token `after`. Comments belong to
    /// class members and to classes and functions; in front of `namespace`, `use` and other
    /// statements at the top they do not (blank lines go between the comment and the statement).
    fn lead(&self, s: &Stmt, body: Body, after: Option<usize>) -> usize {
        let mut b = s.first;
        if matches!(body, Body::Root | Body::Namespace) && !matches!(s.kind, Kind::ClassLike | Kind::Function) {
            return b;
        }
        while b > 0
            && self.toks[b - 1].kind == Tok::Comment
            && self.first_on_line(b - 1)
            && after.is_none_or(|a| b - 1 > a)
        {
            b -= 1;
        }
        b
    }

    /// Statements of one container, attributes on lines of their own merged into what follows.
    fn statements(&self, items: &[Item]) -> Vec<Stmt> {
        let mut out: Vec<Stmt> = Vec::new();
        let mut pending_attr: Option<usize> = None;
        let mut prev_first_do = false;
        for item in items {
            let attr_only = self.text(item.first) == "#[" && self.text(item.last) == "]";
            if attr_only {
                pending_attr.get_or_insert(item.first);
                continue;
            }
            let mut kind = self.classify(item);
            // the `while` of `do { … } while (…);`
            if prev_first_do && self.lower(item.first) == "while" {
                kind = Kind::Continuation;
            }
            prev_first_do = self.lower(item.first) == "do";
            out.push(Stmt { first: pending_attr.take().unwrap_or(item.first), last: item.last, kind });
        }
        if let Some(first) = pending_attr {
            out.push(Stmt { first, last: items.last().map_or(first, |i| i.last), kind: Kind::Other });
        }
        out
    }

    /// Blank lines between two neighbouring statements of a container.
    fn blank_lines_between(&self, body: Body, a: Kind, b: Kind) -> usize {
        let o = self.o;
        if b == Kind::Continuation {
            return 0;
        }
        match body {
            Body::Root | Body::Namespace => {
                let mut n = 0;
                match (a, b) {
                    (_, Kind::Namespace { .. }) => n = n.max(o.BLANK_LINES_BEFORE_PACKAGE),
                    (Kind::Namespace { braced: false }, _) => n = n.max(o.BLANK_LINES_AFTER_PACKAGE),
                    _ => {}
                }
                match (a, b) {
                    // classes → functions → constants (not when going back)
                    (Kind::Import(x), Kind::Import(y)) if x < y => n = n.max(o.BLANK_LINES_BETWEEN_IMPORTS),
                    (Kind::Import(_), Kind::Import(_)) => {}
                    (Kind::Import(_), _) => n = n.max(o.BLANK_LINES_AFTER_IMPORTS),
                    (_, Kind::Import(_)) => n = n.max(o.BLANK_LINES_BEFORE_IMPORTS),
                    _ => {}
                }
                if a == Kind::ClassLike || b == Kind::ClassLike {
                    n = n.max(o.BLANK_LINES_AROUND_CLASS);
                }
                if a == Kind::Function {
                    n = n.max(o.BLANK_LINES_AFTER_FUNCTION);
                }
                if b == Kind::Return {
                    n = n.max(self.before_return());
                }
                n
            }
            Body::Class { interface, .. } => {
                let around = |k: Kind| match k {
                    Kind::Method if interface => o.BLANK_LINES_AROUND_METHOD_IN_INTERFACE,
                    Kind::Method => o.BLANK_LINES_AROUND_METHOD,
                    Kind::Property if interface => o.BLANK_LINES_AROUND_FIELD_IN_INTERFACE,
                    Kind::Property => o.BLANK_LINES_AROUND_FIELD,
                    Kind::Const => o.BLANK_LINES_AROUND_CONSTANTS,
                    Kind::EnumCase => o.BLANK_LINES_AROUND_ENUM_CASES,
                    _ => 0,
                };
                let mut n = around(a).max(around(b));
                match (a, b) {
                    (Kind::TraitUse, Kind::TraitUse) => {}
                    (Kind::TraitUse, _) => n = n.max(o.BLANK_LINES_AFTER_IMPORTS),
                    (_, Kind::TraitUse) => n = n.max(o.BLANK_LINES_BEFORE_IMPORTS),
                    _ => {}
                }
                n
            }
            Body::Function | Body::Closure | Body::Control | Body::Other => {
                if b == Kind::Return {
                    self.before_return()
                } else {
                    0
                }
            }
            Body::List => 0,
        }
    }

    fn before_return(&self) -> usize {
        self.o.BLANK_LINES_BEFORE_RETURN_STATEMENT.max(usize::from(self.o.BLANK_LINE_BEFORE_RETURN_STATEMENT))
    }

    /// Rules between the statements of one container.
    fn container(&mut self, body: Body, items: &[Item]) {
        let stmts = self.statements(items);
        for w in stmts.windows(2) {
            let (a, b) = (w[0], w[1]);
            let lead = self.lead(&b, body, Some(a.last));
            if lead <= a.last || a.last + 1 >= self.toks.len() {
                continue;
            }
            // `} else`: placed by the block rules
            if b.kind == Kind::Continuation {
                continue;
            }
            let n = self.blank_lines_between(body, a.kind, b.kind);
            self.blank_lines(lead, n);
            // one statement per line
            let same_line = self.start_line[lead] == self.end_line[a.last];
            let split = if a.kind == Kind::Label {
                self.o.CASE_STATEMENT_ON_NEW_LINE
            } else {
                !self.o.KEEP_MULTIPLE_EXPRESSIONS_IN_ONE_LINE
            };
            // `case 1: {` keeps its block on the label's line
            let bare_block = self.text(b.first) == "{";
            if same_line && split && lead == a.last + 1 && !(a.kind == Kind::Label && bare_block) {
                self.newline(lead);
            }
        }
        // the first statement of a method body, class body
        if let Some(first) = stmts.first() {
            let lead = self.lead(first, body, None);
            // at the top, after comments that are not its own (a file header): as after a statement
            if matches!(body, Body::Root | Body::Namespace) && lead > 0 && self.toks[lead - 1].kind == Tok::Comment {
                let n = self.blank_lines_between(body, Kind::Other, first.kind);
                self.blank_lines(lead, n);
            }
            let n = match body {
                Body::Function => self.o.BLANK_LINES_BEFORE_METHOD_BODY,
                Body::Class { anonymous: true, .. } => self.o.BLANK_LINES_AFTER_ANONYMOUS_CLASS_HEADER,
                Body::Class { .. } => self.o.BLANK_LINES_AFTER_CLASS_HEADER,
                _ => 0,
            };
            if lead > 0 && self.text(lead - 1) == "{" {
                self.blank_lines(lead, n);
            }
        }
        if let (Body::Class { .. }, Some(last)) = (body, stmts.last()) {
            if last.last + 1 < self.toks.len() && self.text(last.last + 1) == "}" {
                self.blank_lines(last.last + 1, self.o.BLANK_LINES_BEFORE_CLASS_END);
            }
        }
    }

    /// Is the `else` / `elseif` at `i` followed by `:` (after the condition)?
    fn alt_branch(&self, i: usize) -> bool {
        let next = |j: usize| (j + 1..self.toks.len()).find(|&k| !self.is_trivia(k));
        let Some(mut j) = next(i) else { return false };
        if self.lower(i) == "elseif" && self.text(j) == "(" {
            let mut depth = 0;
            loop {
                match self.text(j) {
                    "(" => depth += 1,
                    ")" => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
                match next(j) {
                    Some(k) => j = k,
                    None => return false,
                }
            }
            let Some(k) = next(j) else { return false };
            j = k;
        }
        self.text(j) == ":"
    }

    /// Brace placement and opening up of one block.
    fn block(&mut self, b: &indent::BlockInfo, header_first: Option<usize>) {
        let o = self.o;
        let (style, keep_simple, force_empty) = match b.body {
            Body::Class { anonymous: true, .. } => {
                (o.ANONYMOUS_BRACE_STYLE, o.KEEP_SIMPLE_CLASSES_IN_ONE_LINE, o.FORCE_EMPTY_CLASSES_IN_ONE_LINE)
            }
            Body::Class { .. } => {
                (o.CLASS_BRACE_STYLE, o.KEEP_SIMPLE_CLASSES_IN_ONE_LINE, o.FORCE_EMPTY_CLASSES_IN_ONE_LINE)
            }
            Body::Function => {
                (o.METHOD_BRACE_STYLE, o.KEEP_SIMPLE_METHODS_IN_ONE_LINE, o.FORCE_EMPTY_METHODS_IN_ONE_LINE)
            }
            Body::Closure => (o.LAMBDA_BRACE_STYLE, o.KEEP_SIMPLE_LAMBDAS_IN_ONE_LINE, false),
            Body::Namespace => (o.NAMESPACE_BRACE_STYLE, false, false),
            Body::Control => (o.BRACE_STYLE, o.KEEP_SIMPLE_BLOCKS_IN_ONE_LINE, false),
            Body::Root | Body::Other | Body::List => return,
        };
        let (open, close) = (b.open, b.close);
        // where `{` goes (not after a comment)
        if open > 0 && !self.is_trivia(open - 1) {
            let header_wrapped = header_first.is_some_and(|h| self.start_line[h] < self.end_line[open - 1]);
            // PSR-12: with the parameters wrapped, `) {` stays on one line
            let rparen_alone = {
                let mut j = open - 1;
                while j > 0 && self.text(j) != ")" && !self.first_on_line(j) && !matches!(self.text(j), "{" | ";" | "}")
                {
                    j -= 1;
                }
                self.text(j) == ")" && self.first_on_line(j)
            };
            let next_line = match style {
                END_OF_LINE => false,
                NEXT_LINE_IF_WRAPPED => header_wrapped,
                _ => !(o.KEEP_RPAREN_AND_LBRACE_ON_ONE_LINE && rparen_alone),
            };
            // an empty body forced onto one line goes on the declaration's line: `f() {}`
            let forced_empty = close == open + 1 && force_empty;
            if next_line && !forced_empty {
                self.need(open, |nd| {
                    nd.min = nd.min.max(1);
                    nd.max = nd.max.min(1);
                });
            } else {
                self.join(open);
            }
        }
        // opening up the block
        let one_line = self.start_line[open] == self.end_line[close];
        if one_line && keep_simple {
            return;
        }
        if close == open + 1 {
            if force_empty {
                self.join(close);
            } else {
                self.newline(close);
            }
        } else {
            let first_inner = open + 1;
            let trailing_comment =
                self.toks[first_inner].kind == Tok::Comment && self.start_line[first_inner] == self.end_line[open];
            if !trailing_comment && self.start_line[first_inner] == self.end_line[open] {
                self.newline(first_inner);
            }
            if self.end_line[close - 1] == self.start_line[close] {
                self.newline(close);
            }
        }
        // `} else`, `} catch`, `} finally`, `} while` (not an `else:` / `elseif (…):` of an
        // alternative-syntax `if` around the block)
        if close + 1 < self.toks.len() && self.toks[close + 1].kind == Tok::Ident && !self.alt_branch(close + 1) {
            let do_body = open > 0 && self.lower(open - 1) == "do";
            let on_new_line = match self.lower(close + 1).as_str() {
                "else" | "elseif" => Some(o.ELSE_ON_NEW_LINE),
                "catch" => Some(o.CATCH_ON_NEW_LINE),
                "finally" => Some(o.FINALLY_ON_NEW_LINE),
                "while" if do_body => Some(o.WHILE_ON_NEW_LINE),
                _ => None,
            };
            match on_new_line {
                Some(true) => self.newline(close + 1),
                Some(false) => self.join(close + 1),
                None => {}
            }
        }
    }
}

/// Blank lines that must survive the `KEEP_BLANK_LINES_*` limits: (offset of the token after
/// them, number of blank lines).
pub(crate) type KeepBlank = Vec<(usize, usize)>;

/// Line-break edits for `src` (whitespace only), and the blank lines to keep afterwards.
pub(crate) fn line_breaks(src: &str, toks: &[Token], o: &FormatOptions) -> Result<(Vec<Edit>, KeepBlank), FormatError> {
    let layout = indent::layout(src, toks, o, false, &[])?;
    if layout.template {
        return Ok((Vec::new(), Vec::new()));
    }
    // line of every offset
    let mut line_starts = vec![0usize];
    line_starts.extend(src.match_indices('\n').map(|(i, _)| i + 1));
    let line_of = |off: usize| line_starts.partition_point(|&s| s <= off) - 1;
    let start_line: Vec<usize> = toks.iter().map(|t| line_of(t.span.start as usize)).collect();
    let end_line: Vec<usize> =
        toks.iter().map(|t| line_of((t.span.end as usize).max(t.span.start as usize + 1) - 1)).collect();
    let mut cx = Ctx { src, toks, o, start_line, end_line, needs: FxHashMap::default() };

    // items by container, in order
    let mut order: Vec<usize> = Vec::new();
    let mut by_container: FxHashMap<usize, (Body, Vec<Item>)> = FxHashMap::default();
    for item in &layout.items {
        by_container
            .entry(item.container)
            .or_insert_with(|| {
                order.push(item.container);
                (item.body, Vec::new())
            })
            .1
            .push(*item);
    }
    // the statement each block belongs to: the latest-starting item around its `{` (items are
    // in order of their first token; the ones between end before the `{`)
    let header_of = |open: usize| -> Option<usize> {
        let at = layout.items.partition_point(|it| it.first < open);
        layout.items[..at].iter().rev().take(64).find(|it| it.last >= open).map(|it| it.first)
    };
    for c in &order {
        let (body, items) = &by_container[c];
        cx.container(*body, items);
    }
    for b in &layout.blocks {
        cx.block(b, header_of(b.open));
    }
    // after `<?php` at the start of the file
    let start =
        usize::from(toks.first().is_some_and(|t| t.kind == Tok::InlineHtml && t.span.text(src).starts_with("#!")));
    if toks.get(start).is_some_and(|t| t.kind == Tok::OpenTag)
        && start + 1 < toks.len()
        && toks[start + 1].kind != Tok::Eof
    {
        let g = start + 1;
        let same_line = cx.start_line[g] == cx.end_line[start];
        if o.NEW_LINE_AFTER_PHP_OPENING_TAG && same_line {
            cx.newline(g);
        }
        // like PhpStorm: not in front of a comment. (PhpStorm leaves out the blank line when the
        // code was on the `<?php` line and adds it the next time; it is added at once here.)
        if toks[g].kind != Tok::Comment {
            cx.blank_lines(g, o.BLANK_LINES_AFTER_OPENING_TAG);
        }
    }

    // turn the needs into edits
    let mut gaps: Vec<(usize, Need)> = cx.needs.into_iter().collect();
    gaps.sort_by_key(|(g, _)| *g);
    let mut edits = Vec::new();
    let mut keep = Vec::new();
    for (g, need) in gaps {
        if toks[g].kind == Tok::Eof {
            continue;
        }
        let (start, end) = (toks[g - 1].span.end as usize, toks[g].span.start as usize);
        let gap = &src[start..end];
        let have = gap.matches('\n').count();
        let want = have.min(need.max).max(need.min);
        if need.min >= 2 {
            keep.push((end, need.min - 1));
        }
        if want == have {
            continue;
        }
        // keep the indentation in front of the token (a comment in the first column stays there,
        // one moved to a new line must not land there)
        let text = if want == 0 {
            " ".to_string()
        } else if have > 0 {
            "\n".repeat(want) + &gap[gap.rfind('\n').map_or(0, |i| i + 1)..]
        } else if toks[g].kind == Tok::Comment {
            "\n".repeat(want) + " "
        } else {
            "\n".repeat(want)
        };
        edits.push(Edit { start, end, text });
    }
    Ok((edits, keep))
}
