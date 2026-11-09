mod calls;
mod code_actions;
mod code_lens;
mod completion;
mod definition;
mod extract_method;
mod format;
mod hierarchy;
mod hover;
mod inlay;
mod laravel;
mod move_class;
mod refactor;
mod references;
mod scalar;
mod signature;
mod symbols;
mod type_actions;
mod wordpress;

pub use format::FormatSettings;
use lsp_types::{CompletionItemKind, SymbolKind as LspKind};
use php_index::SymbolKind;

pub fn lsp_symbol_kind(k: SymbolKind, name: &str) -> LspKind {
    match k {
        SymbolKind::Class | SymbolKind::Trait => LspKind::CLASS,
        SymbolKind::Interface => LspKind::INTERFACE,
        SymbolKind::Enum => LspKind::ENUM,
        SymbolKind::Function => LspKind::FUNCTION,
        SymbolKind::Method if name.eq_ignore_ascii_case("__construct") => LspKind::CONSTRUCTOR,
        SymbolKind::Method => LspKind::METHOD,
        SymbolKind::Property => LspKind::PROPERTY,
        SymbolKind::ClassConst | SymbolKind::Const => LspKind::CONSTANT,
        SymbolKind::EnumCase => LspKind::ENUM_MEMBER,
    }
}

pub fn completion_kind(k: SymbolKind) -> CompletionItemKind {
    match k {
        SymbolKind::Class | SymbolKind::Trait => CompletionItemKind::CLASS,
        SymbolKind::Interface => CompletionItemKind::INTERFACE,
        SymbolKind::Enum => CompletionItemKind::ENUM,
        SymbolKind::Function => CompletionItemKind::FUNCTION,
        SymbolKind::Method => CompletionItemKind::METHOD,
        SymbolKind::Property => CompletionItemKind::PROPERTY,
        SymbolKind::ClassConst | SymbolKind::Const => CompletionItemKind::CONSTANT,
        SymbolKind::EnumCase => CompletionItemKind::ENUM_MEMBER,
    }
}
