use crate::span::Span;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tok {
    InlineHtml,
    Variable,
    Ident,
    /// Qualified / fully-qualified name (contains a backslash).
    Name,
    Int,
    Float,
    /// String without interpolation.
    Str,
    /// String with interpolation (double quoted, heredoc, backtick).
    Template,
    Cast(&'static str),
    Punct(&'static str),
    /// `<?=`
    OpenTagEcho,
    /// `?>`
    CloseTag,
    /// `<?php` (only from [`lex_with_trivia`]).
    OpenTag,
    /// `// …`, `# …`, `/* … */`, `/** … */` (only from [`lex_with_trivia`]).
    Comment,
    Unknown,
    Eof,
}

#[derive(Clone, Copy, Debug)]
pub struct Token {
    pub kind: Tok,
    pub span: Span,
    /// Doc comment directly preceding this token.
    pub doc: Option<Span>,
}

const PUNCTS: &[&str] = &[
    "**=", "...", "<=>", "===", "!==", "<<=", ">>=", "??=", "?->", "++", "--", "->", "=>", "::", "==", "!=", "<>",
    "<=", ">=", "&&", "||", "??", "+=", "-=", "*=", "/=", ".=", "%=", "&=", "|=", "^=", "<<", ">>", "**", "#[", "+",
    "-", "*", "/", "%", "=", "<", ">", "!", ".", ",", ";", ":", "?", "(", ")", "[", "]", "{", "}", "&", "|", "^", "~",
    "@", "$", "\\",
];

const CASTS: &[(&str, &str)] = &[
    ("int", "int"),
    ("integer", "int"),
    ("bool", "bool"),
    ("boolean", "bool"),
    ("float", "float"),
    ("double", "float"),
    ("real", "float"),
    ("string", "string"),
    ("binary", "string"),
    ("array", "array"),
    ("object", "object"),
    ("unset", "null"),
];

#[inline]
pub fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_' || b >= 0x80
}

#[inline]
pub fn is_ident_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b >= 0x80
}

/// Lex a whole file, starting in inline-HTML mode.
pub fn lex(text: &str) -> Vec<Token> {
    let mut lx = Lexer::new(text, 0, text.len());
    lx.run(false);
    lx.toks
}

/// Like [`lex`], but comments and `<?php` open tags are tokens too ([`Tok::Comment`],
/// [`Tok::OpenTag`]): with them, the gaps between tokens hold nothing but whitespace. For the
/// formatter; the parser never sees these kinds.
pub fn lex_with_trivia(text: &str) -> Vec<Token> {
    let mut lx = Lexer::new(text, 0, text.len());
    lx.trivia = true;
    lx.run(false);
    lx.toks
}

/// Lex `text[start..end]` as PHP code (used for `{$expr}` inside strings).
pub fn lex_php_range(text: &str, start: usize, end: usize) -> Vec<Token> {
    let mut lx = Lexer::new(text, start, end);
    lx.run(true);
    lx.toks
}

struct Lexer<'a> {
    text: &'a str,
    b: &'a [u8],
    pos: usize,
    end: usize,
    toks: Vec<Token>,
    doc: Option<Span>,
    /// Also emit comment and open-tag tokens.
    trivia: bool,
}

impl<'a> Lexer<'a> {
    fn new(text: &'a str, start: usize, end: usize) -> Self {
        Lexer { text, b: text.as_bytes(), pos: start, end, toks: Vec::new(), doc: None, trivia: false }
    }

    fn push(&mut self, kind: Tok, start: usize, end: usize) {
        let doc = self.doc.take();
        self.toks.push(Token { kind, span: Span::new(start, end), doc });
    }

    fn at(&self, i: usize) -> u8 {
        if i < self.end {
            self.b[i]
        } else {
            0
        }
    }

