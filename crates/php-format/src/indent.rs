//! Indentation: decides the leading whitespace of every line from the token stream.
//!
//! A stack of frames follows the nesting: blocks (`{ … }` of statements, functions, classes),
//! lists (`( … )`, `[ … ]`, `match { … }`) and alternative-syntax bodies (`if: … endif;`).
//! Inside a frame, a line either starts a new item (statement in a block, element in a list) and
//! gets the frame's inner indent, or continues the current item and gets the indent of the line
//! where that item began plus the continuation indent. A closing token goes back to the indent of
//! the line that opened the frame. Nothing but whitespace at line starts is decided here.

use crate::options::FormatOptions;
use crate::FormatError;
use php_syntax::lexer::{Tok, Token};

/// What happens to one line of the input.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Action {
    /// Left exactly as it is (inside a string / heredoc / HTML, or a template file).
    Verbatim,
    /// Starts with a token: indent it by this many columns.
    Indent(usize),
    /// A continuation line of a block comment whose first line moved by `delta` columns;
    /// `star` is the indent for lines starting with `*` (None: the line moves by `delta` too).
    CommentInner { star: Option<usize>, delta: isize },
    /// Only whitespace. `indent` is what the line would get (used while typing); `limit` is
    /// filled in on the next non-blank line.
    Blank { indent: usize },
}

#[derive(Clone, Debug)]
pub(crate) struct Line {
    pub start: usize,
    /// End of the content, before `\r\n` / `\n`.
    pub end: usize,
    /// The line break as written (empty on the last line).
    pub sep_len: usize,
    pub action: Action,
    /// The line break lies inside a token (multi-line string, heredoc, HTML): keep the line's end.
    pub end_in_token: bool,
    /// Most blank lines kept right before this line.
    pub blank_limit: usize,
    /// Spaces between tokens on this line may change (PHP code, not a template).
    pub code: bool,
    /// Inside a `@formatter:off` region: the line stays exactly as it is.
    pub off: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Block,
    List,
    Alt,
}

#[derive(Clone, Debug)]
struct Frame {
    kind: Kind,
    /// `}` `)` `]`, or the `end…` keyword of an alternative-syntax body.
    close: &'static str,
    /// Indent of the line holding the opening token (closing lines go back to it).
    open_indent: usize,
    /// Indent of the items inside.
    inner: usize,
    /// The next token starts a new item.
    fresh: bool,
    /// Indent of the line where the current item began.
    item_indent: usize,
    /// First token of the current item, lowercased (`if`, `public`, `$x`, …).
    item_first: String,
    /// The current item declares a class-like (its body holds declarations).
    item_class: bool,
    /// The current item has a `=>` or an assignment: a binary expression in it starts mid-line.
    item_assign: bool,
    /// A block that is a statement body (if / function / class …), not a closure or anonymous class.
    stmt_level: bool,
    /// Blank lines here count as "in declarations" (top level, class bodies).
    declarations: bool,
    switch: bool,
    case_seen: bool,
    /// Inside a `case …` label, before its `:`; `label_q` counts open ternary `?`.
    label: bool,
    label_q: u32,
    /// An `if:` body, which `else` / `elseif` also end.
    alt_if: bool,
    /// For `(` frames: the keyword in front (`if`, `match`, `switch`, `foreach` …).
    paren_kw: String,
    /// Opened by `#[`: an attribute in front of a declaration.
    attr: bool,
    /// Unique per frame, for grouping recorded items by container.
    id: usize,
    /// What kind of body a block / alternative-syntax frame is.
    body: Body,
    /// Token index where the current item began.
    item_start: usize,
    /// The current item in `Engine::items`.
    cur_item: Option<usize>,
}

/// What a block holds; decides which blank-line and brace rules apply to its items.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Body {
    Root,
    Namespace,
    Class {
        interface: bool,
        anonymous: bool,
    },
    Function,
    Closure,
    /// `if`, `while`, `try` … bodies, `switch` blocks, alternative syntax
    Control,
    /// A bare `{ … }` block or anything else
    Other,
    /// Not a statement container (parentheses, brackets, `match`, attributes)
    List,
}

/// One statement (or class member) as the indentation engine saw it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Item {
    /// The frame holding it (items with the same container follow each other).
    pub container: usize,
    pub body: Body,
    /// First and last token (indices into the token list).
    pub first: usize,
    pub last: usize,
}

/// A block of statements: its `{` and `}` token indices and kind.
#[derive(Clone, Copy, Debug)]
pub(crate) struct BlockInfo {
    pub body: Body,
    pub open: usize,
    pub close: usize,
}

