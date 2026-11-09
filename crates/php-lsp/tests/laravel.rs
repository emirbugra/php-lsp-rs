//! End-to-end tests of the Laravel string features on `tests/fixtures/laravel`.

mod common;

use common::{fixture, locations, pos, Client};
use serde_json::{json, Value};

fn laravel_warnings(diags: &[Value]) -> Vec<String> {
    diags
        .iter()
        .filter(|d| d["code"].as_str().is_some_and(|c| c.starts_with("laravel-unknown-")))
        .map(|d| d["message"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn completion_definition_and_hover() {
    let mut c = Client::start(&fixture("laravel"));
    let (uri, text) = c.open("app/Http/Uses.php", None);
    let td = json!({ "uri": uri });

    let r =
        c.request("textDocument/completion", json!({ "textDocument": td, "position": pos(&text, "users.show", 0, 2) }));
    let items = r.get("items").unwrap_or(&r);
    let labels: Vec<&str> = items.as_array().unwrap().iter().filter_map(|i| i["label"].as_str()).collect();
    for want in ["home", "users.show", "photos.index", "photos.show"] {
        assert!(labels.contains(&want), "{want} missing in {labels:?}");
    }
    assert!(!labels.contains(&"photos.create"), "`only` must drop photos.create: {labels:?}");

    let def = |c: &mut Client, needle: &str| {
        locations(
            &c.request("textDocument/definition", json!({ "textDocument": td, "position": pos(&text, needle, 0, 2) })),
        )
    };
    assert_eq!(def(&mut c, "users.show"), vec![("web.php".to_string(), 8)]);
    assert_eq!(def(&mut c, "app.mail.from"), vec![("app.php".to_string(), 7)]);
    let mut trans = def(&mut c, "messages.welcome");
    trans.sort();
    assert_eq!(trans, vec![("messages.php".to_string(), 4), ("messages.php".to_string(), 4)]);
    assert_eq!(def(&mut c, "users.show');"), vec![("show.blade.php".to_string(), 1)]);
    assert_eq!(def(&mut c, "APP_KEY"), vec![(".env".to_string(), 2)]);

    let h = c
        .request("textDocument/hover", json!({ "textDocument": td, "position": pos(&text, "messages.welcome", 0, 2) }));
    let value = h["contents"]["value"].as_str().unwrap_or("");
    assert!(value.contains("en: 'Welcome'") && value.contains("tr: 'Hoş geldiniz'"), "{value}");
}

#[test]
fn unknown_names_are_reported_with_a_fix() {
    let mut c = Client::start(&fixture("laravel"));
    let (uri, _) = c.open("app/Http/Uses.php", None);
    let diags = c.diagnostics(&uri);
    assert_eq!(
        laravel_warnings(&diags),
        [
            "Route 'users.shwo' not found — did you mean 'users.show'?",
            "Config key 'app.nmae' not found — did you mean 'app.name'?",
            "Translation 'messages.welcom' not found — did you mean 'messages.welcome'?",
            "View 'users.edit' not found",
        ]
    );

    let route = diags.iter().find(|d| d["code"] == "laravel-unknown-route").unwrap().clone();
    let actions = c.request(
        "textDocument/codeAction",
        json!({ "textDocument": { "uri": uri }, "range": route["range"], "context": { "diagnostics": [route] } }),
    );
    let fix = &actions[0];
    assert_eq!(fix["title"], "Change to 'users.show'");
    let edit = &fix["edit"]["changes"][uri.as_str()][0];
    assert_eq!(edit["newText"], "users.show");
}

/// Adding a route in an open routes file clears the warning in other open files.
#[test]
fn warnings_follow_route_file_edits() {
    let mut c = Client::start(&fixture("laravel"));
    let probe = "<?php\nroute('brand.new');\n";
    let (uri, _) = c.open("app/Http/Probe.php", Some(probe));
    assert_eq!(laravel_warnings(&c.diagnostics(&uri)), ["Route 'brand.new' not found"]);

    let (routes, text) = c.open("routes/web.php", None);
    c.change(&routes, 2, &format!("{text}\nRoute::get('/new', fn () => 1)->name('brand.new');\n"));
    assert!(laravel_warnings(&c.diagnostics_after_edit(&uri)).is_empty());
}

fn hover_text(c: &mut Client, uri: &str, text: &str, needle: &str, nth: usize, delta: usize) -> String {
    let r = c.request(
        "textDocument/hover",
        json!({ "textDocument": { "uri": uri }, "position": pos(text, needle, nth, delta) }),
    );
    r["contents"]["value"].as_str().unwrap_or_default().to_string()
}

#[test]
fn blade_templates() {
    let mut c = Client::start(&fixture("laravel"));
    let (uri, text) = c.open("resources/views/posts/index.blade.php", None);
    let td = json!({ "uri": uri });

    // variables passed by the controller, with their types
    assert!(hover_text(&mut c, &uri, &text, "$posts)", 0, 1).contains("App\\Models\\Post[]"));
    assert!(hover_text(&mut c, &uri, &text, "$featured->title", 0, 1).contains("App\\Models\\Post"));
    // the loop variable
    assert!(hover_text(&mut c, &uri, &text, "$post->title", 0, 1).contains("App\\Models\\Post"));

    // member completion inside {{ }}
    let r =
        c.request("textDocument/completion", json!({ "textDocument": td, "position": pos(&text, "title }}", 1, 0) }));
    let items = r.get("items").unwrap_or(&r);
    let labels: Vec<&str> = items.as_array().unwrap().iter().filter_map(|i| i["label"].as_str()).collect();
    assert!(labels.contains(&"author") && labels.contains(&"title"), "{labels:?}");

    // go to definition of a method called in the template (line numbers are 1-based)
    let def =
        c.request("textDocument/definition", json!({ "textDocument": td, "position": pos(&text, "author()", 0, 2) }));
    assert_eq!(locations(&def), vec![("Post.php".to_string(), 16)]);

    // view names in directives
    let def =
        c.request("textDocument/definition", json!({ "textDocument": td, "position": pos(&text, "users.show", 0, 2) }));
    assert_eq!(locations(&def).first().map(|l| l.0.as_str()), Some("show.blade.php"));

    // diagnostics: the real problems only (no syntax errors, no `section()` / text noise)
    let diags = c.diagnostics(&uri);
    let mut msgs: Vec<String> = diags.iter().map(|d| d["message"].as_str().unwrap().to_string()).collect();
    msgs.sort();
    assert_eq!(msgs, ["Method 'nope' not found in Post", "View 'layouts.app' not found"]);

    // inlay hint on the loop variable
    let hints = c.request(
        "textDocument/inlayHint",
        json!({ "textDocument": td, "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 50, "character": 0 } } }),
    );
    let types: Vec<&str> =
        hints.as_array().unwrap().iter().filter(|h| h["kind"] == 1).filter_map(|h| h["label"].as_str()).collect();
    assert_eq!(types, [": Post"]);

    // `compact()` / `->with()` data and component attributes
    let (uri2, text2) = c.open("resources/views/posts/show.blade.php", None);
    assert!(hover_text(&mut c, &uri2, &text2, "$post->title", 0, 1).contains("App\\Models\\Post"));
    assert!(hover_text(&mut c, &uri2, &text2, "$related", 0, 1).contains("App\\Models\\Post"));

    // references from PHP code reach templates that are not open
    let (puri, ptext) = c.open("app/Models/Post.php", None);
    let refs = c.request(
        "textDocument/references",
        json!({ "textDocument": { "uri": puri }, "position": pos(&ptext, "function author", 0, 10), "context": { "includeDeclaration": false } }),
    );
    let mut files: Vec<String> = locations(&refs).into_iter().map(|l| l.0).collect();
    files.sort();
    assert_eq!(files, ["index.blade.php", "show.blade.php"]);

    // templates are never formatted (their document text is the generated code)
    let fmt = c.request(
        "textDocument/formatting",
        json!({ "textDocument": td, "options": { "tabSize": 4, "insertSpaces": true } }),
    );
    assert!(fmt.is_null());
}