    fn run(&mut self, mut php: bool) {
        loop {
            if !php {
                if self.pos >= self.end {
                    break;
                }
                let start = self.pos;
                match self.find_open_tag(start) {
                    Some((tag_start, tag_len, echo)) => {
                        if tag_start > start {
                            self.push(Tok::InlineHtml, start, tag_start);
                        }
                        self.pos = tag_start + tag_len;
                        if echo {
                            self.push(Tok::OpenTagEcho, tag_start, tag_start + tag_len);
                        } else if self.trivia {
                            // the tag itself, without the whitespace character the scan included
                            let end = (tag_start + 5).min(tag_start + tag_len);
                            let end = if self.text[tag_start..end].eq_ignore_ascii_case("<?php") {
                                end
                            } else {
                                tag_start + 2
                            };
                            self.trivia_token(Tok::OpenTag, tag_start, end);
                        }
                        php = true;
                    }
                    None => {
                        self.push(Tok::InlineHtml, start, self.end);
                        self.pos = self.end;
                        break;
                    }
                }
            } else {
                if !self.lex_php() {
                    php = false;
                }
                if self.pos >= self.end && php {
                    break;
                }
            }
        }
        let e = self.end;
        self.push(Tok::Eof, e, e);
    }

    /// Returns (tag start, tag length incl. one trailing whitespace, is `<?=`).
    fn find_open_tag(&self, from: usize) -> Option<(usize, usize, bool)> {
        let mut i = from;
        while i + 1 < self.end {
            let rel = self.text[i..self.end].find("<?")?;
            let p = i + rel;
            let rest = &self.b[p + 2..self.end];
            if rest.len() >= 3 && rest[..3].eq_ignore_ascii_case(b"php") {
                let after = rest.get(3).copied();
                match after {
                    None => return Some((p, 5, false)),
                    Some(c) if c.is_ascii_whitespace() => {
                        let mut len = 6;
                        if c == b'\r' && rest.get(4) == Some(&b'\n') {
                            len = 7;
                        }
                        return Some((p, len, false));
                    }
                    _ => {}
                }
            } else if rest.first() == Some(&b'=') {
                return Some((p, 3, true));
            } else if rest.len() >= 3 && rest[..3].eq_ignore_ascii_case(b"xml") {
                // <?xml ... ?> is markup, not PHP
            } else {
                return Some((p, 2, false));
            }
            i = p + 2;
        }
        None
    }

    /// Lex one token (or skip trivia). Returns false when switching back to HTML mode.
    fn lex_php(&mut self) -> bool {
        while self.pos < self.end && self.b[self.pos].is_ascii_whitespace() {
            self.pos += 1;
        }
        if self.pos >= self.end {
            return true;
        }
        let start = self.pos;
        let c = self.b[start];
        let n = self.at(start + 1);
        match c {
            b'?' if n == b'>' => {
                self.close_tag(start);
                return false;
            }
            b'#' if n != b'[' => self.line_comment(),
            b'/' if n == b'/' => self.line_comment(),
            b'/' if n == b'*' => self.block_comment(),
            b'$' if is_ident_start(n) => self.variable(start),
            b'\'' => self.single_quoted(),
            b'"' => self.double_quoted(),
            b'`' => self.backtick(start),
            b'0'..=b'9' => self.number(),
            b'.' if n.is_ascii_digit() => self.number(),
            b'\\' if is_ident_start(n) => self.name(),
            _ if is_ident_start(c) => self.name(),
            // `<<<` that is not a heredoc and `(` that is not a cast are punctuation
            b'<' if n == b'<' && self.at(start + 2) == b'<' && self.heredoc() => {}
            b'(' if self.cast() => {}
            _ => self.punct(start),
        }
        true
    }

    /// `?>`, which also swallows one newline after it.
    fn close_tag(&mut self, start: usize) {
        self.pos += 2;
        self.push(Tok::CloseTag, start, self.pos);
        if self.at(self.pos) == b'\n' {
            self.pos += 1;
        } else if self.at(self.pos) == b'\r' && self.at(self.pos + 1) == b'\n' {
            self.pos += 2;
        }
    }

    fn variable(&mut self, start: usize) {
        let mut i = start + 1;
        while i < self.end && is_ident_char(self.b[i]) {
            i += 1;
        }
        self.pos = i;
        self.push(Tok::Variable, start, i);
    }

    /// `` `shell command` `` (may contain variables, like a double-quoted string).
    fn backtick(&mut self, start: usize) {
        let mut i = start + 1;
        while i < self.end && self.b[i] != b'`' {
            if self.b[i] == b'\\' {
                i += 1;
            }
            i += 1;
        }
        let e = (i + 1).min(self.end);
        self.pos = e;
        self.push(Tok::Template, start, e);
    }

