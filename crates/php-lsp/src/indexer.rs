//! Background workspace scanner. Parses files in parallel and streams `FileData` back.

use crate::convert::{path_to_uri, read_text};
use crossbeam_channel::Sender;
use php_index::{FileData, Origin, Origins};
use rayon::prelude::*;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

/// Capacity of the indexer -> main loop channel. When the main loop falls behind, parser threads
/// block on `send`, so parsed-but-unindexed data (and parser garbage) can't balloon memory.
pub const QUEUE: usize = 6;

pub enum IndexMsg {
    /// Parsed files, plus how many files are done so far.
    Batch {
        files: Vec<FileData>,
        done: usize,
        total: usize,
    },
    Started {
        total: usize,
    },
    Done {
        files: usize,
    },
}

const EXTENSIONS: &[&str] =
    &["php", "phtml", "inc", "php3", "php4", "php5", "php7", "php8", "module", "install", "engine", "theme", "profile"];
const MAX_FILE_SIZE: u64 = 4 * 1024 * 1024;

pub fn is_php_path(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .map(|e| EXTENSIONS.iter().any(|x| x.eq_ignore_ascii_case(e)))
        .unwrap_or(false)
}

fn skip_dir(name: &str, parent: &str) -> bool {
    matches!(name, ".git" | "node_modules" | ".svn" | ".hg" | ".idea" | ".vscode" | ".cache")
        || (name == "framework" && parent == "storage")
        || (name == "cache" && parent == "var")
}

pub fn index_file(path: &Path) -> Option<FileData> {
    let meta = std::fs::metadata(path).ok()?;
    if meta.len() > MAX_FILE_SIZE {
        return None;
    }
    let text = read_text(path)?;
    Some(php_index::index_source(path_to_uri(path), &text))
}

/// Returns (project files, vendor files).
pub fn collect_files(roots: &[PathBuf], origins: &Origins) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let mut project = Vec::new();
    let mut vendor = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for root in roots {
        let walker = ignore::WalkBuilder::new(root)
            .standard_filters(false)
            .follow_links(false)
            .filter_entry(|e| {
                if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    let name = e.file_name().to_string_lossy();
                    let parent = e
                        .path()
                        .parent()
                        .and_then(|p| p.file_name())
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    return !skip_dir(&name, &parent);
                }
                true
            })
            .build();
        for entry in walker.flatten() {
            if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                continue;
            }
            let p = entry.path();
            if !is_php_path(p) || !seen.insert(p.to_path_buf()) {
                continue;
            }
            if origins.of(&path_to_uri(p)) == Origin::Vendor {
                vendor.push(p.to_path_buf());
            } else {
                project.push(p.to_path_buf());
            }
        }
    }
    (project, vendor)
}

pub fn spawn(roots: Vec<PathBuf>, origins: Origins, index_vendor: bool, tx: Sender<IndexMsg>) {
    std::thread::Builder::new()
        .name("php-indexer".into())
        .stack_size(64 << 20)
        .spawn(move || {
            let threads =
                std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2).saturating_sub(1).clamp(1, 8);
            let pool = match rayon::ThreadPoolBuilder::new().num_threads(threads).stack_size(64 << 20).build() {
                Ok(p) => p,
                Err(_) => return,
            };
            let (project, vendor) = collect_files(&roots, &origins);
            let total = project.len() + if index_vendor { vendor.len() } else { 0 };
            let done = AtomicUsize::new(0);
            let _ = tx.send(IndexMsg::Started { total });
            for (files, enabled) in [(project, true), (vendor, index_vendor)] {
                if !enabled {
                    continue;
                }
                pool.install(|| {
                    files.par_chunks(64).for_each_with(tx.clone(), |tx, chunk| {
                        let files: Vec<FileData> = chunk.iter().filter_map(|p| index_file(p)).collect();
                        let done = done.fetch_add(chunk.len(), Ordering::Relaxed) + chunk.len();
                        // blocks while the queue is full
                        let _ = tx.send(IndexMsg::Batch { files, done, total });
                    });
                });
            }
            let _ = tx.send(IndexMsg::Done { files: done.load(Ordering::Relaxed) });
        })
        .ok();
}
