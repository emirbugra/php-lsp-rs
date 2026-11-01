//! Laravel model members that exist only at runtime: relations (`$user->posts`), accessors
//! (`getFullNameAttribute()` -> `$user->full_name`), local scopes (`scopeActive()` -> `User::active()`)
//! and the entries of `$casts`, `$fillable`, `$dates` …

use super::collect::SymbolCollector;
use super::{ParamInfo, Symbol, SymbolKind};
use crate::istr::{istrs, IStr};
use php_syntax::*;

const REL_ONE: &[&str] = &["hasOne", "belongsTo", "morphOne", "hasOneThrough", "morphTo"];
const REL_MANY: &[&str] = &["hasMany", "belongsToMany", "morphMany", "hasManyThrough", "morphToMany", "morphedByMany"];
const MODEL_ARRAY_PROPS: &[&str] = &["fillable", "guarded", "hidden", "visible", "appends", "casts", "dates"];
const MODEL: &str = "Illuminate\\Database\\Eloquent\\Model";

/// A property that exists at runtime but not in the source.
struct VirtualProp {
    name: String,
    /// Where it comes from (method or array entry) and the part to highlight.
    range: Span,
    name_span: Span,
    ty: Option<String>,
    classes: Vec<String>,
    detail: String,
}

fn to_snake(s: &str) -> String {
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if c.is_ascii_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

fn lcfirst(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_ascii_lowercase().to_string() + c.as_str(),
        None => String::new(),
    }
}

/// `$this->hasMany(...)->orderBy(...)` -> ("hasMany", args)
fn find_relation(mut e: &Expr) -> Option<(&str, &[Arg])> {
    loop {
        match &e.kind {
            ExprKind::MethodCall { obj, name: MemberName::Ident(id), args, .. } => {
                if matches!(&obj.kind, ExprKind::Var(v) if v == "this")
                    && (REL_ONE.contains(&id.text.as_str()) || REL_MANY.contains(&id.text.as_str()))
                {
                    return Some((id.text.as_str(), args.as_slice()));
                }
                e = obj;
            }
            ExprKind::Paren(x) => e = x,
            _ => return None,
        }
    }
}

/// The relation a method body returns, if any.
fn relation_in_body(stmts: &[Stmt]) -> Option<(&str, &[Arg])> {
    for s in stmts {
        match &s.kind {
            StmtKind::Exprs(v) => {
                if let Some(r) = v.iter().find_map(find_relation) {
                    return Some(r);
                }
            }
            StmtKind::Expr(e) => {
                if let Some(r) = find_relation(e) {
                    return Some(r);
                }
            }
            _ => {}
        }
    }
    None
}