impl Frame {
    fn new(kind: Kind, close: &'static str, open_indent: usize, inner: usize) -> Frame {
        Frame {
            kind,
            close,
            open_indent,
            inner,
            fresh: true,
            item_indent: inner,
            item_first: String::new(),
            item_class: false,
            item_assign: false,
            stmt_level: false,
            declarations: false,
            switch: false,
            case_seen: false,
            label: false,
            label_q: 0,
            alt_if: false,
            paren_kw: String::new(),
            attr: false,
            id: 0,
            body: Body::List,
            item_start: 0,
            cur_item: None,
        }
    }
}

/// Items starting with these are statements whose `{` body ends the statement.
const STMT_KEYWORDS: &[&str] = &[
    "if",
    "elseif",
    "else",
    "while",
    "for",
    "foreach",
    "switch",
    "try",
    "catch",
    "finally",
    "do",
    "declare",
    "function",
    "class",
    "interface",
    "trait",
    "enum",
    "abstract",
    "final",
    "readonly",
    "namespace",
    "public",
    "protected",
    "private",
    "static",
    "var",
    "#[",
    "{",
    "use",
];

fn is_binary_op(t: &str) -> bool {
    matches!(
        t,
        "&&" | "||"
            | "|"
            | "&"
            | "^"
            | "."
            | "+"
            | "-"
            | "*"
            | "/"
            | "%"
            | "**"
            | "??"
            | "=="
            | "!="
            | "==="
            | "!=="
            | "<"
            | ">"
            | "<="
            | ">="
            | "<=>"
            | "<<"
            | ">>"
    )
}

fn is_assign(t: &str) -> bool {
    matches!(t, "=" | "+=" | "-=" | "*=" | "/=" | ".=" | "%=" | "**=" | "??=" | "&=" | "|=" | "^=" | "<<=" | ">>=")
}

/// Control keywords whose `(…)` may be followed by `:` (alternative syntax), with the closing keyword.
fn alt_close(kw: &str) -> Option<&'static str> {
    Some(match kw {
        "if" | "elseif" => "endif",
        "while" => "endwhile",
        "for" => "endfor",
        "foreach" => "endforeach",
        "switch" => "endswitch",
        "declare" => "enddeclare",
        _ => return None,
    })
}

struct Engine<'a> {
    src: &'a str,
    toks: &'a [Token],
    o: &'a FormatOptions,
    stack: Vec<Frame>,
    /// Index of the previous significant token.
    prev: Option<usize>,
    /// The one before it.
    prev2: Option<usize>,
    /// Keyword in front of the `(` that the previous `)` closed.
    last_closed_kw: String,
    cur_line_indent: usize,
    /// Byte offset of the first token on the current line.
    cur_line_first: usize,
    /// Last measured (byte offset, column) on the current line, so columns further right are
    /// measured from there (lines can be very long).
    col_cache: std::cell::Cell<(usize, usize)>,
    /// Statements and members seen so far.
    items: Vec<Item>,
    /// Blocks seen so far (filled in when they close).
    blocks: Vec<BlockInfo>,
    /// `{` token index of each open block frame, by frame id.
    open_at: Vec<usize>,
    next_id: usize,
}