    /// The longest operator / punctuation at `start`, or one unknown character.
    fn punct(&mut self, start: usize) {
        for p in PUNCTS {
            if self.b[start..self.end].starts_with(p.as_bytes()) {
                self.pos = start + p.len();
                self.push(Tok::Punct(p), start, self.pos);
                return;
            }
        }
        // Unknown character: consume one full UTF-8 char.
        let mut e = start + 1;
        while e < self.end && !self.text.is_char_boundary(e) {
            e += 1;
        }
        self.pos = e;
        self.push(Tok::Unknown, start, e);
    }

    /// A trivia token: never carries (or consumes) the pending doc comment.
    fn trivia_token(&mut self, kind: Tok, start: usize, end: usize) {
        self.toks.push(Token { kind, span: Span::new(start, end), doc: None });
    }

    fn line_comment(&mut self) {
        let start = self.pos;
        let mut i = self.pos;
        while i < self.end {
            let c = self.b[i];
            if c == b'\n' {
                break;
            }
            if c == b'?' && self.at(i + 1) == b'>' {
                break;
            }
            i += 1;
        }
        self.pos = i;
        if self.trivia {
            self.trivia_token(Tok::Comment, start, i);
        }
    }

    fn block_comment(&mut self) {
        let start = self.pos;
        let mut i = start + 2;
        while i + 1 < self.end && !(self.b[i] == b'*' && self.b[i + 1] == b'/') {
            i += 1;
        }
        let e = if i + 1 < self.end { i + 2 } else { self.end };
        let is_doc = e - start >= 5 && self.at(start + 2) == b'*' && self.at(start + 3).is_ascii_whitespace();
        if is_doc {
            self.doc = Some(Span::new(start, e));
        }
        self.pos = e;
        if self.trivia {
            self.trivia_token(Tok::Comment, start, e);
        }
    }

    fn single_quoted(&mut self) {
        let start = self.pos;
        let mut i = start + 1;
        while i < self.end && self.b[i] != b'\'' {
            if self.b[i] == b'\\' {
                i += 1;
            }
            i += 1;
        }
        let e = (i + 1).min(self.end);
        self.pos = e;
        self.push(Tok::Str, start, e);
    }

    fn double_quoted(&mut self) {
        let start = self.pos;
        let mut i = start + 1;
        let mut interp = false;
        while i < self.end && self.b[i] != b'"' {
            match self.b[i] {
                b'\\' => i += 1,
                b'$' if self.at(i + 1) == b'{' => {
                    interp = true;
                    i = self.skip_brace_expr(i + 1);
                }
                b'$' if is_ident_start(self.at(i + 1)) => interp = true,
                b'{' if self.at(i + 1) == b'$' => {
                    interp = true;
                    i = self.skip_brace_expr(i);
                }
                _ => {}
            }
            i += 1;
        }
        let e = (i + 1).min(self.end);
        self.pos = e;
        self.push(if interp { Tok::Template } else { Tok::Str }, start, e);
    }

