use crate::convert::*;
use crate::server::Server;
use crate::texts::Texts;
use lsp_types::*;
use php_index::RefTarget;
use serde_json::Value;

impl Server {
    pub fn definition(&mut self, p: GotoDefinitionParams) -> Option<Value> {
        let pos = p.text_document_position_params;
        let uri = canon(&pos.text_document.uri);
        let offset = {
            let doc = self.docs.get(&uri)?;
            offset_of(&doc.text, &doc.lines, pos.position)
        };
        if let Some(locs) = self.laravel_definition(&uri, offset).or_else(|| self.hook_definition(&uri, offset)) {
            return serde_json::to_value(GotoDefinitionResponse::Array(locs)).ok();
        }
        let analysis = self.analyze_doc(&uri)?;
        let r = analysis.ref_at(offset);
        log::debug!("definition: offset {offset}, reference under cursor: {:?}", r.map(|r| &r.target));
        let r = r?.clone();

        if let RefTarget::Variable { decl, .. } = &r.target {
            let d = (*decl)?;
            let doc = self.docs.get(&uri)?;
            let loc = Location { uri: lsp_uri(&uri), range: range(&doc.text, &doc.lines, d) };
            return serde_json::to_value(GotoDefinitionResponse::Scalar(loc)).ok();
        }

        let mut targets = self.index.resolve(&r.target);
        if targets.is_empty() {
            let missing: Vec<String> = match &r.target {
                RefTarget::Class(fqn) => vec![fqn.clone()],
                RefTarget::Member { classes, .. } => classes.clone(),
                _ => Vec::new(),
            };
            let mut loaded = false;
            for c in missing {
                loaded |= self.autoload(&c);
            }
            if loaded {
                targets = self.index.resolve(&r.target);
            }
        }
        log::debug!("definition: {} target(s) in index", targets.len());
        let mut texts = Texts::default();
        let locs: Vec<Location> = targets.into_iter().filter_map(|t| texts.location(self, t)).collect();
        if locs.is_empty() {
            return None;
        }
        serde_json::to_value(GotoDefinitionResponse::Array(locs)).ok()
    }
}

/// Class FQN behind an encoded type atom (`Foo<X>` -> Foo, `Foo[]` -> Foo, `class-string<Foo>` -> Foo).
fn atom_class(atom: &str) -> Option<String> {
    let mut a = atom.trim();
    while let Some(e) = php_index::types::array_elem(a) {
        a = e;
    }
    if let Some(inner) = a.strip_prefix("class-string<").and_then(|r| r.strip_suffix('>')) {
        a = inner;
    }
    if a.starts_with("Closure<") {
        a = "Closure";
    }
    let a = php_index::types::atom_base(a);
    if a.is_empty() || a.starts_with('@') || php_index::types::is_non_class(a) {
        return None;
    }
    Some(a.trim_start_matches('\\').to_string())
}

impl Server {
    /// Go to type declaration: the class of a variable / the declared type of a member or function.
    pub fn type_definition(&mut self, p: GotoDefinitionParams) -> Option<Value> {
        let pos = p.text_document_position_params;
        let uri = canon(&pos.text_document.uri);
        let (offset, analysis, at_this) = {
            let doc = self.docs.get(&uri)?;
            let offset = offset_of(&doc.text, &doc.lines, pos.position);
            let b = doc.text.as_bytes();
            let mut s = offset.min(b.len());
            while s > 0 && (b[s - 1].is_ascii_alphanumeric() || b[s - 1] == b'_' || b[s - 1] == b'$') {
                s -= 1;
            }
            let at_this = doc.text[s..].starts_with("$this")
                && !doc.text[s + 5..].starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_');
            (offset, self.analyze_doc(&uri)?, at_this)
        };

        let mut atoms: Vec<String> = Vec::new();
        let mut container: Option<String> = None;
        if at_this {
            atoms.push(analysis.class_at(offset)?.fqn.clone());
        } else if let Some(RefTarget::Variable { classes, .. }) = analysis.ref_at(offset).map(|r| &r.target) {
            atoms = classes.clone();
            container = analysis.class_at(offset).map(|c| c.fqn.clone());
        } else {
            let locs = match self.target_at(&uri, offset)?.0 {
                super::references::Target::Syms { locs, .. } => locs,
                super::references::Target::Var { .. } => return None,
            };
            for l in locs {
                let s = self.index.sym(l);
                if s.kind.is_class_like() {
                    atoms.push(s.fqn.to_string());
                } else {
                    container = s.container.as_ref().map(|c| c.to_string());
                    atoms.extend(s.type_classes.iter().map(|c| c.to_string()));
                }
            }
        }

        let mut out = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut texts = Texts::default();
        for a in atoms {
            let a = match a.as_str() {
                "static" | "self" | "$this" => match &container {
                    Some(c) => c.clone(),
                    None => continue,
                },
                _ => a,
            };
            let Some(fqn) = atom_class(&a) else { continue };
            if !seen.insert(fqn.to_ascii_lowercase()) {
                continue;
            }
            if self.index.find_class(&fqn).is_none() {
                self.autoload(&fqn);
            }
            for l in self.index.find_classes(&fqn).to_vec() {
                out.extend(texts.location(self, l));
            }
        }
        if out.is_empty() {
            return None;
        }
        serde_json::to_value(GotoDefinitionResponse::Array(out)).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::atom_class;

    #[test]
    fn atoms_to_classes() {
        assert_eq!(atom_class("App\\User").as_deref(), Some("App\\User"));
        assert_eq!(atom_class("App\\User[][]").as_deref(), Some("App\\User"));
        assert_eq!(
            atom_class("Illuminate\\Support\\Collection<int,App\\User>").as_deref(),
            Some("Illuminate\\Support\\Collection")
        );
        assert_eq!(atom_class("class-string<App\\User>").as_deref(), Some("App\\User"));
        assert_eq!(atom_class("Closure<App\\User>").as_deref(), Some("Closure"));
        assert_eq!(atom_class("@TValue"), None);
        assert_eq!(atom_class("mixed"), None);
    }
}
