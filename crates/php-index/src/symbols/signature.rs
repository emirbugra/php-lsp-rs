//! Parameter and return types of functions and methods: the native type hint, refined by the
//! docblock (`@param`, `@return`, `@template`), plus the signature text shown in hover.

use super::collect::{truncate, SymbolCollector};
use super::ParamInfo;
use crate::docblock::{self, DocBlock};
use crate::istr::{istrs, IStr};
use crate::names::is_builtin_type;
use crate::types::Types;
use php_syntax::*;

impl SymbolCollector<'_> {
    /// Resolve type names as written (`User`, `self`, `parent`) to class FQNs; builtin types drop out.
    fn resolve_type_names(&self, names: &[String], self_fqn: Option<&str>, parent: Option<&str>) -> Types {
        let mut out = Vec::new();
        for n in names {
            if is_builtin_type(n) {
                continue;
            }
            let r = match n.to_ascii_lowercase().as_str() {
                "self" => match self_fqn {
                    Some(s) => s.to_string(),
                    None => continue,
                },
                "parent" => match parent {
                    Some(p) => p.to_string(),
                    None => continue,
                },
                "static" => "static".to_string(),
                _ => self.ctx.resolve_class(n),
            };
            if !out.contains(&r) {
                out.push(r);
            }
        }
        out
    }

    /// Doc type text -> encoded atoms (see `types`): arrays, generics, templates, `static`.
    pub(super) fn doc_atoms(&self, ty: &str, self_fqn: Option<&str>, parent: Option<&str>) -> Types {
        crate::types::parse_doc_type(ty, &|n| {
            if self.templates_in_scope.iter().any(|t| t == n) {
                return Some(format!("@{n}"));
            }
            if is_builtin_type(n) {
                return None;
            }
            match n.to_ascii_lowercase().as_str() {
                "self" => self_fqn.map(|s| s.to_string()),
                "parent" => parent.map(|p| p.to_string()),
                "static" => Some("static".to_string()),
                _ => Some(self.ctx.resolve_class(n)),
            }
        })
    }

    /// Native type hint classes vs. a docblock type: the doc type wins when the hint is silent
    /// (`array`, `object`) or the doc refines the same class (`Collection` -> `Collection<int, User>`).
    pub(super) fn refine(native: Types, doc: Types) -> Types {
        if doc.is_empty() {
            return native;
        }
        if native.is_empty() {
            return doc;
        }
        let same = doc.iter().any(|d| {
            let b = crate::types::atom_base(d);
            native.iter().any(|n| n.eq_ignore_ascii_case(b))
        });
        if same {
            doc
        } else {
            native
        }
    }

    /// Enter the template scope of a docblock; returns the templates with their encoded bounds.
    pub(super) fn push_templates(&mut self, doc: &Option<DocBlock>) -> Vec<(IStr, IStr)> {
        let Some(d) = doc else { return Vec::new() };
        let ts = d.templates();
        for (n, _) in &ts {
            self.templates_in_scope.push(n.clone());
        }
        ts.into_iter()
            .map(|(n, b)| {
                let bound = if b.is_empty() { String::new() } else { self.doc_atoms(&b, None, None).join("|") };
                (IStr::from(n), IStr::from(bound))
            })
            .collect()
    }

    pub(super) fn pop_templates(&mut self, n: usize) {
        let l = self.templates_in_scope.len().saturating_sub(n);
        self.templates_in_scope.truncate(l);
    }

    pub(super) fn hint_classes(&self, h: &TypeHint, self_fqn: Option<&str>, parent: Option<&str>) -> Types {
        let names: Vec<String> = h.names.iter().map(|n| n.text.clone()).collect();
        self.resolve_type_names(&names, self_fqn, parent)
    }

    pub(super) fn params(
        &self,
        params: &[Param],
        doc: &Option<DocBlock>,
        self_fqn: Option<&str>,
        parent: Option<&str>,
    ) -> Vec<ParamInfo> {
        params.iter().map(|p| self.param(p, doc, self_fqn, parent)).collect()
    }

    fn param(&self, p: &Param, doc: &Option<DocBlock>, self_fqn: Option<&str>, parent: Option<&str>) -> ParamInfo {
        let mut info = ParamInfo {
            name: IStr::from(p.name.clone()),
            variadic: p.variadic,
            by_ref: p.by_ref,
            default: (p.default.as_ref().map(|d| truncate(d.span.text(self.src), 40))).map(IStr::from),
            ..Default::default()
        };
        // the `@param` tag of this parameter
        let doc_type = doc.as_ref().and_then(|d| {
            d.tags_pref("param").into_iter().find_map(|tag| {
                let (ty, name) = docblock::parse_typed_var(&tag.body);
                (name.as_deref() == Some(p.name.as_str()) && !ty.is_empty()).then_some(ty)
            })
        });
        if let Some(t) = &p.ty {
            info.ty = Some(IStr::from(t.text.clone()));
            info.ty_native = true;
            let mut classes = self.hint_classes(t, self_fqn, parent);
            if let Some(ty) = &doc_type {
                classes = Self::refine(classes, self.doc_atoms(ty, self_fqn, parent));
            }
            info.classes = istrs(classes);
        } else if let Some(ty) = doc_type {
            info.classes = istrs(self.doc_atoms(&ty, self_fqn, parent));
            info.ty = Some(IStr::from(ty));
        }
        info
    }

    /// `int $a, ?User &...$rest = null`
    pub(super) fn param_list(params: &[ParamInfo]) -> String {
        params
            .iter()
            .map(|p| {
                let mut s = String::new();
                if let Some(t) = &p.ty {
                    s.push_str(t);
                    s.push(' ');
                }
                if p.by_ref {
                    s.push('&');
                }
                if p.variadic {
                    s.push_str("...");
                }
                s.push('$');
                s.push_str(&p.name);
                if let Some(d) = &p.default {
                    s.push_str(" = ");
                    s.push_str(d);
                }
                s
            })
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// A conditional `@return ($param is X ? A : B)`, unless a native return type names a class.
    pub(super) fn cond_return(
        &self,
        f: &FunctionDecl,
        doc: &Option<DocBlock>,
        self_fqn: Option<&str>,
        parent: Option<&str>,
    ) -> Option<Box<crate::types::CondType>> {
        let t = doc.as_ref()?.tag_pref("return")?;
        let ty = docblock::split_type(&t.body).0;
        if !ty.trim_start().starts_with('(') {
            return None;
        }
        if let Some(r) = &f.ret {
            if !self.hint_classes(r, self_fqn, parent).is_empty() {
                return None;
            }
        }
        let resolve = |n: &str| {
            if self.templates_in_scope.iter().any(|t| t == n) {
                return Some(format!("@{n}"));
            }
            if is_builtin_type(n) {
                return None;
            }
            match n.to_ascii_lowercase().as_str() {
                "self" => self_fqn.map(|s| s.to_string()),
                "parent" => parent.map(|p| p.to_string()),
                "static" => Some("static".to_string()),
                _ => Some(self.ctx.resolve_class(n)),
            }
        };
        crate::types::parse_conditional(ty, &resolve).map(Box::new)
    }

    /// (display text, resolved atoms) of the return type.
    pub(super) fn return_type(
        &self,
        f: &FunctionDecl,
        doc: &Option<DocBlock>,
        self_fqn: Option<&str>,
        parent: Option<&str>,
    ) -> (Option<String>, Types) {
        let doc_ret =
            doc.as_ref().and_then(|d| d.tag_pref("return")).map(|t| docblock::split_type(&t.body).0.to_string());
        if let Some(r) = &f.ret {
            let mut classes = self.hint_classes(r, self_fqn, parent);
            if let Some(t) = &doc_ret {
                classes = Self::refine(classes, self.doc_atoms(t, self_fqn, parent));
            }
            return (Some(r.text.clone()), classes);
        }
        if let Some(ty) = doc_ret {
            if !ty.is_empty() {
                let classes = self.doc_atoms(&ty, self_fqn, parent);
                return (Some(ty), classes);
            }
        }
        (None, Vec::new())
    }
}