    /// `i` is at `{` inside an interpolated string: index of the matching `}` (quotes inside the
    /// expression may be nested, e.g. `"{$a["k"]}"`).
    fn skip_brace_expr(&self, mut i: usize) -> usize {
        let mut depth = 0i32;
        while i < self.end {
            match self.b[i] {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        return i;
                    }
                }
                q @ (b'\'' | b'"') => {
                    i += 1;
                    while i < self.end && self.b[i] != q {
                        if self.b[i] == b'\\' {
                            i += 1;
                        }
                        i += 1;
                    }
                }
                _ => {}
            }
            i += 1;
        }
        self.end
    }

    fn heredoc(&mut self) -> bool {
        let start = self.pos;
        let mut i = start + 3;
        while self.at(i) == b' ' || self.at(i) == b'\t' {
            i += 1;
        }
        let quote = match self.at(i) {
            q @ (b'\'' | b'"') => {
                i += 1;
                Some(q)
            }
            _ => None,
        };
        if !is_ident_start(self.at(i)) {
            return false;
        }
        let label_start = i;
        while i < self.end && is_ident_char(self.b[i]) {
            i += 1;
        }
        let label = &self.text[label_start..i];
        if let Some(q) = quote {
            if self.at(i) != q {
                return false;
            }
            i += 1;
        }
        // rest of the line must be empty
        let mut j = i;
        while self.at(j) == b' ' || self.at(j) == b'\t' || self.at(j) == b'\r' {
            j += 1;
        }
        if j < self.end && self.b[j] != b'\n' {
            return false;
        }
        let mut line = (j + 1).min(self.end);
        let mut end = self.end;
        while line < self.end {
            let mut k = line;
            while k < self.end && (self.b[k] == b' ' || self.b[k] == b'\t') {
                k += 1;
            }
            if self.text[k..self.end].starts_with(label) {
                let after = k + label.len();
                if !is_ident_char(self.at(after)) {
                    end = after;
                    break;
                }
            }
            match self.text[line..self.end].find('\n') {
                Some(r) => line += r + 1,
                None => break,
            }
        }
        self.pos = end;
        let nowdoc = quote == Some(b'\'');
        self.push(if nowdoc { Tok::Str } else { Tok::Template }, start, end);
        true
    }

    fn number(&mut self) {
        let start = self.pos;
        let mut i = start;
        let mut float = false;
        let c = self.b[i];
        let n = self.at(i + 1);
        if c == b'0' && matches!(n, b'o' | b'O') {
            i += 2;
            while i < self.end && (self.b[i].is_ascii_digit() || self.b[i] == b'_') {
                i += 1;
            }
        } else if c == b'0' && matches!(n, b'x' | b'X') {
            i += 2;
            while i < self.end && (self.b[i].is_ascii_hexdigit() || self.b[i] == b'_') {
                i += 1;
            }
        } else if c == b'0' && matches!(n, b'b' | b'B') {
            i += 2;
            while i < self.end && matches!(self.b[i], b'0' | b'1' | b'_') {
                i += 1;
            }
        } else {
            while i < self.end && (self.b[i].is_ascii_digit() || self.b[i] == b'_') {
                i += 1;
            }
            if self.at(i) == b'.' && self.at(i + 1).is_ascii_digit() {
                float = true;
                i += 1;
                while i < self.end && (self.b[i].is_ascii_digit() || self.b[i] == b'_') {
                    i += 1;
                }
            } else if self.at(i) == b'.'
                && c != b'.'
                && !matches!(self.at(i + 1), b'.' | b'=')
                && !is_ident_start(self.at(i + 1))
            {
                float = true;
                i += 1;
            }
            if matches!(self.at(i), b'e' | b'E') {
                let mut j = i + 1;
                if matches!(self.at(j), b'+' | b'-') {
                    j += 1;
                }
                if self.at(j).is_ascii_digit() {
                    float = true;
                    i = j;
                    while i < self.end && self.b[i].is_ascii_digit() {
                        i += 1;
                    }
                }
            }
        }
        self.pos = i;
        self.push(if float { Tok::Float } else { Tok::Int }, start, i);
    }

    fn cast(&mut self) -> bool {
        let start = self.pos;
        let mut i = start + 1;
        while matches!(self.at(i), b' ' | b'\t') {
            i += 1;
        }
        let s = i;
        while self.at(i).is_ascii_alphabetic() {
            i += 1;
        }
        if i == s {
            return false;
        }
        let word = self.text[s..i].to_ascii_lowercase();
        while matches!(self.at(i), b' ' | b'\t') {
            i += 1;
        }
        if self.at(i) != b')' {
            return false;
        }
        for (k, v) in CASTS {
            if *k == word {
                self.pos = i + 1;
                self.push(Tok::Cast(v), start, self.pos);
                return true;
            }
        }
        false
    }

    fn name(&mut self) {
        let start = self.pos;
        let mut i = start;
        if self.b[i] == b'\\' {
            i += 1;
        }
        while i < self.end && is_ident_char(self.b[i]) {
            i += 1;
        }
        let mut qualified = start != i && self.b[start] == b'\\';
        while self.at(i) == b'\\' && is_ident_start(self.at(i + 1)) {
            qualified = true;
            i += 1;
            while i < self.end && is_ident_char(self.b[i]) {
                i += 1;
            }
        }
        self.pos = i;
        self.push(if qualified { Tok::Name } else { Tok::Ident }, start, i);
    }
}
