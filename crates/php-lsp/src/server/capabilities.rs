//! What the server tells the editor it can do, in the `initialize` answer.

use lsp_types::*;
use serde_json::{json, Value};

pub(super) fn initialize_result() -> Value {
    let caps = ServerCapabilities {
        position_encoding: Some(PositionEncodingKind::UTF16),
        text_document_sync: Some(TextDocumentSyncCapability::Options(TextDocumentSyncOptions {
            open_close: Some(true),
            change: Some(TextDocumentSyncKind::INCREMENTAL),
            ..Default::default()
        })),
        definition_provider: Some(OneOf::Left(true)),
        document_formatting_provider: Some(OneOf::Left(true)),
        document_range_formatting_provider: Some(OneOf::Left(true)),
        document_on_type_formatting_provider: Some(DocumentOnTypeFormattingOptions {
            first_trigger_character: "}".into(),
            more_trigger_character: Some(vec!["\n".into()]),
        }),
        inlay_hint_provider: Some(OneOf::Left(true)),
        code_lens_provider: Some(CodeLensOptions { resolve_provider: Some(true) }),
        type_definition_provider: Some(TypeDefinitionProviderCapability::Simple(true)),
        call_hierarchy_provider: Some(CallHierarchyServerCapability::Simple(true)),
        implementation_provider: Some(ImplementationProviderCapability::Simple(true)),
        hover_provider: Some(HoverProviderCapability::Simple(true)),
        document_symbol_provider: Some(OneOf::Left(true)),
        workspace_symbol_provider: Some(OneOf::Left(true)),
        completion_provider: Some(CompletionOptions {
            trigger_characters: Some(vec![
                "$".into(),
                ">".into(),
                ":".into(),
                "\\".into(),
                "'".into(),
                "\"".into(),
                ".".into(),
            ]),
            resolve_provider: Some(false),
            ..Default::default()
        }),
        code_action_provider: Some(CodeActionProviderCapability::Options(CodeActionOptions {
            code_action_kinds: Some(vec![
                CodeActionKind::QUICKFIX,
                CodeActionKind::SOURCE_ORGANIZE_IMPORTS,
                CodeActionKind::REFACTOR_REWRITE,
                CodeActionKind::REFACTOR_EXTRACT,
                CodeActionKind::REFACTOR_INLINE,
            ]),
            work_done_progress_options: Default::default(),
            resolve_provider: Some(false),
        })),
        references_provider: Some(OneOf::Left(true)),
        document_highlight_provider: Some(OneOf::Left(true)),
        rename_provider: Some(OneOf::Right(RenameOptions {
            prepare_provider: Some(true),
            work_done_progress_options: Default::default(),
        })),
        signature_help_provider: Some(SignatureHelpOptions {
            trigger_characters: Some(vec!["(".into(), ",".into()]),
            ..Default::default()
        }),
        ..Default::default()
    };
    let mut caps = serde_json::to_value(caps).unwrap_or(Value::Null);
    // not in lsp-types' ServerCapabilities yet
    caps["typeHierarchyProvider"] = json!(true);
    json!({
        "capabilities": caps,
        "serverInfo": { "name": "php-lsp", "version": env!("CARGO_PKG_VERSION") },
    })
}
