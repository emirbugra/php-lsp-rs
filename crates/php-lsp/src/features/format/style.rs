//! Which formatter options apply to a file, and code style import / export.
//!
//! For a file, from lowest to highest priority:
//! 1. the base: the project's PhpStorm code style (`.idea/codeStyles/Project.xml` when
//!    `codeStyleConfig.xml` turns per-project settings on, or the IDE scheme it names), else the
//!    `phpLsp.format.preset` setting;
//! 2. the `.editorconfig` files from the file's directory up to the one with `root = true`;
//! 3. the `phpLsp.format.options` setting.
//!
//! Steps 1 and 2 are skipped when `phpLsp.format.useProjectCodeStyle` is off. The files are read
//! on every request (they are small), so edits to them apply at once.

use super::options_from;
use crate::convert::*;
use crate::server::Server;
use php_format::{editorconfig, EditorConfig, FormatOptions, Imported};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};

/// The format settings from the client.
#[derive(Clone, Debug)]
pub struct FormatSettings {
    pub preset: String,
    pub overrides: Map<String, Value>,
    pub use_project: bool,
}

impl FormatSettings {
    pub fn from_options(opts: &Value) -> FormatSettings {
        FormatSettings {
            preset: opts.get("formatPreset").and_then(|v| v.as_str()).unwrap_or("psr12").to_string(),
            overrides: opts.get("formatOptions").and_then(|v| v.as_object()).cloned().unwrap_or_default(),
            use_project: opts.get("useProjectCodeStyle").and_then(|v| v.as_bool()).unwrap_or(true),
        }
    }

    /// Take the format settings present in `opts` (a `workspace/didChangeConfiguration` payload).
    pub fn update(&mut self, opts: &Value) {
        if let Some(p) = opts.get("formatPreset").and_then(|v| v.as_str()) {
            self.preset = p.to_string();
        }
        if let Some(o) = opts.get("formatOptions").and_then(|v| v.as_object()) {
            self.overrides = o.clone();
        }
        if let Some(b) = opts.get("useProjectCodeStyle").and_then(|v| v.as_bool()) {
            self.use_project = b;
        }
    }

    fn preset_options(&self) -> FormatOptions {
        options_from(&self.preset, None)
    }

    fn apply_overrides(&self, o: &mut FormatOptions) {
        let base = std::mem::take(o);
        *o = apply_map(base, &self.overrides);
    }
}

