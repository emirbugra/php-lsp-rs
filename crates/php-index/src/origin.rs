//! Where an indexed file comes from: the user's project, the built-in stubs or a library under
//! `vendor/`. Decided from the workspace layout, not from the file name alone, so a project that
//! itself lives under some `.../vendor/...` folder still counts as project code.

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Origin {
    Project,
    Stub,
    Vendor,
}

/// Folder URIs (`file:///...`, without a trailing slash) that classify files.
#[derive(Clone, Debug, Default)]
pub struct Origins {
    roots: Vec<String>,
    vendor_dirs: Vec<String>,
    stub_dir: Option<String>,
}

/// `uri` is `dir` itself or inside it.
fn under<'u>(uri: &'u str, dir: &str) -> Option<&'u str> {
    let rest = uri.strip_prefix(dir)?;
    (rest.is_empty() || rest.starts_with('/')).then_some(rest)
}

impl Origins {
    pub fn new(roots: Vec<String>, vendor_dirs: Vec<String>, stub_dir: Option<String>) -> Self {
        let trim = |v: Vec<String>| v.into_iter().map(|s| s.trim_end_matches('/').to_string()).collect();
        Origins {
            roots: trim(roots),
            vendor_dirs: trim(vendor_dirs),
            stub_dir: stub_dir.map(|s| s.trim_end_matches('/').to_string()),
        }
    }

    pub fn of(&self, uri: &str) -> Origin {
        if self.stub_dir.as_deref().is_some_and(|d| under(uri, d).is_some()) {
            return Origin::Stub;
        }
        if self.vendor_dirs.iter().any(|d| under(uri, d).is_some()) {
            return Origin::Vendor;
        }
        // projects without composer.json: a `vendor` folder below the workspace root
        let inside = self.roots.iter().filter_map(|r| under(uri, r)).min_by_key(|rest| rest.len());
        let rest = inside.unwrap_or(uri);
        if rest.split('/').any(|part| part == "vendor") {
            Origin::Vendor
        } else {
            Origin::Project
        }
    }

    /// Stub or vendor code: read-only for the user, no diagnostics, not renamed.
    pub fn is_library(&self, uri: &str) -> bool {
        self.of(uri) != Origin::Project
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_by_layout() {
        let o = Origins::new(
            vec!["file:///home/u/vendor/shop".into()],
            vec!["file:///home/u/vendor/shop/vendor".into()],
            Some("file:///home/u/.cache/php-lsp/stubs-1".into()),
        );
        assert_eq!(o.of("file:///home/u/vendor/shop/app/User.php"), Origin::Project);
        assert_eq!(o.of("file:///home/u/vendor/shop/vendor/laravel/x.php"), Origin::Vendor);
        assert_eq!(o.of("file:///home/u/vendor/shop/modules/a/vendor/b/c.php"), Origin::Vendor);
        assert_eq!(o.of("file:///home/u/.cache/php-lsp/stubs-1/standard/basic.php"), Origin::Stub);
        assert_eq!(o.of("file:///home/u/vendor/shopping/x.php"), Origin::Vendor);
    }
}
