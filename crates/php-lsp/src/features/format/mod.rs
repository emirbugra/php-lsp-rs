//! Document formatting: the built-in formatter (`php-format`, PhpStorm-style options) or the
//! project's own tool (Laravel Pint, PHP-CS-Fixer).
//!
//! `phpLsp.formatter.tool`: `builtin`, `pint`, `php-cs-fixer`, `off`, or `auto`, which uses the
//! project's tool when it has one (a `pint.json` / `.php-cs-fixer.php` or a `vendor/bin` install)
//! and the built-in formatter otherwise. Formatting a selection and indenting while typing always
//! use the built-in formatter (unless formatting is `off`).
//!
//! - `external.rs`: running Pint / PHP-CS-Fixer
//! - `style.rs`: the options for a file (preset, project's PhpStorm scheme / `.editorconfig`,
//!   settings) and code style import / export

mod external;
mod style;

pub use style::FormatSettings;

use crate::convert::*;
use crate::server::Server;
use external::Tool;
use lsp_types::*;
use php_format::{FormatError, FormatOptions};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

/// Tell the user about a formatter problem once per session (format-on-save would otherwise nag).
static WARNED: AtomicBool = AtomicBool::new(false);

enum Engine {
    Builtin,
    External { tool: Tool, bin: PathBuf, root: PathBuf },
    Off,
}

/// Formatter options from the client settings: a preset plus PhpStorm-named overrides.
pub fn options_from(preset: &str, overrides: Option<&serde_json::Map<String, Value>>) -> FormatOptions {
    let o = FormatOptions::preset(preset).unwrap_or_else(|| {
        log::warn!("format: unknown preset {preset:?}, using PSR-12");
        FormatOptions::psr12()
    });
    match overrides {
        Some(map) => style::apply_map(o, map),
        None => o,
    }
}

/// One edit covering only the lines that differ.
fn diff_edit(old: &str, new: &str) -> Option<TextEdit> {
    if old == new {
        return None;
    }
    let o: Vec<&str> = old.split_inclusive('\n').collect();
    let n: Vec<&str> = new.split_inclusive('\n').collect();
    let mut pre = 0;
    while pre < o.len() && pre < n.len() && o[pre] == n[pre] {
        pre += 1;
    }
    let mut suf = 0;
    while suf < o.len() - pre && suf < n.len() - pre && o[o.len() - 1 - suf] == n[n.len() - 1 - suf] {
        suf += 1;
    }
    let text: String = n[pre..n.len() - suf].concat();
    Some(TextEdit {
        range: Range {
            start: Position { line: pre as u32, character: 0 },
            end: Position { line: (o.len() - suf) as u32, character: 0 },
        },
        new_text: text,
    })
}

fn edits(old: &str, new: &str) -> Option<Value> {
    serde_json::to_value(diff_edit(old, new).into_iter().collect::<Vec<_>>()).ok()
}

impl Server {
    fn warn_once(&self, msg: &str) {
        if !WARNED.swap(true, Ordering::Relaxed) {
            self.notify("window/showMessage", serde_json::json!({ "type": 2, "message": msg }));
        }
    }

    fn engine(&self, uri: &str) -> Engine {
        let setting = self.config.formatter.as_str();
        match setting {
            "off" => return Engine::Off,
            "builtin" => return Engine::Builtin,
            _ => {}
        }
        let Some(file) = uri_to_path(uri) else { return Engine::Builtin };
        let roots: Vec<PathBuf> =
            self.composers.iter().map(|c| c.root.clone()).chain(self.roots.iter().cloned()).collect();
        let Some(root) = external::project_root(&roots, &file) else { return Engine::Builtin };
        let custom = self.config.formatter_path.as_str();
        let found = if setting == "auto" && custom.is_empty() {
            external::project_tool(&root)
        } else {
            external::explicit_tool(setting, custom, &root)
        };
        match found {
            Some((tool, bin)) => Engine::External { tool, bin, root },
            None if setting == "auto" => Engine::Builtin,
            None => {
                self.warn_once(&format!(
                    "PHP formatter `{setting}` not found in the project or on PATH; using the built-in formatter."
                ));
                Engine::Builtin
            }
        }
    }

