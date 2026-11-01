use rustc_hash::FxHashMap;

/// Namespace + `use` state used to resolve names to fully-qualified ones.
#[derive(Debug, Clone, Default)]
pub struct NameCtx {
    pub namespace: String,
    pub classes: FxHashMap<String, String>,
    pub functions: FxHashMap<String, String>,
    pub consts: FxHashMap<String, String>,
}

pub fn is_builtin_type(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "int"
            | "integer"
            | "string"
            | "bool"
            | "boolean"
            | "float"
            | "double"
            | "array"
            | "object"
            | "mixed"
            | "void"
            | "null"
            | "callable"
            | "iterable"
            | "resource"
            | "false"
            | "true"
            | "never"
            | "numeric"
    )
}

impl NameCtx {
    pub fn set_namespace(&mut self, ns: &str) {
        self.namespace = ns.trim_matches('\\').to_string();
        self.classes.clear();
        self.functions.clear();
        self.consts.clear();
    }

    pub fn add_use(&mut self, kind: php_syntax::UseKind, fqn: &str, alias: Option<&str>) {
        let short = alias.unwrap_or_else(|| fqn.rsplit('\\').next().unwrap_or(fqn));
        match kind {
            php_syntax::UseKind::Class => {
                self.classes.insert(short.to_ascii_lowercase(), fqn.to_string());
            }
            php_syntax::UseKind::Function => {
                self.functions.insert(short.to_ascii_lowercase(), fqn.to_string());
            }
            php_syntax::UseKind::Const => {
                self.consts.insert(short.to_string(), fqn.to_string());
            }
        }
    }

    fn prefixed(&self, name: &str) -> String {
        if self.namespace.is_empty() {
            name.to_string()
        } else {
            format!("{}\\{}", self.namespace, name)
        }
    }

    /// Resolve a class-like name as written to a fully-qualified name (no leading backslash).
    /// `self`/`static`/`parent` are returned unchanged.
    pub fn resolve_class(&self, name: &str) -> String {
        if let Some(rest) = name.strip_prefix('\\') {
            return rest.to_string();
        }
        if matches!(name.to_ascii_lowercase().as_str(), "self" | "static" | "parent") {
            return name.to_ascii_lowercase();
        }
        if let Some(rest) = strip_prefix_ci(name, "namespace\\") {
            return self.prefixed(rest);
        }
        match name.split_once('\\') {
            Some((first, rest)) => match self.classes.get(&first.to_ascii_lowercase()) {
                Some(full) => format!("{full}\\{rest}"),
                None => self.prefixed(name),
            },
            None => match self.classes.get(&name.to_ascii_lowercase()) {
                Some(full) => full.clone(),
                None => self.prefixed(name),
            },
        }
    }

    /// Candidate FQNs for a function call, in lookup priority order.
    pub fn resolve_function(&self, name: &str) -> Vec<String> {
        self.resolve_fallback(name, &self.functions, true)
    }

    pub fn resolve_const(&self, name: &str) -> Vec<String> {
        self.resolve_fallback(name, &self.consts, false)
    }

    fn resolve_fallback(&self, name: &str, map: &FxHashMap<String, String>, ci: bool) -> Vec<String> {
        if let Some(rest) = name.strip_prefix('\\') {
            return vec![rest.to_string()];
        }
        if let Some(rest) = strip_prefix_ci(name, "namespace\\") {
            return vec![self.prefixed(rest)];
        }
        if name.contains('\\') {
            return vec![self.resolve_class(name)];
        }
        let key = if ci { name.to_ascii_lowercase() } else { name.to_string() };
        if let Some(full) = map.get(&key) {
            return vec![full.clone()];
        }
        if self.namespace.is_empty() {
            vec![name.to_string()]
        } else {
            vec![self.prefixed(name), name.to_string()]
        }
    }
}

fn strip_prefix_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    if s.len() >= prefix.len() && s.is_char_boundary(prefix.len()) && s[..prefix.len()].eq_ignore_ascii_case(prefix) {
        Some(&s[prefix.len()..])
    } else {
        None
    }
}