impl SymbolCollector<'_> {
    fn lit_str(&self, e: &Expr) -> Option<String> {
        e.string_value(self.src).map(str::to_string)
    }

    /// `Foo::class` -> FQN of `Foo`.
    fn class_const_name(&self, e: &Expr, class_fqn: &str) -> Option<String> {
        if let ExprKind::ClassConst { class: ClassRef::Name(n), name } = &e.kind {
            if name.text == "class" {
                return Some(match n.text.to_ascii_lowercase().as_str() {
                    "self" | "static" => class_fqn.to_string(),
                    _ => self.ctx.resolve_class(&n.text),
                });
            }
        }
        None
    }

    fn virtual_prop(&mut self, class_fqn: &str, p: VirtualProp) {
        // never shadow a real declaration
        if self
            .out
            .iter()
            .any(|s| s.kind == SymbolKind::Property && s.container.as_deref() == Some(class_fqn) && s.name == p.name)
        {
            return;
        }
        let mut sym = Symbol::member(SymbolKind::Property, p.name, class_fqn, p.range, p.name_span);
        sym.is_virtual = true;
        sym.ty = p.ty.map(IStr::from);
        sym.type_classes = istrs(p.classes);
        sym.detail = IStr::from(p.detail);
        self.out.push(sym);
    }

    /// Relationships, accessors and scopes declared as methods.
    pub(super) fn eloquent_method_members(
        &mut self,
        decl: &FunctionDecl,
        class_fqn: &str,
        params: &[ParamInfo],
        ret_classes: &[IStr],
    ) {
        let Some(name) = &decl.name else { return };
        if let Some((rel, args)) = decl.body.as_deref().and_then(relation_in_body) {
            self.relation_prop(decl, name, class_fqn, rel, args);
        }
        self.accessor_prop(decl, name, class_fqn, ret_classes);
        // local scope scopeActive($query) -> active()
        let n = name.text.as_str();
        if n.len() > 5 && n.starts_with("scope") && n.as_bytes()[5].is_ascii_uppercase() {
            let scope = lcfirst(&n[5..]);
            let mut sym = Symbol::member(SymbolKind::Method, scope.clone(), class_fqn, decl.span, name.span);
            sym.is_virtual = true;
            sym.is_static = true;
            sym.params = params.iter().skip(1).cloned().collect();
            sym.type_classes = vec![IStr::from("Illuminate\\Database\\Eloquent\\Builder<static>".to_string())];
            sym.ty = Some(IStr::from("Builder".to_string()));
            sym.detail = IStr::from(format!(
                "public static function {scope}({}): Builder // scope",
                Self::param_list(&sym.params)
            ));
            self.out.push(sym);
        }
    }

    /// `function posts() { return $this->hasMany(Post::class); }` -> `$posts` is a collection of posts.
    fn relation_prop(&mut self, decl: &FunctionDecl, name: &Ident, class_fqn: &str, rel: &str, args: &[Arg]) {
        let related = if rel == "morphTo" {
            MODEL.to_string()
        } else {
            args.first()
                .and_then(|a| {
                    self.class_const_name(&a.value, class_fqn).or_else(|| {
                        self.lit_str(&a.value).map(|s| s.replace("\\\\", "\\").trim_start_matches('\\').to_string())
                    })
                })
                .unwrap_or_else(|| MODEL.to_string())
        };
        let short = related.rsplit('\\').next().unwrap_or(&related).to_string();
        let (ty, display) = if REL_MANY.contains(&rel) {
            (format!("Illuminate\\Database\\Eloquent\\Collection<{related}>"), format!("Collection<{short}>"))
        } else {
            (related.clone(), short.clone())
        };
        let n = &name.text;
        self.virtual_prop(
            class_fqn,
            VirtualProp {
                name: n.to_string(),
                range: decl.span,
                name_span: name.span,
                ty: Some(display.clone()),
                classes: vec![ty],
                detail: format!("public {display} ${n} // {rel}()"),
            },
        );
    }

    /// `getFooBarAttribute()` (legacy) or `fooBar(): Attribute` (Laravel 9+) -> `$foo_bar`.
    fn accessor_prop(&mut self, decl: &FunctionDecl, name: &Ident, class_fqn: &str, ret_classes: &[IStr]) {
        let n = name.text.as_str();
        if n.len() > 12 && n.starts_with("get") && n.ends_with("Attribute") {
            let prop = to_snake(&n[3..n.len() - 9]);
            let prop = lcfirst(&prop).trim_start_matches('_').to_string();
            let ty = decl.ret.as_ref().map(|r| r.text.clone());
            let detail =
                format!("public {}${prop} // accessor", ty.as_ref().map(|t| format!("{t} ")).unwrap_or_default());
            self.virtual_prop(
                class_fqn,
                VirtualProp {
                    name: prop,
                    range: decl.span,
                    name_span: name.span,
                    ty,
                    classes: ret_classes.iter().map(|c| c.to_string()).collect(),
                    detail,
                },
            );
            return;
        }
        let returns_attribute = decl
            .ret
            .as_ref()
            .map(|r| r.names.iter().any(|x| x.text.rsplit('\\').next() == Some("Attribute")))
            .unwrap_or(false);
        if returns_attribute {
            let prop = to_snake(n);
            let detail = format!("public ${prop} // attribute");
            self.virtual_prop(
                class_fqn,
                VirtualProp {
                    name: prop,
                    range: decl.span,
                    name_span: name.span,
                    ty: None,
                    classes: Vec::new(),
                    detail,
                },
            );
        }
    }

    /// `$fillable`, `$hidden`, `$appends`, `$casts`, `$dates` entries become virtual properties.
    pub(super) fn eloquent_array_props(&mut self, p: &PropertyDecl, class_fqn: &str) {
        let pname = p.name.text.as_str();
        if !MODEL_ARRAY_PROPS.contains(&pname) {
            return;
        }
        let Some(def) = &p.default else { return };
        let ExprKind::Array(items) = &def.kind else { return };
        for it in items {
            let (key_expr, val_expr) = match (&it.key, pname) {
                (Some(k), "casts") => (k, Some(&it.value)),
                (None, "casts") => continue,
                (_, _) => (&it.value, None),
            };
            let Some(attr) = self.lit_str(key_expr) else { continue };
            let mut classes = Vec::new();
            let mut ty = None;
            if pname == "dates" {
                classes.push("Illuminate\\Support\\Carbon".to_string());
                ty = Some("Carbon".to_string());
            }
            if let Some(v) = val_expr {
                if let Some(c) = self.class_const_name(v, class_fqn) {
                    ty = Some(c.rsplit('\\').next().unwrap_or(&c).to_string());
                    classes.push(c);
                } else if let Some(t) = self.lit_str(v) {
                    let t = t.to_ascii_lowercase();
                    if t.starts_with("date")
                        || t.contains("datetime")
                        || t == "timestamp"
                        || t.starts_with("immutable_date")
                    {
                        classes.push("Illuminate\\Support\\Carbon".to_string());
                    } else if t == "collection" {
                        classes.push("Illuminate\\Support\\Collection".to_string());
                    }
                    ty = Some(t);
                }
            }
            let detail =
                format!("public {}${attr} // ${pname}", ty.as_ref().map(|t| format!("{t} ")).unwrap_or_default());
            self.virtual_prop(
                class_fqn,
                VirtualProp { name: attr, range: key_expr.span, name_span: key_expr.span, ty, classes, detail },
            );
        }
    }
}
