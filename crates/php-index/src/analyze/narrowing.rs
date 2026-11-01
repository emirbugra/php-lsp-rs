//! Types learned from conditions.
//!
//! Inside `if ($x instanceof User) { … }` the type of `$x` is `User`, and after the guard clause
//! `if (!$x instanceof User) { return; }` too. These facts live on the `narrowed` stack: entering a
//! branch pushes them, leaving it truncates the stack back to the mark taken before.

use super::{Analyzer, Types};
use php_syntax::walk::Visitor;
use php_syntax::*;

impl Analyzer<'_> {
    /// Narrowed type of an expression path (`$x`, `$this->a->b`), if a condition set one.
    pub(super) fn narrowed_type(&self, path: &str) -> Option<Types> {
        self.narrowed.borrow().iter().rev().find(|(p, _)| p == path).and_then(|(_, v)| v.clone())
    }

    /// Current stack height; pass it to `restore_narrowing` to drop everything pushed after it.
    pub(super) fn narrowing_mark(&self) -> usize {
        self.narrowed.borrow().len()
    }

    pub(super) fn restore_narrowing(&self, mark: usize) {
        self.narrowed.borrow_mut().truncate(mark);
    }

    pub(super) fn push_narrowing(&self, facts: Vec<(String, Types)>) {
        self.narrowed.borrow_mut().extend(facts.into_iter().map(|(p, v)| (p, Some(v))));
    }

    /// Push the types implied by `cond` being `positive` (true / false).
    pub(super) fn narrow_by_condition(&self, cond: &Expr, positive: bool) {
        let mut facts = Vec::new();
        self.condition_types(cond, positive, &mut facts);
        self.push_narrowing(facts);
    }

    /// A variable or property path was assigned: drop narrowings of it and of anything below it.
    pub(super) fn forget_narrowing(&self, path: &str) {
        let mut o = self.narrowed.borrow_mut();
        let prefix = format!("{path}->");
        let mut paths: Vec<String> =
            o.iter().map(|(p, _)| p.clone()).filter(|p| p == path || p.starts_with(&prefix)).collect();
        paths.sort();
        paths.dedup();
        for p in paths {
            o.push((p, None));
        }
    }

    /// `$a`, `$this->b->c`
    pub(super) fn expr_path(e: &Expr) -> Option<String> {
        match &e.kind {
            ExprKind::Var(n) => Some(format!("${n}")),
            ExprKind::Paren(x) => Self::expr_path(x),
            ExprKind::Prop { obj, name: MemberName::Ident(id), .. } => {
                Some(format!("{}->{}", Self::expr_path(obj)?, id.text))
            }
            _ => None,
        }
    }

    /// Types implied for expression paths when `cond` evaluates to `positive`.
    fn condition_types(&self, cond: &Expr, positive: bool, out: &mut Vec<(String, Types)>) {
        match &cond.kind {
            ExprKind::Paren(x) => self.condition_types(x, positive, out),
            ExprKind::Unary { op: "!", expr } => self.condition_types(expr, !positive, out),
            ExprKind::Binary { op: "&&" | "and", lhs, rhs } if positive => {
                self.condition_types(lhs, true, out);
                self.condition_types(rhs, true, out);
            }
            ExprKind::Binary { op: "||" | "or", lhs, rhs } if !positive => {
                self.condition_types(lhs, false, out);
                self.condition_types(rhs, false, out);
            }
            ExprKind::InstanceOf { expr, class } if positive => {
                let (Some(p), ClassRef::Name(n)) = (Self::expr_path(expr), class) else { return };
                if let Some(c) = self.resolve_class_name(&n.text) {
                    out.push((p, vec![c]));
                }
            }
            ExprKind::Call { callee, args } if positive => {
                let ExprKind::Name(n) = &callee.kind else { return };
                if !matches!(n.text.trim_start_matches('\\').to_ascii_lowercase().as_str(), "is_a" | "is_subclass_of") {
                    return;
                }
                let (Some(x), Some(c)) = (args.first(), args.get(1)) else { return };
                if let (Some(p), ExprKind::ClassConst { class: ClassRef::Name(cn), name }) =
                    (Self::expr_path(&x.value), &c.value.kind)
                {
                    if name.text == "class" {
                        if let Some(c) = self.resolve_class_name(&cn.text) {
                            out.push((p, vec![c]));
                        }
                    }
                }
            }
            _ => {}
        }
    }

    /// Does control always leave the statement list (return / throw / break / continue / exit)?
    fn always_exits(&self, stmts: &[Stmt]) -> bool {
        let Some(last) = stmts.last() else { return false };
        match &last.kind {
            StmtKind::Exprs(_) => last.jump_keyword(self.src).is_some(),
            StmtKind::Expr(e) => matches!(e.kind, ExprKind::Throw(_) | ExprKind::Exit(_)),
            StmtKind::Block(b) => self.always_exits(b),
            _ => false,
        }
    }

    /// `if / elseif / else`: each block sees its condition as true and the earlier ones as false.
    pub(super) fn visit_if(&mut self, header: &[Expr], blocks: &[Vec<Stmt>]) {
        let base = self.narrowing_mark();
        for (i, block) in blocks.iter().enumerate() {
            let Some(cond) = header.get(i) else {
                // `else`: every condition above was false
                self.walk_branch(block);
                break;
            };
            self.visit_expr(cond);
            let mark = self.narrowing_mark();
            self.narrow_by_condition(cond, true);
            self.walk_branch(block);
            self.restore_narrowing(mark);
            self.narrow_by_condition(cond, false);
        }
        let has_else = blocks.len() > header.len();
        let all_exit =
            !header.is_empty() && blocks[..header.len().min(blocks.len())].iter().all(|b| self.always_exits(b));
        if all_exit && !has_else {
            // guard clause: `if (!($x instanceof Foo)) { return; }` -> `$x` is a Foo from here on,
            // so the "all conditions were false" facts pushed above stay
            return;
        }
        if header.len() == 1
            && has_else
            && blocks.len() == 2
            && self.always_exits(&blocks[1])
            && !self.always_exits(&blocks[0])
        {
            // `if (c) { … } else { return; }`: after the statement `c` holds
            self.restore_narrowing(base);
            self.narrow_by_condition(&header[0], true);
            return;
        }
        self.restore_narrowing(base);
    }
}
