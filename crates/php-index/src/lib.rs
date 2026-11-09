//! Symbol extraction, workspace index and semantic analysis for PHP.

pub mod analyze;
mod arg_count;
pub mod docblock;
pub mod index;
pub mod inspections;
pub mod istr;
pub mod names;
pub mod origin;
pub mod refnames;
pub mod suggest;
pub mod symbols;
pub mod types;
pub mod undefvars;

pub use analyze::{analyze, analyze_with, Analysis, ClassScope, RefTarget, Reference};
pub use index::{fuzzy_score, fuzzy_score_loose, generic_arg, strip_generic, FileData, Index, MemberKind, SymLoc};
pub use inspections::{inspect, unused_uses, Options as InspectOptions, Problem, Severity, UnusedUse};
pub use istr::{istrs, IStr};
pub use names::NameCtx;
pub use origin::{Origin, Origins};
pub use refnames::{last_segment, name_hash};
pub use suggest::{Atom, PhpVersion, Place, PropertyType, SuggestedType, TypeSuggester};
pub use symbols::{extract, Bases, ParamInfo, Symbol, SymbolKind};

/// Build index data for an already parsed file.
pub fn index_parse(uri: String, src: &str, parse: &php_syntax::Parse) -> FileData {
    FileData::new(uri, extract(src, &parse.stmts), refnames::collect(src, &parse.stmts))
}

/// Parse + extract in one go (used by the background indexer). Blade templates are indexed as
/// the code [`php_syntax::blade::to_php`] makes of them.
pub fn index_source(uri: String, src: &str) -> FileData {
    if php_syntax::blade::is_blade(&uri) {
        let code = php_syntax::blade::to_php(src);
        let parse = php_syntax::parse_code(&code);
        return index_parse(uri, &code, &parse);
    }
    let parse = php_syntax::parse(src);
    index_parse(uri, src, &parse)
}