impl Engine<'_> {
    fn text(&self, i: usize) -> &str {
        self.toks[i].span.text(self.src)
    }

    fn lower(&self, i: usize) -> String {
        match self.toks[i].kind {
            Tok::Ident | Tok::Name => self.text(i).to_ascii_lowercase(),
            _ => self.text(i).to_string(),
        }
    }

    fn top(&self) -> &Frame {
        self.stack.last().expect("root frame")
    }

    fn top_mut(&mut self) -> &mut Frame {
        self.stack.last_mut().expect("root frame")
    }

    fn prev_text(&self) -> &str {
        self.prev.map(|p| self.text(p)).unwrap_or("")
    }

    /// Does token `i` close the innermost frame?
    fn closes_top(&self, i: usize) -> bool {
        let f = self.top();
        match self.toks[i].kind {
            Tok::Punct(p @ (")" | "]" | "}")) => f.close == p,
            Tok::Ident if f.kind == Kind::Alt => {
                let l = self.lower(i);
                l == f.close || (f.alt_if && f.fresh && (l == "else" || l == "elseif") && self.alt_branch(i))
            }
            _ => false,
        }
    }

    /// Is the `else` / `elseif` at `i` an alternative-syntax branch (`else:`, `elseif (…):`)
    /// rather than part of a braced `if` inside the body?
    fn alt_branch(&self, i: usize) -> bool {
        let next =
            |from: usize| (from..self.toks.len()).find(|&j| !matches!(self.toks[j].kind, Tok::Comment | Tok::OpenTag));
        let Some(mut j) = next(i + 1) else { return false };
        if self.lower(i) == "elseif" {
            if self.text(j) != "(" {
                return false;
            }
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
                j += 1;
                if j >= self.toks.len() || self.toks[j].kind == Tok::Eof {
                    return false;
                }
            }
            let Some(k) = next(j + 1) else { return false };
            j = k;
        }
        self.text(j) == ":"
    }

    /// Would the `{` at `i` open a list (`match {`, `->{…}`, group `use`) rather than a block?
    fn brace_is_list(&self) -> bool {
        let prev = self.prev_text();
        matches!(prev, "->" | "?->" | "::" | "$")
            || prev.ends_with('\\')
            || (prev == ")" && self.last_closed_kw == "match")
    }

    /// Would a block opened now be a statement body (`if`, `function`, `class` …)?
    fn block_is_statement(&self) -> bool {
        let parent = self.top();
        parent.kind != Kind::List && STMT_KEYWORDS.contains(&parent.item_first.as_str())
    }

    /// Indent of a line that starts a new item with token text `first`.
    fn start_indent(&self, first: &str) -> usize {
        let f = self.top();
        if !f.switch {
            return f.inner;
        }
        let case_level = if self.o.INDENT_CASE_FROM_SWITCH { f.inner } else { f.open_indent };
        if first == "case" || first == "default" {
            case_level
        } else if f.case_seen {
            case_level + self.o.INDENT_SIZE
        } else {
            f.inner
        }
    }

    fn continuation(&self) -> usize {
        self.top().item_indent + self.o.CONTINUATION_INDENT_SIZE
    }

    /// Continuation for a line starting with token `i`. Inside parentheses and brackets, the
    /// operands of a wrapped binary expression that starts the element line up with it
    /// (`foo(\n $a\n | $b\n)`), like PhpStorm does; after `=>` / `=`, and for `->` chains, the
    /// line is indented.
    fn continuation_for(&self, i: usize) -> usize {
        let f = self.top();
        let binary = |j: usize| {
            self.toks[j].kind != Tok::Ident && is_binary_op(&self.lower(j))
                || (self.toks[j].kind == Tok::Ident
                    && matches!(self.lower(j).as_str(), "and" | "or" | "xor" | "instanceof"))
        };
        if f.kind == Kind::List && !f.item_assign && (binary(i) || self.prev.is_some_and(binary)) {
            f.inner
        } else {
            self.continuation()
        }
    }

    /// Indent for a line whose first token is significant token `i`.
    fn token_indent(&self, i: usize) -> usize {
        if self.closes_top(i) {
            self.top().open_indent
        } else if self.text(i) == "{" && !self.top().fresh && !self.brace_is_list() && self.block_is_statement() {
            // `{` of a class / function / control statement on its own line
            self.top().item_indent
        } else if self.top().fresh {
            self.start_indent(&self.lower(i))
        } else {
            self.continuation_for(i)
        }
    }

    /// Indent for a comment line or a blank line in front of significant token `next`.
    fn gap_indent(&self, next: Option<usize>) -> usize {
        if !self.top().fresh {
            return self.continuation();
        }
        match next {
            Some(j) if !self.closes_top(j) => self.start_indent(&self.lower(j)),
            _ => self.start_indent(""),
        }
    }

    /// Blank-line limit in front of significant token `next`.
    fn blank_limit(&self, next: Option<usize>) -> usize {
        let o = self.o;
        match next {
            // in class bodies the "declarations" limit applies next to braces too
            _ if self.top().declarations => o.KEEP_BLANK_LINES_IN_DECLARATIONS,
            Some(j) if self.text(j) == "}" && self.closes_top(j) => o.KEEP_BLANK_LINES_BEFORE_RBRACE,
            _ if self.prev.is_some_and(|p| self.text(p) == "{") => o.KEEP_BLANK_LINES_AFTER_LBRACE,
            _ => o.KEEP_BLANK_LINES_IN_CODE,
        }
    }

    fn advance(&mut self, i: usize) {
        self.prev2 = self.prev;
        self.prev = Some(i);
        // the current item of every statement container around `i` reaches at least here
        for f in &self.stack {
            if let Some(c) = f.cur_item {
                self.items[c].last = i;
            }
        }
    }

    fn push(&mut self, kind: Kind, close: &'static str) -> &mut Frame {
        let open = self.cur_line_indent;
        let mut f = Frame::new(kind, close, open, open + self.o.INDENT_SIZE);
        self.next_id += 1;
        f.id = self.next_id;
        self.stack.push(f);
        self.top_mut()
    }

    /// With `ALIGN_MULTILINE_*`: the column of the first item when it stays on the line of the
    /// bracket at `i` (later items line up with it).
    fn align_column(&self, i: usize, prev_kw: &str) -> Option<usize> {
        let o = self.o;
        let toks = self.toks;
        let next = (i + 1..toks.len()).find(|&j| toks[j].kind != Tok::Comment)?;
        if self.src[toks[i].span.end as usize..toks[next].span.start as usize].contains('\n')
            || matches!(toks[next].kind, Tok::Punct(")" | "]") | Tok::Eof)
        {
            return None;
        }
        let prev = self.prev;
        let wanted = if self.text(i) == "[" {
            // an array literal, not an index
            let index = prev.is_some_and(|p| {
                matches!(toks[p].kind, Tok::Variable | Tok::Punct(")" | "]" | "}"))
                    || toks[p].kind == Tok::Ident
                        && !["return", "echo", "yield", "print", "as"].contains(&self.lower(p).as_str())
            });
            !index && o.ALIGN_MULTILINE_ARRAY_INITIALIZER_EXPRESSION
        } else if prev_kw == "array" {
            o.ALIGN_MULTILINE_ARRAY_INITIALIZER_EXPRESSION
        } else if prev_kw == "function" || prev_kw == "fn" || self.prev2.is_some_and(|q| self.lower(q) == "function") {
            o.ALIGN_MULTILINE_PARAMETERS
        } else if matches!(
            prev_kw,
            "if" | "elseif" | "while" | "for" | "foreach" | "switch" | "catch" | "match" | "declare"
        ) {
            false
        } else {
            prev.is_some_and(|p| matches!(toks[p].kind, Tok::Ident | Tok::Name | Tok::Variable | Tok::Punct(")" | "]")))
                && o.ALIGN_MULTILINE_PARAMETERS_IN_CALLS
        };
        if !wanted {
            return None;
        }
        // the column of `next` on the line, continuing from the last measured point
        let target = toks[next].span.start as usize;
        let (mut at, mut col) = self.col_cache.get();
        if at < self.cur_line_first || at > target {
            (at, col) = (self.cur_line_first, 0);
        }
        let tab = o.TAB_SIZE.max(1);
        col = self.src[at..target].chars().fold(col, |w, c| if c == '\t' { (w / tab + 1) * tab } else { w + 1 });
        self.col_cache.set((target, col));
        Some(self.cur_line_indent + col)
    }

    /// Extra indent for a list opened at `i` whose items start on the next line, when it is an
    /// operand of a binary expression (`&&`, `==`, `.`, … but not `??`) that starts in the middle
    /// of a line (`return $a && f(` / `if (f(…) && $b`): PhpStorm indents such operands by the
    /// continuation indent.
    fn binary_extra(&self, i: usize) -> usize {
        let toks = self.toks;
        let next = (i + 1..toks.len()).find(|&j| toks[j].kind != Tok::Comment);
        let starts_next_line =
            next.is_some_and(|j| self.src[toks[i].span.end as usize..toks[j].span.start as usize].contains('\n'));
        if !starts_next_line {
            return 0;
        }
        let is_boundary = |j: usize| {
            let t = self.text(j);
            matches!(t, "," | ";" | "{" | "}" | "=>" | "?" | ":" | "??" | "(" | "[")
                || (matches!(toks[j].kind, Tok::Punct(_)) && is_assign(t))
                || (toks[j].kind == Tok::Ident
                    && matches!(
                        self.lower(j).as_str(),
                        "return" | "echo" | "print" | "throw" | "yield" | "case" | "if" | "elseif" | "while"
                    ))
        };
        let binary = |j: usize| {
            let t = self.text(j);
            (toks[j].kind != Tok::Ident && is_binary_op(t) && t != "??")
                || (toks[j].kind == Tok::Ident && matches!(self.lower(j).as_str(), "and" | "or" | "xor" | "instanceof"))
        };
        // backwards to the start of the expression
        let floor = self.top().item_start;
        let mut j = i;
        let mut depth = 0i32;
        let mut found = false;
        let mut start = floor;
        while j > floor {
            j -= 1;
            if matches!(toks[j].kind, Tok::Comment | Tok::OpenTag) {
                continue;
            }
            match self.text(j) {
                ")" | "]" => depth += 1,
                "(" | "[" if depth > 0 => depth -= 1,
                _ if depth > 0 => {}
                _ if is_boundary(j) => {
                    start = j + 1;
                    break;
                }
                _ if binary(j) => found = true,
                _ => {}
            }
        }
        // the operator right at the start is unary (`-f(`)
        let start = (start..i).find(|&k| !matches!(toks[k].kind, Tok::Comment | Tok::OpenTag)).unwrap_or(i);
        if found && (start..i).filter(|&k| !matches!(toks[k].kind, Tok::Comment | Tok::OpenTag)).all(&binary) {
            found = false;
        }
        if !found {
            // forwards from the matching closer to the end of the expression
            let mut depth = 0i32;
            let mut k = i;
            while k < toks.len() {
                match self.text(k) {
                    "(" | "[" | "{" | "#[" => depth += 1,
                    ")" | "]" | "}" => {
                        depth -= 1;
                        if depth < 0 {
                            break;
                        }
                    }
                    _ if depth > 0 => {}
                    _ if toks[k].kind == Tok::Comment => {}
                    _ if is_boundary(k) && k > i => break,
                    _ if binary(k) => {
                        found = true;
                        break;
                    }
                    _ => {}
                }
                k += 1;
            }
        }
        if !found {
            return 0;
        }
        // an expression that starts its line gets no extra indent
        let line_start = self.src[..toks[start].span.start as usize].rfind('\n').map_or(0, |p| p + 1);
        if self.src[line_start..toks[start].span.start as usize].trim().is_empty() {
            0
        } else {
            self.o.CONTINUATION_INDENT_SIZE
        }
    }

    /// The kind of a block opened at token `i` by the current item of the top frame.
    fn body_kind(&self, i: usize, stmt_level: bool) -> Body {
        let parent = self.top();
        // a bare `{ … }` statement
        if parent.item_start == i {
            return Body::Other;
        }
        let mut prev: Option<usize> = None;
        let mut kind = None;
        for j in parent.item_start..i {
            let t = self.toks[j];
            if matches!(t.kind, Tok::Comment | Tok::OpenTag) {
                continue;
            }
            let after_member = prev.is_some_and(|p| matches!(self.text(p), "->" | "?->" | "::"));
            if t.kind == Tok::Ident && !after_member {
                match self.lower(j).as_str() {
                    w @ ("class" | "interface" | "trait" | "enum") => {
                        let anonymous = w == "class" && prev.is_some_and(|p| self.lower(p) == "new");
                        kind = Some(Body::Class { interface: w == "interface", anonymous });
                    }
                    "function" | "fn" if kind.is_none() => {
                        kind = Some(if stmt_level { Body::Function } else { Body::Closure });
                    }
                    "namespace" if kind.is_none() && j == parent.item_start => kind = Some(Body::Namespace),
                    _ => {}
                }
            }
            prev = Some(j);
        }
        kind.unwrap_or(if stmt_level { Body::Control } else { Body::Other })
    }

    /// Update the frames for significant token `i`.
    fn process(&mut self, i: usize) -> Result<(), FormatError> {
        let kind = self.toks[i].kind;
        let lower = self.lower(i);
        // closing tokens
        if let Tok::Punct(p @ (")" | "]" | "}")) = kind {
            if self.top().close != p {
                return Err(FormatError::Unbalanced(self.toks[i].span.start as usize));
            }
            let f = self.stack.pop().expect("checked above");
            if f.kind == Kind::Block {
                if let Some(&open) = self.open_at.get(f.id) {
                    self.blocks.push(BlockInfo { body: f.body, open, close: i });
                }
            }
            let parent = self.top_mut();
            // a statement body ends the statement; an attribute alone does not start the item
            if (f.kind == Kind::Block && f.stmt_level && parent.kind != Kind::List)
                || (f.attr && parent.item_first == "#[")
            {
                parent.fresh = true;
            }
            self.last_closed_kw = if p == ")" { f.paren_kw } else { String::new() };
            self.advance(i);
            return Ok(());
        }
        if kind == Tok::Ident && self.top().kind == Kind::Alt && self.closes_top(i) {
            self.stack.pop();
            if lower != "else" && lower != "elseif" {
                self.advance(i);
                return Ok(());
            }
        }
        // HTML between `?>` and `<?php` separates statements like `;`
        if kind == Tok::InlineHtml {
            let f = self.top_mut();
            if f.kind != Kind::List {
                f.fresh = true;
            }
            self.advance(i);
            return Ok(());
        }
        // start of an item
        let line_indent = self.cur_line_indent;
        let n_items = self.items.len();
        let f = self.top_mut();
        if f.fresh {
            f.fresh = false;
            f.item_start = i;
            if f.kind != Kind::List {
                f.cur_item = Some(n_items);
                let (container, body) = (f.id, f.body);
                self.items.push(Item { container, body, first: i, last: i });
            }
            let f = self.top_mut();
            f.item_indent = line_indent;
            f.item_first = lower.clone();
            f.item_class = false;
            f.item_assign = false;
            if f.switch && (lower == "case" || lower == "default") {
                f.label = true;
                f.label_q = 0;
            }
        }
        if kind == Tok::Ident
            && matches!(lower.as_str(), "class" | "interface" | "trait" | "enum")
            && !matches!(self.prev_text(), "::" | "->" | "?->")
        {
            self.top_mut().item_class = true;
        }
        // the keyword in front, unless it is a name (`function for(`, `$x->if(`, `A::match(`)
        let prev_kw = match self.prev {
            Some(p)
                if self.toks[p].kind == Tok::Ident
                    && !self.prev2.is_some_and(|q| {
                        matches!(self.text(q), "->" | "?->" | "::") || self.lower(q) == "function"
                    }) =>
            {
                self.lower(p)
            }
            _ => String::new(),
        };
        match kind {
            Tok::Punct("(") => {
                let extra = self.binary_extra(i);
                let align = self.align_column(i, &prev_kw);
                let f = self.push(Kind::List, ")");
                f.paren_kw = prev_kw;
                f.open_indent += extra;
                f.inner += extra;
                if let Some(col) = align {
                    f.inner = col;
                }
            }
            Tok::Punct("[") => {
                let extra = self.binary_extra(i);
                let align = self.align_column(i, "");
                let f = self.push(Kind::List, "]");
                f.open_indent += extra;
                f.inner += extra;
                if let Some(col) = align {
                    f.inner = col;
                }
            }
            Tok::Punct("#[") => self.push(Kind::List, "]").attr = true,
            Tok::Punct("{") => {
                if self.brace_is_list() {
                    self.push(Kind::List, "}");
                } else {
                    let stmt_level = self.block_is_statement();
                    let parent = self.top();
                    let declarations = parent.item_class || parent.item_first == "namespace";
                    let switch = self.prev_text() == ")" && self.last_closed_kw == "switch";
                    // a statement body is indented from the line where the statement began (a
                    // wrapped `if (… &&\n …) {` too); a closure from the line it sits on
                    let open = if stmt_level { parent.item_indent } else { self.cur_line_indent };
                    let inner = open + self.o.INDENT_SIZE;
                    let body = self.body_kind(i, stmt_level);
                    let f = self.push(Kind::Block, "}");
                    f.open_indent = open;
                    f.inner = inner;
                    f.stmt_level = stmt_level;
                    f.declarations = declarations;
                    f.switch = switch;
                    f.body = body;
                    let id = f.id;
                    if self.open_at.len() <= id {
                        self.open_at.resize(id + 1, usize::MAX);
                    }
                    self.open_at[id] = i;
                }
            }
            Tok::Punct(";") | Tok::CloseTag => {
                let f = self.top_mut();
                if f.kind != Kind::List {
                    f.fresh = true;
                    f.label = false;
                }
            }
            Tok::Punct(",") => {
                let f = self.top_mut();
                if f.kind == Kind::List {
                    f.fresh = true;
                }
            }
            Tok::Punct("?") if self.top().label => self.top_mut().label_q += 1,
            Tok::Punct(
                "=>" | "=" | "+=" | "-=" | "*=" | "/=" | ".=" | "%=" | "**=" | "??=" | "&=" | "|=" | "^=" | "<<="
                | ">>=",
            ) => self.top_mut().item_assign = true,
            Tok::Punct(":") => {
                let f = self.top_mut();
                if f.label {
                    if f.label_q == 0 {
                        f.label = false;
                        f.fresh = true;
                        f.case_seen = true;
                    } else {
                        f.label_q -= 1;
                    }
                } else {
                    let kw = if self.prev_text() == ")" {
                        self.last_closed_kw.clone()
                    } else if prev_kw == "else" {
                        "else".to_string()
                    } else {
                        String::new()
                    };
                    let close = if kw == "else" { Some("endif") } else { alt_close(&kw) };
                    if let Some(close) = close {
                        let f = self.push(Kind::Alt, close);
                        f.body = Body::Control;
                        f.alt_if = close == "endif";
                        f.switch = close == "endswitch";
                    }
                }
            }
            _ => {}
        }
        self.advance(i);
        Ok(())
    }
}

