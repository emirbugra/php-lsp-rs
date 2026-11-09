//! The type of an expression.

use super::{push_unique, Analyzer, ParamTypes, Types};
use crate::docblock;
use crate::index::{generic_arg, strip_generic, MemberKind, SymLoc};
use crate::index::{ELOQUENT_BUILDER, ELOQUENT_COLLECTION, MODEL, QUERY_BUILDER};
use crate::symbols::SymbolKind;
use crate::types::{self, array_elem, has_template, Bindings};
use php_syntax::*;

/// Eloquent builder / collection methods that return one model.
const SINGLE_MODEL: &[&str] = &[
    "first",
    "firstorfail",
    "find",
    "findorfail",
    "firstor",
    "sole",
    "create",
    "forcecreate",
    "firstorcreate",
    "updateorcreate",
    "firstornew",
    "make",
    "findornew",
    "firstwhere",
    "newmodelinstance",
    "last",
    "pop",
    "shift",
    "random",
];

/// Eloquent builder methods that return a collection of models.
const MANY_MODELS: &[&str] = &["get", "findmany", "hydrate"];

impl Analyzer<'_> {
    pub(super) fn infer(&self, e: &Expr) -> Types {
        match &e.kind {
            ExprKind::Var(n) => self.infer_var(n),
            ExprKind::Paren(x) | ExprKind::Clone(x) => self.infer(x),
            ExprKind::Assign { value, .. } => self.infer(value),
            ExprKind::New { class: ClassRef::Name(n), args } => {
                let Some(fqn) = self.resolve_class_name(&n.text) else { return Vec::new() };
                vec![self.type_of_new(&fqn, args)]
            }
            ExprKind::New { .. } => Vec::new(),
            ExprKind::Call { callee, args } => self.infer_call(callee, args),
            ExprKind::MethodCall { obj, name: MemberName::Ident(id), args, .. } => {
                self.member_result_types(self.infer(obj), &id.text, MemberKind::Method, args)
            }
            ExprKind::Prop { obj, name: MemberName::Ident(id), .. } => {
                if let Some(narrowed) = Self::expr_path(e).and_then(|p| self.narrowed_type(&p)) {
                    return narrowed;
                }
                self.member_result_types(self.infer(obj), &id.text, MemberKind::Property, &[])
            }
            ExprKind::StaticCall { class, name: MemberName::Ident(id), args } => {
                self.member_result_types(self.peek_class_ref(class), &id.text, MemberKind::Method, args)
            }
            ExprKind::StaticProp { class, name: MemberName::Ident(id) } => {
                self.member_result_types(self.peek_class_ref(class), &id.text, MemberKind::Property, &[])
            }
            ExprKind::ClassConst { class, name } => self.infer_class_const(class, &name.text),
            ExprKind::Index { base, .. } => self.element_types_of_all(&self.infer(base)),
            ExprKind::Array(items) => self.infer_array(items),
            ExprKind::Ternary { cond, then, els } => self.infer_ternary(cond, then.as_deref(), els),
            ExprKind::Binary { op: "??", lhs, rhs } => {
                let mut out = self.infer(lhs);
                for c in self.infer(rhs) {
                    push_unique(&mut out, c);
                }
                out
            }
            ExprKind::Match { arms, .. } => {
                let mut out = Vec::new();
                for a in arms {
                    for c in self.infer(&a.body) {
                        push_unique(&mut out, c);
                    }
                }
                out
            }
            ExprKind::Closure(f) => self.closure_type(f, None),
            _ => Vec::new(),
        }
    }

    fn infer_var(&self, name: &str) -> Types {
        if name == "this" {
            return self.current_class().map(|c| vec![c.fqn.clone()]).unwrap_or_default();
        }
        if let Some(narrowed) = self.narrowed_type(&format!("${name}")) {
            return narrowed;
        }
        self.lookup(name).map(|v| v.classes.clone()).unwrap_or_default()
    }

    /// `foo(...)` or `$closure(...)`
    fn infer_call(&self, callee: &Expr, args: &[Arg]) -> Types {
        if let ExprKind::Name(n) = &callee.kind {
            if let Some(r) = self.builtin_call(n.text.trim_start_matches('\\'), args) {
                return r;
            }
            if let Some(r) = self.factory_call(n.text.trim_start_matches('\\'), args) {
                return r;
            }
            for cand in self.ctx.resolve_function(&n.text) {
                if let Some(loc) = self.index.find_function(&cand) {
                    return self.member_types(loc, "", args);
                }
            }
            return Vec::new();
        }
        let mut out = Vec::new();
        for c in self.infer(callee) {
            if types::atom_base(&c) == "Closure" {
                for r in types::generic_args(&c).first().map(|x| types::arg_atoms(x)).unwrap_or_default() {
                    push_unique(&mut out, r);
                }
            }
        }
        out
    }

    /// Type of member `name` (method result or property) looked up on each receiver type.
    fn member_result_types(&self, receivers: Types, name: &str, kind: MemberKind, args: &[Arg]) -> Types {
        let mut out = Vec::new();
        for receiver in receivers {
            if let Some(loc) = self.index.find_member(&receiver, name, kind) {
                for c in self.member_types(loc, &receiver, args) {
                    push_unique(&mut out, c);
                }
            }
        }
        out
    }

    /// `Foo::class` is a `class-string<Foo>`, `Status::Active` (enum case) is a `Status`.
    fn infer_class_const(&self, class: &ClassRef, name: &str) -> Types {
        let mut out = Vec::new();
        if name == "class" {
            for c in self.peek_class_ref(class) {
                push_unique(&mut out, format!("class-string<{c}>"));
            }
            return out;
        }
        for receiver in self.peek_class_ref(class) {
            if let Some(loc) = self.index.find_member(&receiver, name, MemberKind::Const) {
                if self.index.sym(loc).kind == SymbolKind::EnumCase {
                    push_unique(&mut out, receiver.clone());
                }
            }
        }
        out
    }

    /// `[new A, new B]` is an `A[]|B[]`.
    fn infer_array(&self, items: &[ArrayItem]) -> Types {
        let mut out = Vec::new();
        for it in items.iter().take(64) {
            let v = self.infer(&it.value);
            let v = if it.spread { self.element_types_of_all(&v) } else { v };
            for a in v {
                push_unique(&mut out, format!("{a}[]"));
            }
        }
        out
    }

    /// `cond ? then : els` (and `cond ?: els`), each side seeing what `cond` implies.
    fn infer_ternary(&self, cond: &Expr, then: Option<&Expr>, els: &Expr) -> Types {
        let mark = self.narrowing_mark();
        self.narrow_by_condition(cond, true);
        let mut out = match then {
            Some(t) => self.infer(t),
            None => self.infer(cond),
        };
        self.restore_narrowing(mark);
        self.narrow_by_condition(cond, false);
        for c in self.infer(els) {
            push_unique(&mut out, c);
        }
        self.restore_narrowing(mark);
        out
    }

    /// Element atoms when iterating / indexing `atom`.
    fn element_types(&self, atom: &str) -> Types {
        match array_elem(atom) {
            Some(e) => vec![e.to_string()],
            None => self.index.iter_elems(atom),
        }
    }

    pub(super) fn element_types_of_all(&self, atoms: &[String]) -> Types {
        let mut out = Vec::new();
        for a in atoms {
            for e in self.element_types(a) {
                push_unique(&mut out, e);
            }
        }
        out
    }

    /// The classes a value names when used as `$x::foo()`: `class-string<Foo>` -> `Foo`; arrays drop out.
    pub(super) fn only_classes(&self, atoms: Types) -> Types {
        let mut out = Vec::new();
        for a in atoms {
            if a.ends_with("[]") {
                continue;
            }
            if types::atom_base(&a).eq_ignore_ascii_case("class-string") {
                for x in types::generic_args(&a).first().map(|x| types::arg_atoms(x)).unwrap_or_default() {
                    push_unique(&mut out, x);
                }
            } else {
                push_unique(&mut out, a);
            }
        }
        out
    }

    /// Template bindings for a call of `loc`: receiver generics (via `@extends`) plus argument unification.
    fn call_bindings(&self, loc: SymLoc, receiver: &str, args: &[Arg]) -> Bindings {
        let sym = self.index.sym(loc);
        let mut b = match &sym.container {
            Some(c) if !receiver.is_empty() => self.index.class_bindings(receiver, c),
            _ => Bindings::default(),
        };
        let iter = |a: &str| self.index.iter_elems(a);
        let hints = if sym.container.is_some() {
            self.method_hints(&[receiver.to_string()], &sym.name, args.len(), false)
        } else {
            self.function_hints(&sym.name, args)
        };
        for (i, p) in sym.params.iter().enumerate() {
            if !p.classes.iter().any(|c| has_template(c)) {
                continue;
            }
            let actual_args: Vec<&Arg> = if p.variadic {
                args.iter().skip(i).collect()
            } else {
                args.iter()
                    .find(|a| a.name.as_ref().map(|n| n.text.as_str() == &*p.name).unwrap_or(false))
                    .or_else(|| args.get(i).filter(|a| a.name.is_none()))
                    .into_iter()
                    .collect()
            };
            for arg in actual_args {
                if arg.spread {
                    continue;
                }
                let pos = args.iter().position(|x| std::ptr::eq(x, arg)).unwrap_or(0);
                let actual = self.infer_arg(&arg.value, hints.get(pos).and_then(|h| h.as_ref()));
                for pat in p.classes.iter() {
                    types::unify(pat, &actual, &mut b, &iter);
                }
            }
        }
        // unbound parameters fall back to their `of` bound
        let mut tmpls: Vec<(String, String)> =
            sym.templates().iter().map(|(n, bd)| (n.to_string(), bd.to_string())).collect();
        if let Some(c) = sym.container.as_ref().and_then(|c| self.index.find_class(c)) {
            tmpls.extend(self.index.sym(c).templates().iter().map(|(n, bd)| (n.to_string(), bd.to_string())));
        }
        for (n, bound) in tmpls {
            if b.get(&n).is_none() && !bound.is_empty() {
                b.bind(&n, types::arg_atoms(&bound));
            }
        }
        b
    }

    /// Result type of calling / reading member (or function) `loc` on `receiver` (empty for functions).
    fn member_types(&self, loc: SymLoc, receiver: &str, args: &[Arg]) -> Types {
        let sym = self.index.sym(loc);
        let receiver_base = strip_generic(receiver);
        let container = sym.container.as_deref().unwrap_or("");
        let is_builder = |c: &str| {
            strip_generic(c).eq_ignore_ascii_case(ELOQUENT_BUILDER)
                || strip_generic(c).eq_ignore_ascii_case(QUERY_BUILDER)
        };
        let receiver_is_model =
            !is_builder(receiver_base) && !receiver_base.is_empty() && self.index.is_model(receiver_base);
        // `Model::where()` runs on a builder bound to the model
        let effective: String = if is_builder(container) && receiver_is_model {
            format!("{ELOQUENT_BUILDER}<{receiver_base}>")
        } else {
            receiver.to_string()
        };
        let effective_base = strip_generic(&effective);
        let model: Option<String> = if effective_base.eq_ignore_ascii_case(ELOQUENT_BUILDER)
            || effective_base.eq_ignore_ascii_case(ELOQUENT_COLLECTION)
        {
            generic_arg(&effective).map(|m| m.to_string())
        } else if receiver_is_model {
            Some(receiver_base.to_string())
        } else {
            None
        };

        if let Some(m) = &model {
            if let Some(t) = eloquent_result(effective_base, m, &sym.name) {
                return t;
            }
        }

        let declared: Types = if sym.type_classes.iter().any(|c| has_template(c)) {
            types::subst(&sym.type_classes, &self.call_bindings(loc, &effective, args))
        } else {
            sym.type_classes.iter().map(|c| c.to_string()).collect()
        };
        let mut out = Vec::new();
        for c in &declared {
            let mut t = if c == "static" { effective.clone() } else { c.to_string() };
            if t.contains("static") {
                t = types::replace_static(&t, model.as_deref().unwrap_or(&effective));
            }
            let tb = strip_generic(&t);
            if generic_arg(&t).is_none()
                && (tb.eq_ignore_ascii_case(ELOQUENT_BUILDER) || tb.eq_ignore_ascii_case(ELOQUENT_COLLECTION))
            {
                if let Some(m) = &model {
                    t = format!("{t}<{m}>");
                }
            } else if t.eq_ignore_ascii_case(MODEL) && effective_base.eq_ignore_ascii_case(ELOQUENT_BUILDER) {
                if let Some(m) = &model {
                    t = m.clone();
                }
            }
            push_unique(&mut out, t);
        }
        out
    }

    /// Return type of well-known array functions, which the stubs only declare as `array`.
    /// CodeIgniter 4 factories named by a string: `model('UserModel')` → `App\Models\UserModel`,
    /// `service('request')` → what `Config\Services::request()` returns, `config('App')` →
    /// `Config\App`. Only when that class / method exists, so other frameworks' helpers of the
    /// same name (`config('app.name')` in Laravel) are left alone.
    fn factory_call(&self, name: &str, args: &[Arg]) -> Option<Types> {
        let lower = name.to_ascii_lowercase();
        if !matches!(lower.as_str(), "model" | "service" | "config") {
            return None;
        }
        let key = args.first().filter(|a| !a.spread && a.name.is_none())?.value.string_value(self.src)?;
        let key = key.trim_start_matches('\\');
        if key.is_empty() || key.contains(['.', ' ', '/']) {
            return None;
        }
        let class =
            |cands: &[String]| cands.iter().find(|c| self.index.find_class(c).is_some()).map(|c| vec![c.clone()]);
        match lower.as_str() {
            "model" if key.contains('\\') => class(&[key.to_string()]),
            "model" => class(&[format!("App\\Models\\{key}"), key.to_string()]),
            "config" if key.contains('\\') => class(&[key.to_string()]),
            "config" => class(&[format!("Config\\{key}"), format!("App\\Config\\{key}")]),
            _ => ["Config\\Services", "CodeIgniter\\Config\\Services"].iter().find_map(|services| {
                let loc = self.index.find_member(services, key, MemberKind::Method)?;
                let t = self.member_types(loc, services, &[]);
                (!t.is_empty()).then_some(t)
            }),
        }
    }

    fn builtin_call(&self, name: &str, args: &[Arg]) -> Option<Types> {
        if name.contains('\\') {
            return None;
        }
        let hints = self.function_hints(name, args);
        let arg = |i: usize| {
            args.get(i)
                .filter(|a| !a.spread)
                .map(|a| self.infer_arg(&a.value, hints.get(i).and_then(|h| h.as_ref())))
                .unwrap_or_default()
        };
        let arrays = |v: Types| -> Types { v.into_iter().filter(|a| a.ends_with("[]")).collect() };
        match name.to_ascii_lowercase().as_str() {
            "array_filter"
            | "array_values"
            | "array_reverse"
            | "array_slice"
            | "array_unique"
            | "array_diff"
            | "array_diff_key"
            | "array_diff_assoc"
            | "array_intersect"
            | "array_intersect_key"
            | "array_pad"
            | "array_replace"
            | "array_splice" => Some(arrays(arg(0))),
            "array_merge" | "array_merge_recursive" | "array_replace_recursive" => {
                let mut out = Vec::new();
                for i in 0..args.len().min(8) {
                    for a in arrays(arg(i)) {
                        push_unique(&mut out, a);
                    }
                }
                Some(out)
            }
            "array_pop" | "array_shift" | "reset" | "end" | "current" | "next" | "prev" | "array_find" => {
                Some(self.element_types_of_all(&arg(0)))
            }
            "array_map" if args.len() >= 2 => {
                let mut out = Vec::new();
                for c in arg(0) {
                    if types::atom_base(&c) == "Closure" {
                        for r in types::generic_args(&c).first().map(|x| types::arg_atoms(x)).unwrap_or_default() {
                            push_unique(&mut out, format!("{r}[]"));
                        }
                    }
                }
                Some(out)
            }
            "array_chunk" => Some(arrays(arg(0)).into_iter().map(|a| format!("{a}[]")).collect()),
            "array_fill" => Some(arg(2).into_iter().map(|a| format!("{a}[]")).collect()),
            "array_combine" => Some(arrays(arg(1))),
            "iterator_to_array" => {
                Some(self.element_types_of_all(&arg(0)).into_iter().map(|a| format!("{a}[]")).collect())
            }
            _ => None,
        }
    }

    /// Infer a call argument; closure literals see `hint` as their parameter types.
    fn infer_arg(&self, e: &Expr, hint: Option<&ParamTypes>) -> Types {
        match &e.kind {
            ExprKind::Closure(f) => self.closure_type(f, hint),
            ExprKind::Paren(x) => self.infer_arg(x, hint),
            _ => self.infer(e),
        }
    }

    /// Declared / documented / inferred return atoms of a closure literal (`Closure<ret>`).
    fn closure_type(&self, f: &FunctionDecl, hint: Option<&ParamTypes>) -> Types {
        let mut ret: Types = Vec::new();
        if let Some(r) = &f.ret {
            ret = self.hint_classes(r);
        }
        if let Some(d) = f.doc {
            let doc = docblock::parse(d.text(self.src));
            if let Some(t) = doc.tag_pref("return") {
                let doc_ret = self.doc_classes(docblock::split_type(&t.body).0);
                if ret.is_empty() {
                    ret = doc_ret;
                }
            }
        }
        if ret.is_empty() {
            if let Some(body) = &f.arrow_body {
                // parameters shadow outer variables while the body is inferred
                let mark = self.narrowing_mark();
                for (i, p) in f.params.iter().enumerate() {
                    let ty = match &p.ty {
                        Some(t) if !p.variadic => self.hint_classes(t),
                        _ => hint.and_then(|h| h.get(i)).cloned().unwrap_or_default(),
                    };
                    self.push_narrowing(vec![(format!("${}", p.name), ty)]);
                }
                ret = self.infer(body);
                self.restore_narrowing(mark);
            }
        }
        if ret.is_empty() {
            vec!["Closure".to_string()]
        } else {
            vec![format!("Closure<{}>", ret.join("|"))]
        }
    }

    /// `new Foo(...)`: the class atom, with template arguments inferred from the constructor call.
    fn type_of_new(&self, fqn: &str, args: &[Arg]) -> String {
        let Some(cls) = self.index.find_class(fqn) else { return fqn.to_string() };
        let tmpls: Vec<String> = self.index.sym(cls).templates().iter().map(|(n, _)| n.to_string()).collect();
        if tmpls.is_empty() {
            return fqn.to_string();
        }
        let Some(ctor) = self.index.find_member(fqn, "__construct", MemberKind::Method) else { return fqn.to_string() };
        let b = self.call_bindings(ctor, "", args);
        let enc: Vec<String> = tmpls
            .iter()
            .map(|t| match b.get(t) {
                Some(v) if !v.is_empty() => v.join("|"),
                _ => "mixed".to_string(),
            })
            .collect();
        if enc.iter().all(|e| e == "mixed") {
            fqn.to_string()
        } else {
            format!("{fqn}<{}>", enc.join(","))
        }
    }

    /// Class refs without recording references.
    fn peek_class_ref(&self, c: &ClassRef) -> Types {
        match c {
            ClassRef::Name(n) => self.resolve_class_name(&n.text).into_iter().collect(),
            ClassRef::Expr(e) => self.only_classes(self.infer(e)),
            ClassRef::Anon(_) => Vec::new(),
        }
    }
}

/// `User::where(…)->first()` is a `User`, `->get()` a collection of users.
/// `builder` is the (generic-stripped) Eloquent builder / collection class the method runs on.
fn eloquent_result(builder: &str, model: &str, method: &str) -> Option<Types> {
    let method = method.to_ascii_lowercase();
    if builder.eq_ignore_ascii_case(ELOQUENT_BUILDER) {
        if SINGLE_MODEL.contains(&method.as_str()) {
            return Some(vec![model.to_string()]);
        }
        if MANY_MODELS.contains(&method.as_str()) {
            return Some(vec![format!("{ELOQUENT_COLLECTION}<{model}>")]);
        }
    } else if builder.eq_ignore_ascii_case(ELOQUENT_COLLECTION) && SINGLE_MODEL.contains(&method.as_str()) {
        return Some(vec![model.to_string()]);
    }
    None
}
