//! PHP built-in function / class declarations, embedded from JetBrains' `phpstorm-stubs`
//! (Apache-2.0, see LICENSE-phpstorm-stubs). They are unpacked into a cache directory so that
//! "go to definition" on `strlen` opens a real file.

use include_dir::{include_dir, Dir};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

static STUBS: Dir = include_dir!("$CARGO_MANIFEST_DIR/stubs");

const MARKER: &str = ".complete";

fn cache_root() -> PathBuf {
    if let Some(x) = std::env::var_os("XDG_CACHE_HOME") {
        return PathBuf::from(x).join("php-lsp");
    }
    if let Some(l) = std::env::var_os("LOCALAPPDATA") {
        return PathBuf::from(l).join("php-lsp");
    }
    if let Some(h) = std::env::var_os("HOME") {
        return PathBuf::from(h).join(".cache").join("php-lsp");
    }
    std::env::temp_dir().join("php-lsp")
}

fn write_dir(dir: &Dir, base: &Path) -> std::io::Result<()> {
    for f in dir.files() {
        let target = base.join(f.path());
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(target, f.contents())?;
    }
    for d in dir.dirs() {
        write_dir(d, base)?;
    }
    Ok(())
}

/// Unpack the stubs (once per version) and return the directory containing them.
///
/// Several servers may start at the same time (one per editor window), so the stubs are written
/// to a private temporary folder first and then moved into place in one step.
pub fn ensure_extracted() -> std::io::Result<PathBuf> {
    // key on content size so a rebuilt binary with different stubs never reuses a stale cache
    fn size(d: &Dir) -> usize {
        d.files().map(|f| f.contents().len()).sum::<usize>() + d.dirs().map(size).sum::<usize>()
    }
    let dir = cache_root().join(format!("stubs-{}-{}", env!("CARGO_PKG_VERSION"), size(&STUBS)));
    if dir.join(MARKER).exists() {
        return Ok(dir);
    }
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let tmp = dir.with_extension(format!("tmp-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp)?;
    write_dir(&STUBS, &tmp)?;
    std::fs::write(tmp.join(MARKER), b"ok")?;
    // an unfinished folder from a crashed run has no marker: replace it
    if dir.exists() && !dir.join(MARKER).exists() {
        let _ = std::fs::remove_dir_all(&dir);
    }
    if std::fs::rename(&tmp, &dir).is_err() {
        // another server finished first; its copy is identical
        let _ = std::fs::remove_dir_all(&tmp);
        if !dir.join(MARKER).exists() {
            return Err(std::io::Error::other("could not move the stubs into the cache folder"));
        }
    }
    Ok(dir)
}

#[cfg(test)]
mod tests {
    #[test]
    fn concurrent_extraction_is_safe() {
        let cache = std::env::temp_dir().join(format!("phplsp_stubs_{}", std::process::id()));
        std::env::set_var("XDG_CACHE_HOME", &cache);
        let dirs: Vec<_> = (0..4).map(|_| std::thread::spawn(super::ensure_extracted)).collect();
        for d in dirs {
            let dir = d.join().unwrap().expect("extraction failed");
            assert!(dir.join(super::MARKER).exists());
            assert!(dir.join("standard").is_dir());
        }
        std::fs::remove_dir_all(&cache).ok();
    }
}
