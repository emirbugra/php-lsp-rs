//! WordPress hooks in string arguments: completion of hook names in `add_action('…')` /
//! `add_filter('…')` / `do_action('…')` …, go to definition (where the hook is fired; from a
//! `do_action` the callbacks added to it), hover (how often it is fired and used), and go to
//! definition of a callback named by a string (`add_action('init', 'my_setup')`).
//!
//! Works in any project: the hook functions are WordPress's own names.

use super::laravel::string_at;
use super::laravel::strings::{open_paren, word_before, StrLit};
use crate::convert::*;
use crate::server::Server;
use lsp_types::*;
use php_index::hooks::{hook_function, HookKind, HookRole};
use php_syntax::{LineIndex, Span};
use std::collections::HashMap;

/// What the string under the cursor is.
enum At {
    /// A hook name; `kind` from the function it is passed to.
    Hook { kind: HookKind, role: HookRole },
    /// The callback of `add_action` / `add_filter` / `remove_*` named by a string.
    Callback,
}

fn kind_name(k: HookKind) -> &'static str {
    match k {
        HookKind::Action => "action",
        HookKind::Filter => "filter",
    }
}

fn hook_at(text: &str, offset: usize) -> Option<(StrLit, At)> {
    let lit = string_at(text, offset)?;
    let (paren, arg) = open_paren(text, lit.quote)?;
    let (start, word) = word_before(text, paren);
    let before = text[..start].trim_end();
    if before.ends_with("->") || before.ends_with("::") || before.ends_with("function") {
        return None;
    }
    let (kind, role) = hook_function(&word.trim_start_matches('\\').to_ascii_lowercase())?;
    match arg {
        0 => Some((lit, At::Hook { kind, role })),
        // `add_action('init', 'cb')`, `remove_filter('x', 'cb')`, `has_action('x', 'cb')`
        1 if matches!(word.to_ascii_lowercase().split('_').next(), Some("add" | "remove" | "has")) => {
            Some((lit, At::Callback))
        }
        _ => None,
    }
}

impl Server {
    pub fn hook_completion(&mut self, uri: &str, offset: usize) -> Option<Vec<CompletionItem>> {
        let doc = self.docs.get(uri)?;
        let (lit, At::Hook { kind, .. }) = hook_at(&doc.text, offset)? else { return None };
        let range = self.doc_range(uri, lit.start, lit.end)?;
        // name -> (fired?, file where it is fired / first seen)
        let mut names: HashMap<&str, (bool, &str)> = HashMap::new();
        for (file, h) in self.index.hooks() {
            if h.kind != kind {
                continue;
            }
            let fired = h.role == HookRole::Fire;
            let e = names.entry(&h.name).or_insert((fired, file));
            if fired && !e.0 {
                *e = (true, file);
            }
        }
        let root = self.roots.first().map(|r| path_to_uri(r)).unwrap_or_default();
        let items = names
            .into_iter()
            .map(|(name, (fired, file))| {
                let rel = file.strip_prefix(root.as_str()).unwrap_or(file).trim_start_matches('/');
                CompletionItem {
                    label: name.to_string(),
                    kind: Some(CompletionItemKind::EVENT),
                    detail: Some(format!("{} — {rel}", kind_name(kind))),
                    // hooks fired somewhere come first
                    sort_text: Some(format!("{}{name}", if fired { 0 } else { 1 })),
                    filter_text: Some(name.to_string()),
                    text_edit: Some(CompletionTextEdit::Edit(TextEdit { range, new_text: name.to_string() })),
                    ..Default::default()
                }
            })
            .collect();
        Some(items)
    }

    pub fn hook_definition(&mut self, uri: &str, offset: usize) -> Option<Vec<Location>> {
        let doc = self.docs.get(uri)?;
        let (lit, at) = hook_at(&doc.text, offset)?;
        let value = doc.text[lit.start..lit.end].to_string();
        match at {
            At::Hook { kind, role } => {
                // from where a hook is fired, go to its callbacks; elsewhere to where it is fired
                let want = if role == HookRole::Fire { HookRole::Add } else { HookRole::Fire };
                let mut locs = self.hook_sites(&value, kind, want);
                if locs.is_empty() && role != HookRole::Fire {
                    locs = self.hook_sites(&value, kind, HookRole::Add);
                }
                (!locs.is_empty()).then_some(locs)
            }
            At::Callback => {
                let (class, method) = match value.split_once("::") {
                    Some((c, m)) => (Some(c.trim_start_matches('\\').to_string()), m.to_string()),
                    None => (None, value.trim_start_matches('\\').to_string()),
                };
                let loc = match class {
                    Some(c) => self.index.find_member(&c, &method, php_index::MemberKind::Method),
                    None => self.index.find_function(&method),
                }?;
                Some(vec![crate::texts::Texts::default().location(self, loc)?])
            }
        }
    }

    pub fn hook_hover(&mut self, uri: &str, offset: usize) -> Option<Hover> {
        let doc = self.docs.get(uri)?;
        let (lit, At::Hook { kind, .. }) = hook_at(&doc.text, offset)? else { return None };
        let value = doc.text[lit.start..lit.end].to_string();
        let (mut fired, mut added) = (Vec::new(), 0usize);
        for (file, h) in self.index.hooks() {
            if h.kind != kind || *h.name != *value {
                continue;
            }
            match h.role {
                HookRole::Fire => fired.push(file.to_string()),
                HookRole::Add => added += 1,
                HookRole::Other => {}
            }
        }
        let root = self.roots.first().map(|r| path_to_uri(r)).unwrap_or_default();
        let mut body = format!("**{}** `{value}`\n\n", kind_name(kind));
        if fired.is_empty() {
            body.push_str("Not fired in the indexed code (it may be fired with a dynamic name).");
        } else {
            body.push_str(&format!("Fired in {} place{}:\n", fired.len(), if fired.len() == 1 { "" } else { "s" }));
            fired.sort();
            fired.dedup();
            for f in fired.iter().take(5) {
                body.push_str(&format!("- {}\n", f.strip_prefix(root.as_str()).unwrap_or(f).trim_start_matches('/')));
            }
            if fired.len() > 5 {
                body.push_str("- …\n");
            }
        }
        body.push_str(&format!("\n{added} callback{} added", if added == 1 { "" } else { "s" }));
        Some(Hover {
            contents: HoverContents::Markup(MarkupContent { kind: MarkupKind::Markdown, value: body }),
            range: self.doc_range(uri, lit.start, lit.end),
        })
    }

    /// Locations of the string literals naming hook `name` with this role.
    fn hook_sites(&self, name: &str, kind: HookKind, role: HookRole) -> Vec<Location> {
        let mut by_file: HashMap<String, Vec<Span>> = HashMap::new();
        for (file, h) in self.index.hooks() {
            if h.kind == kind && h.role == role && *h.name == *name {
                by_file.entry(file.to_string()).or_default().push(h.span);
            }
        }
        let mut out = Vec::new();
        for (file, spans) in by_file {
            let Some(text) = self.text_for(&file) else { continue };
            let lines = LineIndex::new(&text);
            for s in spans {
                out.push(Location { uri: lsp_uri(&file), range: range(&text, &lines, s) });
            }
        }
        out.sort_by(|a, b| (a.uri.as_str(), a.range.start.line).cmp(&(b.uri.as_str(), b.range.start.line)));
        out
    }
}