pub(super) fn apply_map(mut o: FormatOptions, overrides: &Map<String, Value>) -> FormatOptions {
    for (key, value) in overrides {
        let text = match value {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        if !o.set(key, &text) {
            log::warn!("format: ignoring option {key} = {text} (unknown name or bad value)");
        }
    }
    o
}

/// Where the options for a file came from (for the log and the export header).
#[derive(Debug, Default)]
pub struct Sources {
    pub scheme: Option<PathBuf>,
    pub editorconfigs: Vec<PathBuf>,
}

/// The PhpStorm scheme a project uses, if it chose one: `.idea/codeStyles/Project.xml` with
/// per-project settings on, or the IDE scheme named by `PREFERRED_PROJECT_CODE_STYLE`.
fn project_scheme(idea: &Path) -> Option<PathBuf> {
    let config = std::fs::read_to_string(idea.join("codeStyleConfig.xml")).ok()?;
    let value = |name: &str| -> Option<String> {
        let at = config.find(&format!("name=\"{name}\""))?;
        let rest = &config[at..];
        let v = rest.find("value=\"")? + 7;
        Some(rest[v..v + rest[v..].find('"')?].to_string())
    };
    if value("USE_PER_PROJECT_SETTINGS").as_deref() == Some("true") {
        let p = idea.join("Project.xml");
        return p.is_file().then_some(p);
    }
    let name = value("PREFERRED_PROJECT_CODE_STYLE")?;
    ide_scheme(&name)
}

/// JetBrains' settings folder on this system:
/// - Linux: `$XDG_CONFIG_HOME/JetBrains`, by default `~/.config/JetBrains`
/// - macOS: `~/Library/Application Support/JetBrains`
/// - Windows: `%APPDATA%\JetBrains` (`C:\Users\<name>\AppData\Roaming\JetBrains`)
fn jetbrains_config_dir() -> Option<PathBuf> {
    let env = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    if cfg!(windows) {
        env("APPDATA").or_else(|| env("USERPROFILE").map(|h| h.join("AppData").join("Roaming")))
    } else if cfg!(target_os = "macos") {
        env("HOME").map(|h| h.join("Library").join("Application Support"))
    } else {
        env("XDG_CONFIG_HOME").or_else(|| env("HOME").map(|h| h.join(".config")))
    }
    .map(|d| d.join("JetBrains"))
}

/// `PhpStorm2025.1` → (2025, 1), for picking the newest installation.
fn phpstorm_version(dir_name: &str) -> Option<(u32, u32)> {
    let v = dir_name.strip_prefix("PhpStorm")?;
    let (major, minor) = v.split_once('.').unwrap_or((v, "0"));
    Some((major.parse().ok()?, minor.chars().take_while(char::is_ascii_digit).collect::<String>().parse().unwrap_or(0)))
}

/// `<JetBrains folder>/PhpStorm<version>/codestyles/<name>.xml` of the newest PhpStorm that has it.
fn ide_scheme(name: &str) -> Option<PathBuf> {
    let base = jetbrains_config_dir()?;
    std::fs::read_dir(&base)
        .ok()?
        .flatten()
        .filter_map(|e| {
            let version = phpstorm_version(&e.file_name().to_string_lossy())?;
            let p = e.path().join("codestyles").join(format!("{name}.xml"));
            p.is_file().then_some((version, p))
        })
        .max_by_key(|(v, _)| *v)
        .map(|(_, p)| p)
}

/// The options for `file` from its project's files, on top of `base`.
pub fn project_options(file: &Path, base: FormatOptions) -> (FormatOptions, Sources) {
    let mut sources = Sources::default();
    let mut scheme_opts = None;
    let mut configs: Vec<(PathBuf, EditorConfig)> = Vec::new();
    let mut ec_done = false;
    let mut dir = file.parent();
    while let Some(d) = dir {
        if !ec_done {
            let p = d.join(".editorconfig");
            if let Ok(text) = std::fs::read_to_string(&p) {
                let ec = EditorConfig::parse(&text);
                ec_done = ec.root;
                configs.push((p, ec));
            }
        }
        if scheme_opts.is_none() {
            let idea = d.join(".idea/codeStyles");
            if idea.is_dir() {
                if let Some(p) = project_scheme(&idea) {
                    match std::fs::read_to_string(&p)
                        .map_err(|e| e.to_string())
                        .and_then(|t| FormatOptions::from_phpstorm_xml(&t))
                    {
                        Ok(imported) => {
                            scheme_opts = Some(imported.options);
                            sources.scheme = Some(p);
                        }
                        Err(e) => log::warn!("format: cannot read {}: {e}", p.display()),
                    }
                }
                // the nearest `.idea` is the project's, even when it has no code style of its own
                scheme_opts.get_or_insert_with(|| base.clone());
            }
        }
        dir = d.parent();
    }
    let mut o = scheme_opts.unwrap_or(base);
    let (mut unsupported, mut invalid) = (Vec::new(), Vec::new());
    for (path, ec) in configs.iter().rev() {
        let dir = path.parent().unwrap_or(Path::new(""));
        let rel = file.strip_prefix(dir).unwrap_or(file).to_string_lossy().replace('\\', "/");
        ec.apply(&rel, &mut o, &mut unsupported, &mut invalid);
        sources.editorconfigs.push(path.clone());
    }
    for bad in invalid {
        log::warn!("format: .editorconfig: ignoring {bad}");
    }
    (o, sources)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportParams {
    pub text: String,
    /// `phpstorm` (XML) or `editorconfig`; guessed from the text when missing.
    pub kind: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportParams {
    /// The file whose effective options are exported; none: the settings alone.
    pub uri: Option<String>,
    /// `phpstorm` (XML) or `editorconfig`.
    pub kind: String,
    /// Scheme name for the XML.
    pub name: Option<String>,
}

/// Imported options as settings: the preset that needs the fewest overrides, and those overrides.
fn as_settings(i: &Imported) -> Value {
    let (preset, diff) = php_format::PRESETS
        .iter()
        .filter_map(|p| Some((*p, i.options.diff(&FormatOptions::preset(p)?))))
        .min_by_key(|(_, d)| d.len())
        .expect("presets");
    let options: Map<String, Value> = diff
        .into_iter()
        .map(|(k, v)| {
            let value = match v.as_str() {
                "true" => Value::Bool(true),
                "false" => Value::Bool(false),
                _ => v.parse::<u64>().map(Value::from).unwrap_or(Value::String(v.clone())),
            };
            (k.to_string(), value)
        })
        .collect();
    json!({
        "name": i.name,
        "preset": preset,
        "options": options,
        "unsupported": i.unsupported,
        "invalid": i.invalid,
    })
}

impl Server {
    /// The formatter options for document `uri`.
    pub fn format_options(&self, uri: &str) -> FormatOptions {
        self.format_options_with_sources(uri).0
    }

    fn format_options_with_sources(&self, uri: &str) -> (FormatOptions, Sources) {
        let s = &self.config.format;
        let base = s.preset_options();
        let (mut o, sources) = match uri_to_path(uri) {
            Some(path) if s.use_project => project_options(&path, base),
            _ => (base, Sources::default()),
        };
        s.apply_overrides(&mut o);
        (o, sources)
    }

    /// `phpLsp/importCodeStyle`: a PhpStorm scheme or an `.editorconfig` → settings.
    pub fn import_code_style(&self, p: ImportParams) -> Result<Value, String> {
        let xml = match p.kind.as_deref() {
            Some("phpstorm") => true,
            Some("editorconfig") => false,
            _ => p.text.trim_start().starts_with('<'),
        };
        let imported = if xml { FormatOptions::from_phpstorm_xml(&p.text)? } else { EditorConfig::import(&p.text) };
        Ok(as_settings(&imported))
    }

    /// `phpLsp/exportCodeStyle`: the options in effect (for a file, or the settings alone) as a
    /// PhpStorm scheme or an `.editorconfig`.
    pub fn export_code_style(&self, p: ExportParams) -> Result<Value, String> {
        let (o, sources) = match &p.uri {
            Some(uri) => self.format_options_with_sources(uri),
            None => {
                let mut o = self.config.format.preset_options();
                self.config.format.apply_overrides(&mut o);
                (o, Sources::default())
            }
        };
        let text = match p.kind.as_str() {
            "phpstorm" => o.to_phpstorm_xml(p.name.as_deref().unwrap_or("php-lsp")),
            "editorconfig" => {
                "# PHP code style (php-lsp); PhpStorm reads the ij_* keys too\n".to_string()
                    + &editorconfig::to_editorconfig(&o)
            }
            other => return Err(format!("unknown code style format {other:?}")),
        };
        Ok(json!({
            "text": text,
            "scheme": sources.scheme.map(|p| p.display().to_string()),
            "editorconfigs": sources.editorconfigs.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_update() {
        let mut s = FormatSettings::from_options(&serde_json::json!({}));
        s.update(
            &serde_json::json!({ "formatOptions": { "CONCAT_SPACES": false, "BLANK_LINES_AFTER_OPENING_TAG": 0 } }),
        );
        let mut o = s.preset_options();
        s.apply_overrides(&mut o);
        assert!(!o.CONCAT_SPACES);
        assert_eq!(o.BLANK_LINES_AFTER_OPENING_TAG, 0);
    }

    #[test]
    fn phpstorm_versions() {
        assert_eq!(phpstorm_version("PhpStorm2025.1"), Some((2025, 1)));
        assert_eq!(phpstorm_version("PhpStorm2024.10"), Some((2024, 10)));
        assert_eq!(phpstorm_version("PhpStorm2026"), Some((2026, 0)));
        assert_eq!(phpstorm_version("IdeaIC2025.2"), None);
        assert!(phpstorm_version("PhpStorm2025.1").unwrap() > phpstorm_version("PhpStorm2024.3").unwrap());
    }

    #[test]
    fn project_files_and_priority() {
        let dir = std::env::temp_dir().join(format!("php-lsp-style-{}", std::process::id()));
        let sub = dir.join("app/Http");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::create_dir_all(dir.join(".idea/codeStyles")).unwrap();
        std::fs::write(
            dir.join(".idea/codeStyles/codeStyleConfig.xml"),
            r#"<component name="ProjectCodeStyleConfiguration"><state><option name="USE_PER_PROJECT_SETTINGS" value="true" /></state></component>"#,
        )
        .unwrap();
        std::fs::write(
            dir.join(".idea/codeStyles/Project.xml"),
            r#"<component name="ProjectCodeStyleConfiguration"><code_scheme name="Project" version="173">
                 <PHPCodeStyleSettings><option name="CONCAT_SPACES" value="false" /></PHPCodeStyleSettings>
                 <codeStyleSettings language="PHP"><indentOptions><option name="INDENT_SIZE" value="2" /></indentOptions></codeStyleSettings>
               </code_scheme></component>"#,
        )
        .unwrap();
        std::fs::write(
            dir.join(".editorconfig"),
            "root = true\n[*.php]\nij_php_space_after_type_cast = true\nindent_size = 3\n",
        )
        .unwrap();
        std::fs::write(sub.join(".editorconfig"), "[*]\nindent_size = 8\n").unwrap();
        let file = sub.join("Kernel.php");

        let (o, sources) = project_options(&file, FormatOptions::psr12());
        // the scheme is the base (PhpStorm defaults + scheme), .editorconfig files go on top,
        // the nearest one last
        assert!(!o.CONCAT_SPACES);
        assert!(o.SPACE_AFTER_TYPE_CAST);
        assert_eq!(o.INDENT_SIZE, 8);
        assert!(!o.LOWER_CASE_NULL_CONST, "PhpStorm default, not PSR-12");
        assert_eq!(sources.editorconfigs.len(), 2);
        assert!(sources.scheme.is_some());

        let s = FormatSettings {
            preset: "psr12".into(),
            overrides: serde_json::json!({ "INDENT_SIZE": 5 }).as_object().unwrap().clone(),
            use_project: true,
        };
        let mut o2 = o.clone();
        s.apply_overrides(&mut o2);
        assert_eq!(o2.INDENT_SIZE, 5);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
