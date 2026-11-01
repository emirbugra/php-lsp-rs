//! Formatting through an external tool (Laravel Pint or PHP-CS-Fixer).
//!
//! The buffer is written to a sibling temp file inside the project (so the tool's config and
//! path rules apply), the tool runs on it, and the result is read back.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum Tool {
    Pint,
    CsFixer,
}

fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|d| d.join(name)).find(|p| p.is_file())
}

/// Project root that owns `file`: the deepest composer project containing it.
pub(super) fn project_root(roots: &[PathBuf], file: &Path) -> Option<PathBuf> {
    roots.iter().filter(|r| file.starts_with(r)).max_by_key(|r| r.components().count()).cloned()
}

/// The tool for an explicit `phpLsp.formatter.tool` / `phpLsp.formatter.path` setting.
pub(super) fn explicit_tool(setting: &str, custom: &str, root: &Path) -> Option<(Tool, PathBuf)> {
    if !custom.is_empty() {
        let tool = if custom.contains("pint") { Tool::Pint } else { Tool::CsFixer };
        return Some((tool, PathBuf::from(custom)));
    }
    let bin = |n: &str| Some(root.join("vendor/bin").join(n)).filter(|p| p.is_file()).or_else(|| which(n));
    match setting {
        "pint" => bin("pint").map(|p| (Tool::Pint, p)),
        "php-cs-fixer" => bin("php-cs-fixer").map(|p| (Tool::CsFixer, p)),
        _ => None,
    }
}

/// The tool a project has chosen: its config file (`pint.json`, `.php-cs-fixer[.dist].php`) or a
/// local install in `vendor/bin`. None means the project has no formatter of its own.
pub(super) fn project_tool(root: &Path) -> Option<(Tool, PathBuf)> {
    let local = |n: &str| Some(root.join("vendor/bin").join(n)).filter(|p| p.is_file());
    let installed = |n: &str| local(n).or_else(|| which(n));
    let has = |f: &str| root.join(f).is_file();
    if has("pint.json") {
        return installed("pint").map(|p| (Tool::Pint, p));
    }
    if has(".php-cs-fixer.php") || has(".php-cs-fixer.dist.php") {
        return installed("php-cs-fixer").map(|p| (Tool::CsFixer, p));
    }
    local("pint").map(|p| (Tool::Pint, p)).or_else(|| local("php-cs-fixer").map(|p| (Tool::CsFixer, p)))
}

/// Run the tool on a copy of `text` next to `file`; the formatted text.
pub(super) fn format_with(tool: Tool, bin: &Path, root: &Path, file: &Path, text: &str) -> Result<String, String> {
    let dir = file.parent().ok_or("no parent folder")?;
    let name = file.file_name().ok_or("no file name")?.to_string_lossy();
    let tmp = dir.join(format!("__phplsp_fmt_{}_{name}", std::process::id()));
    std::fs::write(&tmp, text).map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
    let res = run(tool, bin, root, &tmp);
    let formatted = std::fs::read_to_string(&tmp);
    let _ = std::fs::remove_file(&tmp);
    res?;
    formatted.map_err(|e| e.to_string())
}

fn run(tool: Tool, bin: &Path, root: &Path, file: &Path) -> Result<(), String> {
    let mut cmd = Command::new(bin);
    match tool {
        Tool::Pint => {
            cmd.arg("--no-interaction").arg(file);
        }
        Tool::CsFixer => {
            // older PHP-CS-Fixer builds refuse to start on newer PHP versions
            cmd.env("PHP_CS_FIXER_IGNORE_ENV", "1");
            cmd.args(["fix", "--no-interaction", "--using-cache=no", "--path-mode=override", "--quiet"]).arg(file);
        }
    }
    let mut child = cmd
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot start {}: {e}", bin.display()))?;
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        match child.try_wait() {
            Ok(Some(st)) => {
                // PHP-CS-Fixer exits 8 when it changed files; that is success for us
                if st.success() || st.code() == Some(8) {
                    return Ok(());
                }
                let mut err = String::new();
                if let Some(mut s) = child.stderr.take() {
                    use std::io::Read;
                    let _ = s.read_to_string(&mut err);
                }
                return Err(format!("{} exited with {st}: {}", bin.display(), err.trim()));
            }
            Ok(None) if Instant::now() > deadline => {
                let _ = child.kill();
                return Err(format!("{} timed out", bin.display()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(15)),
            Err(e) => return Err(e.to_string()),
        }
    }
}