/// Split `src` into lines.
fn lines_of(src: &str) -> Vec<Line> {
    let mut out = Vec::new();
    let mut start = 0;
    let b = src.as_bytes();
    loop {
        let nl = src[start..].find('\n').map(|i| start + i);
        let (end, sep_len, next) = match nl {
            Some(n) if n > start && b[n - 1] == b'\r' => (n - 1, 2, n + 1),
            Some(n) => (n, 1, n + 1),
            None => (src.len(), 0, src.len()),
        };
        out.push(Line {
            start,
            end,
            sep_len,
            action: Action::Verbatim,
            end_in_token: false,
            blank_limit: usize::MAX,
            code: false,
            off: false,
        });
        // the empty rest after a final line break is not a line
        if nl.is_none() || next == src.len() {
            break;
        }
        start = next;
    }
    out
}

/// Result of [`layout`].
pub(crate) struct Layout {
    pub lines: Vec<Line>,
    /// Statements / members in token order (a container's items follow each other).
    pub items: Vec<Item>,
    /// Statement blocks, in the order they close.
    pub blocks: Vec<BlockInfo>,
    /// The file has HTML outside PHP tags: its PHP is left as it is.
    pub template: bool,
}

/// Decide what happens to every line. `toks` come from `lex_with_trivia`. `lenient`: blocks still
/// open at the end of the file are fine (a file being typed); for formatting they are an error.
/// `min_blank`: (byte offset of a token, blank lines) that must be kept in front of that token
/// even beyond the `KEEP_BLANK_LINES_*` limits, sorted by offset.
pub(crate) fn layout(
    src: &str,
    toks: &[Token],
    o: &FormatOptions,
    lenient: bool,
    min_blank: &[(usize, usize)],
) -> Result<Layout, FormatError> {
    let mut lines = lines_of(src);
    let starts: Vec<usize> = lines.iter().map(|l| l.start).collect();
    let line_of = |off: usize| starts.partition_point(|&s| s <= off) - 1;
    // a file with real HTML in it is a template: its PHP is not re-indented (yet)
    // (a `#!/usr/bin/env php` line at the very start is not HTML)
    let template = toks.iter().any(|t| {
        let text = t.span.text(src);
        let shebang = t.span.start == 0 && text.starts_with("#!") && text.trim_end().lines().count() == 1;
        t.kind == Tok::InlineHtml && !text.trim().is_empty() && !shebang
    });

    // which lines begin / end inside a token
    let mut inside = vec![false; lines.len()];
    for t in toks {
        let (s, e) = (t.span.start as usize, t.span.end as usize);
        if e <= s {
            continue;
        }
        let (first, last) = (line_of(s), line_of(e.saturating_sub(1)));
        for ln in first..=last {
            if ln > first {
                inside[ln] = true;
            }
            // a token that covers the line's end owns its trailing whitespace (comments do not:
            // trailing spaces in them may go)
            if t.kind != Tok::Comment && (ln < last || (lines[ln].end <= e && lines[ln].end >= s)) {
                lines[ln].end_in_token = true;
            }
        }
        if t.kind == Tok::InlineHtml {
            // the line where HTML starts is HTML too
            for (ln, l) in lines.iter_mut().enumerate().take(last + 1).skip(first) {
                if src[l.start..s.max(l.start)].trim().is_empty() {
                    inside[ln] = true;
                }
            }
        }
    }

    let mut eng = Engine {
        src,
        toks,
        o,
        stack: vec![Frame { declarations: true, body: Body::Root, ..Frame::new(Kind::Block, "", 0, 0) }],
        prev: None,
        prev2: None,
        last_closed_kw: String::new(),
        cur_line_indent: 0,
        cur_line_first: 0,
        col_cache: std::cell::Cell::new((0, 0)),
        items: Vec::new(),
        blocks: Vec::new(),
        open_at: Vec::new(),
        next_id: 0,
    };
    // the next significant token after each token, for comment and blank lines
    let mut next_sig = vec![None; toks.len() + 1];
    for i in (0..toks.len()).rev() {
        next_sig[i] =
            if !matches!(toks[i].kind, Tok::Comment | Tok::OpenTag | Tok::Eof) { Some(i) } else { next_sig[i + 1] };
    }

    let mut last_line_seen = 0usize;
    // line where the previous token ended: a token on that line is not the first one there
    let mut prev_end_line: Option<usize> = None;
    // templates: the PHP block being read, as (indent of the `<?php` line, the engine's indent
    // of the block's first code token); code lines inside get the line's indent plus their depth
    // relative to the block's start
    let mut block: Option<(usize, Option<usize>)> = None;
    let code_in_tags = if o.INDENT_CODE_IN_PHP_TAGS { o.INDENT_SIZE } else { 0 };
    for (i, t) in toks.iter().enumerate() {
        if t.kind == Tok::Eof {
            break;
        }
        let s = t.span.start as usize;
        let ln = line_of(s);
        if template {
            match t.kind {
                Tok::OpenTag => {
                    // the indent the `<?php` line ends up with (a code line already re-indented)
                    let html = match lines[ln].action {
                        Action::Indent(n) => n,
                        _ => {
                            let ws = &src[lines[ln].start..s];
                            o.width(&ws[..ws.len() - ws.trim_start().len()])
                        }
                    };
                    block = Some((html, None));
                }
                Tok::CloseTag | Tok::InlineHtml | Tok::OpenTagEcho => block = None,
                Tok::Comment => {}
                _ => {
                    if let Some((html, None)) = block {
                        block = Some((html, Some(eng.token_indent(i))));
                    }
                }
            }
        }
        let first_on_line = prev_end_line != Some(ln) && src[lines[ln].start..s].trim().is_empty();
        prev_end_line = Some(line_of((t.span.end as usize).max(s + 1) - 1));
        if first_on_line {
            let next = if t.kind == Tok::Comment || t.kind == Tok::OpenTag { next_sig[i] } else { Some(i) };
            // blank lines in front of this one
            let limit = eng.blank_limit(next);
            let gap = eng.gap_indent(next);
            for l in lines.iter_mut().take(ln).skip(last_line_seen + 1) {
                if src[l.start..l.end].trim().is_empty() && !l.end_in_token {
                    l.action = Action::Blank { indent: gap };
                }
            }
            let min = min_blank.binary_search_by_key(&s, |&(at, _)| at).map(|k| min_blank[k].1).unwrap_or(0);
            lines[ln].blank_limit = limit.max(min);
            last_line_seen = ln;
            eng.cur_line_first = s;
            eng.col_cache.set((s, 0));
            let template_code = template
                && !inside[ln]
                && !matches!(t.kind, Tok::OpenTag | Tok::OpenTagEcho | Tok::CloseTag | Tok::InlineHtml)
                && matches!(block, Some((_, Some(_))));
            if template_code {
                // in engine terms for the engine, in the template's terms for the line
                let (html, base) = block.map(|(h, b)| (h, b.unwrap_or(0))).unwrap_or((0, 0));
                let engine = if t.kind == Tok::Comment { eng.gap_indent(next) } else { eng.token_indent(i) };
                lines[ln].action = Action::Indent(html + code_in_tags + engine.saturating_sub(base));
                eng.cur_line_indent = engine;
            } else if inside[ln] || template || t.kind == Tok::OpenTag {
                eng.cur_line_indent = o.width(&src[lines[ln].start..s]);
            } else {
                let old = o.width(&src[lines[ln].start..s]);
                let keep_first_column = t.kind == Tok::Comment
                    && old == 0
                    && o.KEEP_FIRST_COLUMN_COMMENT
                    && !t.span.text(src).starts_with("/**");
                let indent = if keep_first_column {
                    0
                } else if t.kind == Tok::Comment {
                    eng.gap_indent(next)
                } else {
                    eng.token_indent(i)
                };
                lines[ln].action = Action::Indent(indent);
                eng.cur_line_indent = indent;
                if t.kind == Tok::Comment {
                    let e = t.span.end as usize;
                    let delta = indent as isize - old as isize;
                    let last = line_of(e.saturating_sub(1));
                    // `/** … */` and comments whose lines start with `*` keep the stars lined up;
                    // other block comments move as a whole
                    let text = t.span.text(src);
                    let starred = text.starts_with("/**")
                        || text.lines().skip(1).all(|l| l.trim().is_empty() || l.trim_start().starts_with('*'));
                    for l in lines.iter_mut().take(last + 1).skip(ln + 1) {
                        l.action = Action::CommentInner { star: if starred { Some(indent + 1) } else { None }, delta };
                    }
                }
            }
        }
        if matches!(t.kind, Tok::Comment | Tok::OpenTag) {
            continue;
        }
        eng.process(i)?;
    }
    if !lenient && eng.stack.len() > 1 && eng.stack.iter().any(|f| f.kind != Kind::Alt) {
        return Err(FormatError::Unbalanced(src.len()));
    }
    // trailing blank lines
    let end_gap = eng.gap_indent(None);
    for l in lines.iter_mut().skip(last_line_seen + 1) {
        if src[l.start..l.end].trim().is_empty() && !l.end_in_token {
            l.action = Action::Blank { indent: end_gap };
        }
    }
    for (ln, l) in lines.iter_mut().enumerate() {
        // (spaces between PHP tokens may change on any line; HTML and tags are never touched)
        l.code = !matches!(l.action, Action::Blank { .. });
        let template_code = template && matches!(l.action, Action::Indent(_)) && !inside[ln];
        if (template && !template_code || inside[ln]) && !matches!(l.action, Action::CommentInner { .. }) {
            if matches!(l.action, Action::Blank { .. }) && !inside[ln] {
                continue;
            }
            l.action = Action::Verbatim;
        }
    }
    if o.FORMATTER_TAGS_ENABLED {
        formatter_off(src, toks, o, &mut lines, line_of);
    }
    Ok(Layout { lines, items: eng.items, blocks: eng.blocks, template })
}

/// Lines from a comment holding `FORMATTER_OFF_TAG` through the next comment holding
/// `FORMATTER_ON_TAG` (or the end of the file) are left alone.
fn formatter_off(src: &str, toks: &[Token], o: &FormatOptions, lines: &mut [Line], line_of: impl Fn(usize) -> usize) {
    if o.FORMATTER_OFF_TAG.is_empty() || !src.contains(&o.FORMATTER_OFF_TAG) {
        return;
    }
    let mut off_from: Option<usize> = None;
    for t in toks.iter().filter(|t| t.kind == Tok::Comment) {
        let text = t.span.text(src);
        let (first, last) = (line_of(t.span.start as usize), line_of((t.span.end as usize).saturating_sub(1)));
        match off_from {
            None if text.contains(&o.FORMATTER_OFF_TAG) => off_from = Some(first),
            Some(from) if !o.FORMATTER_ON_TAG.is_empty() && text.contains(&o.FORMATTER_ON_TAG) => {
                lines[from..=last].iter_mut().for_each(|l| l.off = true);
                off_from = None;
            }
            _ => {}
        }
    }
    if let Some(from) = off_from {
        lines[from..].iter_mut().for_each(|l| l.off = true);
    }
}