    /// The built-in formatter failed: explain once (syntax errors are expected while typing).
    fn builtin_failed(&self, uri: &str, e: FormatError) -> Option<Value> {
        log::info!("format {uri}: {e}");
        match e {
            FormatError::Unbalanced(_) => {}
            FormatError::ShortOpenTag | FormatError::Changed => self.warn_once(&format!("PHP formatter: {e}")),
        }
        None
    }

    pub fn formatting(&mut self, p: DocumentFormattingParams) -> Option<Value> {
        let uri = canon(&p.text_document.uri);
        let text = self.docs.get(&uri)?.text.clone();
        let started = Instant::now();
        let formatted = match self.engine(&uri) {
            Engine::Off => return None,
            // trailing commas only where the project's PHP allows them
            Engine::Builtin => match php_format::format_for(&text, &self.format_options(&uri), {
                let v = self.php_version(&uri);
                (v.0, v.1)
            }) {
                Ok(t) => t,
                Err(e) => return self.builtin_failed(&uri, e),
            },
            Engine::External { tool, bin, root } => {
                match external::format_with(tool, &bin, &root, &uri_to_path(&uri)?, &text) {
                    Ok(t) => t,
                    Err(e) => {
                        log::warn!("format failed: {e}");
                        self.warn_once(&format!("PHP formatter failed: {}", e.lines().next().unwrap_or("")));
                        return None;
                    }
                }
            }
        };
        log::debug!("format: {:.1} ms", started.elapsed().as_secs_f64() * 1000.0);
        edits(&text, &formatted)
    }

    pub fn range_formatting(&mut self, p: DocumentRangeFormattingParams) -> Option<Value> {
        let uri = canon(&p.text_document.uri);
        if matches!(self.engine(&uri), Engine::Off) {
            return None;
        }
        let text = self.docs.get(&uri)?.text.clone();
        let (first, mut last) = (p.range.start.line as usize, p.range.end.line as usize);
        // a selection ending at the start of a line does not include that line
        if p.range.end.character == 0 && last > first {
            last -= 1;
        }
        match php_format::format_lines(&text, &self.format_options(&uri), first, last) {
            Ok(formatted) => edits(&text, &formatted),
            Err(e) => self.builtin_failed(&uri, e),
        }
    }

    /// Re-indent the line where `}` was typed, or the new line after Enter.
    pub fn on_type_formatting(&mut self, p: DocumentOnTypeFormattingParams) -> Option<Value> {
        let uri = canon(&p.text_document_position.text_document.uri);
        if matches!(self.engine(&uri), Engine::Off) {
            return None;
        }
        let o = self.format_options(&uri);
        let doc = self.docs.get(&uri)?;
        let line = p.text_document_position.position.line as usize;
        let want = php_format::line_indent(&doc.text, line, &o).ok()??;
        let (start, end) = (doc.lines.offset(&doc.text, line as u32, 0), doc.text.len());
        let rest = &doc.text[start..end];
        let current = &rest[..rest.len() - rest.trim_start_matches([' ', '\t']).len()];
        if current == want {
            return None;
        }
        let (l, c) = doc.lines.position(&doc.text, start + current.len());
        let edit = TextEdit {
            range: Range {
                start: Position { line: line as u32, character: 0 },
                end: Position { line: l, character: c },
            },
            new_text: want,
        };
        serde_json::to_value(vec![edit]).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimal_edit() {
        let e = diff_edit("a\nb\nc\nd\n", "a\nB\nX\nd\n").unwrap();
        assert_eq!((e.range.start.line, e.range.end.line), (1, 3));
        assert_eq!(e.new_text, "B\nX\n");
        assert!(diff_edit("a\n", "a\n").is_none());
        let e = diff_edit("a\n", "a\nb\n").unwrap();
        assert_eq!((e.range.start.line, e.range.end.line, e.new_text.as_str()), (1, 1, "b\n"));
    }

    #[test]
    fn options_from_settings() {
        let overrides = serde_json::json!({ "INDENT_SIZE": 2, "USE_TAB_CHARACTER": true, "NOPE": 1 });
        let o = options_from("PSR-12", overrides.as_object());
        assert_eq!((o.INDENT_SIZE, o.USE_TAB_CHARACTER), (2, true));
    }
}
