//! Scalar / array types of variables, for hover and inlay hints. The type inference only follows
//! classes; this reads what a variable is assigned where that is plain: literals, arrays, string
//! concatenation, comparisons, casts, and calls / properties declared with a scalar type
//! (`$email = $user->email` with `@property string $email`, `$n = count($items)`).

use php_index::{Analysis, Index, RefTarget};
use php_syntax::walk::{walk_expr, walk_stmts, Visitor};
use php_syntax::{Expr, ExprKind, MemberName, Stmt};
use rustc_hash::FxHashMap;

/// Assignment target start (`$x = …`) -> scalar type text.
pub fn scalar_types(src: &str, stmts: &[Stmt], analysis: &Analysis, index: &Index) -> FxHashMap<u32, String> {
    let by_end: FxHashMap<u32, &RefTarget> = analysis.refs.iter().map(|r| (r.span.end, &r.target)).collect();
    let mut c = Collect { src, index, by_end, out: FxHashMap::default() };
    walk_stmts(&mut c, stmts);
    c.out
}

struct Collect<'a> {
    src: &'a str,
    index: &'a Index,
    by_end: FxHashMap<u32, &'a RefTarget>,
    out: FxHashMap<u32, String>,
}

impl<'a> Visitor<'a> for Collect<'_> {
    fn visit_expr(&mut self, e: &'a Expr) {
        if let ExprKind::Assign { target, value, op: "=", .. } = &e.kind {
            if matches!(target.kind, ExprKind::Var(_)) {
                if let Some(t) = self.type_of(value) {
                    self.out.insert(target.span.start, t);
                }
            }
        }
        walk_expr(self, e);
    }
}

impl Collect<'_> {
    fn type_of(&self, e: &Expr) -> Option<String> {
        let t = match &e.kind {
            ExprKind::Paren(x) => return self.type_of(x),
            ExprKind::Assign { value, .. } => return self.type_of(value),
            ExprKind::Array(_) => "array",
            ExprKind::Lit => {
                let t = e.span.text(self.src).trim();
                if t.starts_with(['\'', '"']) || t.starts_with("<<<") {
                    "string"
                } else if t.contains(['.', 'e', 'E']) && !t.starts_with("0x") && !t.starts_with("0X") {
                    "float"
                } else {
                    "int"
                }
            }
            ExprKind::Interp(_) | ExprKind::Binary { op: ".", .. } => "string",
            ExprKind::Name(n) => match n.text.trim_start_matches('\\').to_ascii_lowercase().as_str() {
                "true" | "false" => "bool",
                "null" => "null",
                "php_eol" | "php_version" | "php_os" | "directory_separator" => "string",
                "php_int_max" | "php_int_min" | "php_int_size" => "int",
                _ => return None,
            },
            ExprKind::Binary {
                op: "==" | "!=" | "===" | "!==" | "<" | ">" | "<=" | ">=" | "&&" | "||" | "and" | "or" | "xor",
                ..
            }
            | ExprKind::Unary { op: "!", .. }
            | ExprKind::Isset(_)
            | ExprKind::Empty(_)
            | ExprKind::InstanceOf { .. } => "bool",
            ExprKind::Binary { op: "<=>", .. } => "int",
            ExprKind::Cast { ty, .. } => match *ty {
                "int" | "integer" => "int",
                "float" | "double" | "real" => "float",
                "string" | "binary" => "string",
                "bool" | "boolean" => "bool",
                "array" => "array",
                _ => return None,
            },
            ExprKind::Call { callee, .. } => match &callee.kind {
                ExprKind::Name(n) => return self.declared(n.span.end),
                _ => return None,
            },
            ExprKind::MethodCall { name: MemberName::Ident(i), .. }
            | ExprKind::StaticCall { name: MemberName::Ident(i), .. }
            | ExprKind::Prop { name: MemberName::Ident(i), .. }
            | ExprKind::StaticProp { name: MemberName::Ident(i), .. } => return self.declared(i.span.end),
            _ => return None,
        };
        Some(t.to_string())
    }

    /// The declared type of the function / member whose name ends at `end`, when it names no class.
    fn declared(&self, end: u32) -> Option<String> {
        let target = self.by_end.get(&end)?;
        let mut found: Option<String> = None;
        for loc in self.index.resolve(target) {
            let sym = self.index.sym(loc);
            if !sym.type_classes.is_empty() {
                return None;
            }
            let ty = sym.ty.as_deref()?.trim();
            let l = ty.to_ascii_lowercase();
            if ty.is_empty() || matches!(l.as_str(), "mixed" | "void" | "never" | "static" | "self" | "object") {
                return None;
            }
            match &found {
                Some(f) if f != ty => return None,
                _ => found = Some(ty.to_string()),
            }
        }
        found.filter(|t| t.chars().count() <= 48)
    }
}
