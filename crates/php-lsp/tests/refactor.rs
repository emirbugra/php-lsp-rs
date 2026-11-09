//! "Move class" (`phpLsp/moveClass`) on `tests/fixtures/movecls`.

mod common;

use common::{fixture, Client};
use serde_json::{json, Value};
use std::collections::HashMap;

fn offset(text: &str, p: &Value) -> usize {
    let line = p["line"].as_u64().unwrap() as usize;
    let col = p["character"].as_u64().unwrap() as usize;
    text.split_inclusive('\n').take(line).map(str::len).sum::<usize>() + col
}

fn apply(text: &str, edits: &[Value]) -> String {
    let mut spans: Vec<(usize, usize, String)> = edits
        .iter()
        .map(|e| {
            (
                offset(text, &e["range"]["start"]),
                offset(text, &e["range"]["end"]),
                e["newText"].as_str().unwrap().into(),
            )
        })
        .collect();
    spans.sort_by_key(|s| std::cmp::Reverse((s.0, s.1)));
    let mut out = text.to_string();
    for (s, e, t) in spans {
        out.replace_range(s..e, &t);
    }
    out
}

#[test]
fn move_class_to_another_namespace_and_name() {
    let root = fixture("movecls");
    let mut c = Client::start(&root);
    let (uri, text) = c.open("src/Models/User.php", None);

    // the code action on the class name
    let at = common::pos(&text, "class User", 0, 7);
    let actions = c.request(
        "textDocument/codeAction",
        json!({ "textDocument": { "uri": uri }, "range": { "start": at, "end": at }, "context": { "diagnostics": [] } }),
    );
    let mv = actions.as_array().unwrap().iter().find(|a| a["title"] == "Move class…").expect("no Move class action");
    assert_eq!(mv["command"]["arguments"][1], "App\\Models\\User");

    let r = c.request("phpLsp/moveClass", json!({ "uri": uri, "fqn": "App\\Models\\User" }));
    assert_eq!(r["fqn"], "App\\Models\\User");

    let r = c.request(
        "phpLsp/moveClass",
        json!({ "uri": uri, "fqn": "App\\Models\\User", "newFqn": "App\\Domain\\Account\\Member" }),
    );
    let ops = r["edit"]["documentChanges"].as_array().unwrap();
    let mut after: HashMap<String, String> = HashMap::new();
    let mut rename = None;
    for op in ops {
        if op["kind"] == "rename" {
            rename = Some((op["oldUri"].as_str().unwrap().to_string(), op["newUri"].as_str().unwrap().to_string()));
            continue;
        }
        let u = op["textDocument"]["uri"].as_str().unwrap();
        let rel = u.split("/movecls/").nth(1).unwrap().to_string();
        let original = std::fs::read_to_string(root.join(&rel)).unwrap();
        after.insert(rel, apply(&original, op["edits"].as_array().unwrap()));
    }
    let (old, new) = rename.expect("file not moved");
    assert!(old.ends_with("src/Models/User.php") && new.ends_with("src/Domain/Account/Member.php"), "{old} -> {new}");

    assert_eq!(
        after["src/Models/User.php"],
        "<?php\n\nnamespace App\\Domain\\Account;\n\nuse App\\Models\\Helper;\n\nclass Member\n{\n    public function helper(): Helper\n    {\n        return new Helper();\n    }\n}\n"
    );
    assert_eq!(
        after["src/Models/Post.php"],
        "<?php\n\nnamespace App\\Models;\n\nuse App\\Domain\\Account\\Member;\n\nclass Post\n{\n    public function author(): ?Member\n    {\n        return null;\n    }\n}\n"
    );
    let ctl = &after["src/Http/Controller.php"];
    for want in [
        "use App\\Domain\\Account\\Member;",
        "/** @var Member */",
        "public function make(): Member",
        "$class = 'App\\Domain\\Account\\Member';",
        "$other = \"App\\\\Domain\\\\Account\\\\Member\";",
        "return new Member();",
        "public function full(): \\App\\Domain\\Account\\Member",
        "return new \\App\\Domain\\Account\\Member();",
    ] {
        assert!(ctl.contains(want), "{want} missing in:\n{ctl}");
    }
    let other = &after["src/Other.php"];
    assert!(other.contains("use App\\Models\\{Post};\nuse App\\Domain\\Account\\Member;"), "{other}");
    assert!(other.contains("public function run(Member $u, Post $p)"), "{other}");
    assert!(!after.contains_key("src/Models/Helper.php"));

    // bad names and taken names
    let r = c.request_raw(
        "phpLsp/moveClass",
        json!({ "uri": uri, "fqn": "App\\Models\\User", "newFqn": "App\\Models\\Helper" }),
    );
    assert!(r.response_result.is_err());
    let r = c.request_raw("phpLsp/moveClass", json!({ "uri": uri, "fqn": "App\\Models\\User", "newFqn": "App\\9x" }));
    assert!(r.response_result.is_err());
}
