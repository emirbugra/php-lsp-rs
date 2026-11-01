//! Declarations: functions, classes and their members, parameters, types, attributes.

use super::Parser;
use crate::ast::*;
use crate::lexer::Tok;
use crate::span::Span;

impl Parser<'_> {
    pub(super) fn parse_attributes(&mut self) {
        while self.at_punct("#[") {
            self.bump();
            loop {
                if self.at_punct("]") || self.at_eof() {
                    break;
                }
                let Some(n) = self.name_here() else {
                    self.error_here("expected attribute name");
                    break;
                };
                let start = n.span.start as usize;
                let args = if self.at_punct("(") { self.parse_args() } else { Vec::new() };
                let e = self.mk_expr(ExprKind::New { class: ClassRef::Name(n), args }, start);
                self.attrs.push(e);
                if !self.eat_punct(",") {
                    break;
                }
            }
            self.expect_punct("]");
        }
    }

    pub(super) fn parse_type(&mut self) -> Option<TypeHint> {
        let start = self.span().start as usize;
        let mut nullable = false;
        let mut names = Vec::new();
        if self.eat_punct("?") {
            nullable = true;
        }
        let mut any = false;
        loop {
            if self.at_punct("(") {
                // DNF type
                self.bump();
                if let Some(inner) = self.parse_type() {
                    names.extend(inner.names);
                }
                self.expect_punct(")");
                any = true;
            } else if let Some(n) = self.name_here() {
                if n.text.eq_ignore_ascii_case("null") {
                    nullable = true;
                }
                names.push(n);
                any = true;
            } else {
                break;
            }
            // `A|B` union or `A&B` intersection; `A &$x` / `A &...$x` is a by-reference parameter
            let intersection = self.at_punct("&")
                && !matches!(self.peek_at(1).kind, Tok::Variable | Tok::Punct("...") | Tok::Punct("&"));
            if self.at_punct("|") || intersection {
                self.bump();
            } else {
                break;
            }
        }
        if !any {
            return None;
        }
        let span = Span::new(start, self.prev_end());
        Some(TypeHint { names, nullable, text: span.text(self.src).to_string(), span })
    }

    pub(super) fn parse_params(&mut self) -> Vec<Param> {
        let mut params = Vec::new();
        if !self.expect_punct("(") {
            return params;
        }
        loop {
            if self.at_punct(")") || self.at_eof() {
                break;
            }
            let before = self.pos;
            self.attrs.clear();
            if self.at_punct("#[") {
                self.parse_attributes();
            }
            let attrs = std::mem::take(&mut self.attrs);
            let mut promoted = None;
            loop {
                if (self.at_kw("public") || self.at_kw("protected") || self.at_kw("private"))
                    && self.skip_set_visibility()
                {
                    promoted = promoted.or(Some(Visibility::Public));
                    continue;
                }
                if self.at_kw("public") {
                    promoted = Some(Visibility::Public);
                } else if self.at_kw("protected") {
                    promoted = Some(Visibility::Protected);
                } else if self.at_kw("private") {
                    promoted = Some(Visibility::Private);
                } else if self.at_kw("readonly") {
                    promoted = promoted.or(Some(Visibility::Public));
                } else {
                    break;
                }
                self.bump();
            }
            let ty = if self.kind() != Tok::Variable && !self.at_punct("&") && !self.at_punct("...") {
                self.parse_type()
            } else {
                None
            };
            let by_ref = self.eat_punct("&");
            let variadic = self.eat_punct("...");
            if self.kind() == Tok::Variable {
                let t = self.bump();
                let name = self.text_of(t)[1..].to_string();
                let default = if self.eat_punct("=") { Some(self.parse_expr()) } else { None };
                if self.at_punct("{") {
                    self.skip_braces(); // property hooks on a promoted parameter
                }
                params.push(Param { name, span: t.span, ty, default, by_ref, variadic, promoted, attrs });
            } else {
                self.error_here("expected parameter variable");
            }
            if !self.eat_punct(",") {
                break;
            }
            if self.pos == before {
                self.bump();
            }
        }
        self.expect_punct(")");
        params
    }

    pub(super) fn parse_function(&mut self, doc: Option<Span>, start: usize) -> FunctionDecl {
        self.bump(); // function
        let by_ref = self.eat_punct("&");
        let name = self.ident_here();
        let attrs = std::mem::take(&mut self.attrs);
        let params = self.parse_params();
        let ret = if self.eat_punct(":") { self.parse_type() } else { None };
        let body = if self.at_punct("{") {
            Some(self.parse_braced_block())
        } else {
            self.expect_semi();
            None
        };
        FunctionDecl {
            name,
            params,
            ret,
            by_ref,
            body,
            attrs,
            doc,
            span: Span::new(start, self.prev_end()),
            ..Default::default()
        }
    }

    /// At `private` / `protected` / `public` followed by `(set)`: consume all four tokens.
    fn skip_set_visibility(&mut self) -> bool {
        if self.peek_punct(1, "(") && self.peek_kw(2, "set") && self.peek_punct(3, ")") {
            for _ in 0..4 {
                self.bump();
            }
            return true;
        }
        false
    }

    /// Skip a balanced `{ ... }` group (property hooks).
    fn skip_braces(&mut self) {
        let mut depth = 0;
        while !self.at_eof() {
            if self.at_punct("{") {
                depth += 1;
            } else if self.at_punct("}") {
                depth -= 1;
                if depth == 0 {
                    self.bump();
                    return;
                }
            }
            self.bump();
        }
    }

    fn parse_modifiers(&mut self) -> Modifiers {
        let mut m = Modifiers::default();
        loop {
            if self.kind() != Tok::Ident {
                break;
            }
            let w = self.text().to_ascii_lowercase();
            if matches!(w.as_str(), "public" | "protected" | "private") && self.skip_set_visibility() {
                // `private(set)`: write-only restriction, reading stays public unless stated
                m.vis = m.vis.or(Some(Visibility::Public));
                continue;
            }
            match w.as_str() {
                "public" => m.vis = Some(Visibility::Public),
                "protected" => m.vis = Some(Visibility::Protected),
                "private" => m.vis = Some(Visibility::Private),
                "static" => m.is_static = true,
                "abstract" => m.is_abstract = true,
                "final" => m.is_final = true,
                "readonly" => m.is_readonly = true,
                "var" => m.vis = Some(Visibility::Public),
                _ => break,
            }
            self.bump();
        }
        m
    }

    pub(super) fn parse_class_decl(&mut self, doc: Option<Span>, start: usize) -> ClassDecl {
        let attrs = std::mem::take(&mut self.attrs);
        let modifiers = self.parse_modifiers();
        let kind = if self.eat_kw("class") {
            ClassKind::Class
        } else if self.eat_kw("interface") {
            ClassKind::Interface
        } else if self.eat_kw("trait") {
            ClassKind::Trait
        } else if self.eat_kw("enum") {
            ClassKind::Enum
        } else {
            self.error_here("expected class");
            ClassKind::Class
        };
        let name = self.ident_here();
        let mut decl = self.parse_class_rest(kind, name, modifiers, doc, start);
        decl.attrs = attrs;
        decl
    }

    pub(super) fn parse_class_rest(
        &mut self,
        kind: ClassKind,
        name: Option<Ident>,
        modifiers: Modifiers,
        doc: Option<Span>,
        start: usize,
    ) -> ClassDecl {
        let mut backing = None;
        if kind == ClassKind::Enum && self.eat_punct(":") {
            backing = self.parse_type();
        }
        let mut extends = Vec::new();
        let mut implements = Vec::new();
        loop {
            if self.eat_kw("extends") {
                while let Some(n) = self.name_here() {
                    extends.push(n);
                    if !self.eat_punct(",") {
                        break;
                    }
                }
            } else if self.eat_kw("implements") {
                while let Some(n) = self.name_here() {
                    implements.push(n);
                    if !self.eat_punct(",") {
                        break;
                    }
                }
            } else {
                break;
            }
        }
        let mut members = Vec::new();
        if self.expect_punct("{") {
            loop {
                if self.at_eof() || self.at_punct("}") {
                    break;
                }
                let before = self.pos;
                if let Some(m) = self.parse_member(kind) {
                    members.push(m);
                }
                members.append(&mut self.extra_members);
                if self.pos == before {
                    self.error_here("unexpected token in class body");
                    self.bump();
                }
            }
            self.expect_punct("}");
        }
        ClassDecl {
            kind,
            name,
            modifiers,
            extends,
            implements,
            backing,
            members,
            attrs: Vec::new(),
            doc,
            span: Span::new(start, self.prev_end()),
        }
    }

    fn parse_member(&mut self, class_kind: ClassKind) -> Option<Member> {
        self.attrs.clear();
        let lead_doc = self.tok().doc;
        if self.at_punct("#[") {
            self.parse_attributes();
        }
        let t = self.tok();
        let start = t.span.start as usize;
        let doc = t.doc.or(lead_doc);
        if self.at_punct(";") {
            self.bump();
            return None;
        }
        if self.at_kw("use") {
            return Some(self.parse_trait_use(start));
        }
        if class_kind == ClassKind::Enum && self.at_kw("case") {
            self.bump();
            let name = self.ident_here()?;
            let value = if self.eat_punct("=") { Some(self.parse_expr()) } else { None };
            self.expect_semi();
            return Some(Member::Case { name, value, doc, span: Span::new(start, self.prev_end()) });
        }
        let modifiers = self.parse_modifiers();
        if self.at_kw("const") {
            self.bump();
            let items = self.parse_const_items(doc);
            self.expect_semi();
            return Some(Member::Const { items, modifiers, span: Span::new(start, self.prev_end()) });
        }
        if self.at_kw("function") {
            let mut f = self.parse_function(doc, start);
            f.is_static = modifiers.is_static;
            return Some(Member::Method { decl: Box::new(f), modifiers });
        }
        let ty = if self.kind() != Tok::Variable { self.parse_type() } else { None };
        if self.kind() == Tok::Variable {
            return self.parse_properties(ty, modifiers, doc, start);
        }
        self.error_here("expected class member");
        None
    }

    /// `use A, B;` or `use A, B { A::x insteadof B; }` inside a class.
    fn parse_trait_use(&mut self, start: usize) -> Member {
        self.bump();
        let mut names = Vec::new();
        while let Some(n) = self.name_here() {
            names.push(n);
            if !self.eat_punct(",") {
                break;
            }
        }
        if self.at_punct("{") {
            // conflict resolution rules: skip balanced
            let mut depth = 0;
            loop {
                if self.at_punct("{") {
                    depth += 1;
                } else if self.at_punct("}") {
                    depth -= 1;
                    if depth == 0 {
                        self.bump();
                        break;
                    }
                } else if self.at_eof() {
                    break;
                }
                self.bump();
            }
        } else {
            self.expect_semi();
        }
        Member::TraitUse { names, span: Span::new(start, self.prev_end()) }
    }

    /// `public int $a = 1, $b;` The first property is returned, the others go to `extra_members`.
    fn parse_properties(
        &mut self,
        ty: Option<TypeHint>,
        modifiers: Modifiers,
        doc: Option<Span>,
        start: usize,
    ) -> Option<Member> {
        let mut result = None;
        let mut hooked = false;
        while self.kind() == Tok::Variable {
            let t = self.bump();
            let name = Ident { text: self.text_of(t)[1..].to_string(), span: t.span };
            let default = if self.eat_punct("=") { Some(self.parse_expr()) } else { None };
            if self.at_punct("{") {
                self.skip_braces(); // PHP 8.4 property hooks
                hooked = true;
            }
            let prop = Member::Property(PropertyDecl {
                name,
                ty: ty.clone(),
                default,
                modifiers: modifiers.clone(),
                doc,
                span: Span::new(start, self.prev_end()),
            });
            if result.is_none() {
                result = Some(prop);
            } else {
                self.extra_members.push(prop);
            }
            if hooked || !self.eat_punct(",") {
                break;
            }
        }
        if !hooked {
            self.expect_semi();
        }
        result
    }
}
