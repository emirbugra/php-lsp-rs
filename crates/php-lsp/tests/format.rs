//! The built-in formatter through LSP: whole document, a selection, and indenting while typing.
//! The `proj` fixture has no Pint / PHP-CS-Fixer, so `auto` picks the built-in formatter.

mod common;

use common::{fixture, Client};
use serde_json::{json, Value};

fn offset(text: &str, p: &Value) -> usize {
    let line = p["line"].as_u64().unwrap() as usize;
    let col = p["character"].as_u64().unwrap() as usize;
    text.split_inclusive('\n').take(line).map(str::len).sum::<usize>() + col
}

fn apply(text: &str, edits: &Value) -> String {
    let mut out = text.to_string();
    let mut list: Vec<&Value> = edits.as_array().map(|a| a.iter().collect()).unwrap_or_default();
    list.sort_by_key(|e| std::cmp::Reverse(offset(text, &e["range"]["start"])));
    for e in list {
        let (s, t) = (offset(text, &e["range"]["start"]), offset(text, &e["range"]["end"]));
        out.replace_range(s..t, e["newText"].as_str().unwrap());
    }
    out
}

const SRC: &str = "<?php\nfunction f() {\n$a = DB::table('x')\n->where('a', 1)\n->get();\nreturn $a;\n}\n";

#[test]
fn whole_document_selection_and_typing() {
    let mut c = Client::start(&fixture("proj"));
    let (uri, text) = c.open("src/Scratch.php", Some(SRC));
    let doc = json!({ "uri": uri });
    let opts = json!({ "tabSize": 4, "insertSpaces": true });

    let edits = c.request("textDocument/formatting", json!({ "textDocument": doc, "options": opts }));
    assert_eq!(
        apply(&text, &edits),
        "<?php\n\nfunction f()\n{\n    $a = DB::table('x')\n        ->where('a', 1)\n        ->get();\n    return $a;\n}\n"
    );

    // only lines 2..3 (0-based) of the selection change
    let range = json!({ "start": { "line": 2, "character": 0 }, "end": { "line": 4, "character": 0 } });
    let edits =
        c.request("textDocument/rangeFormatting", json!({ "textDocument": doc, "range": range, "options": opts }));
    assert_eq!(
        apply(&text, &edits),
        "<?php\nfunction f() {\n    $a = DB::table('x')\n        ->where('a', 1)\n->get();\nreturn $a;\n}\n"
    );

    // Enter after `$a = 1;` inside a new block: the new line is indented one level deeper
    let typed = "<?php\nfunction g() {\n    if ($b) {\n\n    }\n}\n";
    c.change(&uri, 2, typed);
    let edits = c.request(
        "textDocument/onTypeFormatting",
        json!({ "textDocument": doc, "position": { "line": 3, "character": 0 }, "ch": "\n", "options": opts }),
    );
    assert_eq!(apply(typed, &edits), "<?php\nfunction g() {\n    if ($b) {\n        \n    }\n}\n");
}

#[test]
fn code_style_import_export_and_settings_change() {
    let mut c = Client::start(&fixture("proj"));
    let (uri, text) = c.open("src/Scratch.php", Some("<?php\n$s = 'a'.$b;\n"));
    let doc = json!({ "uri": uri });
    let opts = json!({ "tabSize": 4, "insertSpaces": true });

    // a PhpStorm scheme becomes a preset plus overrides
    let xml = r#"<code_scheme name="Team" version="173">
      <PHPCodeStyleSettings><option name="CONCAT_SPACES" value="false" /><option name="WRAP_LONG_LINES" value="true" /></PHPCodeStyleSettings>
      <codeStyleSettings language="PHP"><indentOptions><option name="INDENT_SIZE" value="2" /></indentOptions></codeStyleSettings>
    </code_scheme>"#;
    let r = c.request("phpLsp/importCodeStyle", json!({ "text": xml }));
    assert_eq!(r["name"], "Team");
    assert_eq!(r["preset"], "phpstorm");
    assert_eq!(r["options"], json!({ "INDENT_SIZE": 2, "CONCAT_SPACES": false }));
    assert_eq!(r["unsupported"], json!(["WRAP_LONG_LINES"]));

    // export of the options in effect for a file
    let r = c.request("phpLsp/exportCodeStyle", json!({ "uri": uri, "kind": "editorconfig" }));
    assert!(r["text"].as_str().unwrap().contains("ij_php_concat_spaces = true"));
    let r = c.request("phpLsp/exportCodeStyle", json!({ "kind": "phpstorm", "name": "Mine" }));
    assert!(r["text"].as_str().unwrap().starts_with("<code_scheme name=\"Mine\""));

    let edits = c.request("textDocument/formatting", json!({ "textDocument": doc, "options": opts }));
    assert_eq!(apply(&text, &edits), "<?php\n\n$s = 'a' . $b;\n");
    // changed settings apply without a restart
    c.notify(
        "workspace/didChangeConfiguration",
        json!({ "settings": { "formatOptions": { "CONCAT_SPACES": false, "BLANK_LINES_AFTER_OPENING_TAG": 0 } } }),
    );
    let edits = c.request("textDocument/formatting", json!({ "textDocument": doc, "options": opts }));
    assert_eq!(apply(&text, &edits), text);
}
