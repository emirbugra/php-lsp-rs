//! End-to-end tests on `tests/fixtures/proj` (a tiny PSR-4 project, namespace `App\`).

mod common;

use common::{fixture, locations, pos, Client};
use serde_json::{json, Value};

fn client() -> Client {
    Client::start(&fixture("proj"))
}

fn names(items: &Value) -> Vec<String> {
    items.as_array().into_iter().flatten().filter_map(|i| i["name"].as_str().map(str::to_string)).collect()
}

/// First `$name` in `s`: (byte offset, name).
fn first_var(s: &str) -> Option<(usize, &str)> {
    let at = s.find('$')?;
    let len = s[at + 1..].find(|c: char| !(c.is_alphanumeric() || c == '_')).unwrap_or(s.len() - at - 1);
    Some((at, &s[at..at + 1 + len]))
}

/// Every `//@ type` comment in Types.php states the type hover must show for the variable of
/// that line (inside `{ … }` when the line has a block, otherwise the first variable).
#[test]
fn hover_shows_inferred_types() {
    let mut c = client();
    let (uri, text) = c.open("src/Types.php", None);
    let mut failures = Vec::new();
    let mut checked = 0;
    for (ln, line) in text.lines().enumerate() {
        let Some(mark) = line.find("//@") else { continue };
        let expected = line[mark + 3..].trim();
        if expected.is_empty() {
            continue;
        }
        let code = &line[..mark];
        let col = match code.find('{') {
            Some(b) if code[b + 1..].contains('$') => b + 1 + first_var(&code[b + 1..]).unwrap().0,
            _ => first_var(code).expect("no variable on a //@ line").0,
        };
        let h = c.request(
            "textDocument/hover",
            json!({ "textDocument": { "uri": uri }, "position": { "line": ln, "character": col + 1 } }),
        );
        let value = h["contents"]["value"].as_str().unwrap_or("");
        let got = value
            .lines()
            .find_map(|l| {
                first_var(l).filter(|(at, _)| *at == 0).and_then(|_| l.split_once(": ")).map(|(_, t)| t.trim())
            })
            .unwrap_or("");
        checked += 1;
        if got != expected {
            failures.push(format!("line {}: {}  expected [{expected}] got [{got}]", ln + 1, code.trim()));
        }
    }
    assert!(checked >= 28, "only {checked} //@ checks found");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn member_completion_follows_types() {
    let mut c = client();
    c.open("src/Types.php", None);
    let (uri, text) = c.open("src/Comp.php", None);
    let expected: &[(u32, &[&str])] = &[
        (1, &["posts", "members"]),
        (2, &["posts", "team", "name"]),
        (3, &["posts"]),
        (4, &["posts"]),
        (5, &["posts"]),
        (6, &["posts"]),
        (7, &["posts"]),
        (8, &["posts"]),
        (9, &["members"]),
        (10, &["posts"]),
        (11, &["posts"]),
        (12, &["members"]),
        (13, &["members", "posts"]),
    ];
    for (n, want) in expected {
        let r = c.request(
            "textDocument/completion",
            json!({ "textDocument": { "uri": uri }, "position": pos(&text, &format!("/*{n}*/"), 0, 0) }),
        );
        let items = r.get("items").unwrap_or(&r);
        let got: Vec<&str> = items.as_array().unwrap().iter().filter_map(|i| i["label"].as_str()).collect();
        for w in *want {
            assert!(got.contains(w), "/*{n}*/: '{w}' missing in {got:?}");
        }
    }
}

#[test]
fn arrow_completion_hides_static_properties() {
    let mut c = client();
    let src =
        "<?php\nnamespace App;\nclass St { public static $shared; public $own; public static function make() {} }\n\
               function f(St $s) {\n    $s->;\n    St::;\n}\n";
    let (uri, text) = c.open("src/__static_probe.php", Some(src));
    let labels = |c: &mut Client, needle: &str| -> Vec<String> {
        let r = c.request(
            "textDocument/completion",
            json!({ "textDocument": { "uri": uri }, "position": pos(&text, needle, 0, needle.len()) }),
        );
        let items = r.get("items").unwrap_or(&r).as_array().cloned().unwrap_or_default();
        items.iter().filter_map(|i| i["label"].as_str().map(str::to_string)).collect()
    };
    let arrow = labels(&mut c, "$s->");
    assert!(arrow.iter().any(|l| l == "own") && !arrow.iter().any(|l| l.contains("shared")), "{arrow:?}");
    let colons = labels(&mut c, "St::");
    assert!(colons.iter().any(|l| l.contains("shared")) && !colons.iter().any(|l| l == "own"), "{colons:?}");
}

#[test]
fn implementation_and_type_hierarchy() {
    let mut c = client();
    let (uri, text) = c.open("src/Impl.php", None);
    let td = json!({ "uri": uri });
    let r =
        c.request("textDocument/implementation", json!({ "textDocument": td, "position": pos(&text, "/*1*/", 0, 5) }));
    let mut got = locations(&r);
    got.sort();
    assert_eq!(got, vec![("Impl.php".into(), 5), ("Impl.php".into(), 6), ("Impl.php".into(), 7)]);
    let r =
        c.request("textDocument/implementation", json!({ "textDocument": td, "position": pos(&text, "/*2*/", 0, 5) }));
    assert_eq!(locations(&r), vec![("Impl.php".into(), 7)]);

    let items = c.request(
        "textDocument/prepareTypeHierarchy",
        json!({ "textDocument": td, "position": pos(&text, "class Base", 0, 7) }),
    );
    assert_eq!(names(&items), ["Base"]);
    let base = items[0].clone();
    let subs = c.request("typeHierarchy/subtypes", json!({ "item": base }));
    assert_eq!(names(&subs), ["Circle", "Square"]);
    assert_eq!(names(&c.request("typeHierarchy/supertypes", json!({ "item": base }))), ["Shape"]);
    let square = subs.as_array().unwrap().iter().find(|i| i["name"] == "Square").unwrap().clone();
    assert_eq!(names(&c.request("typeHierarchy/subtypes", json!({ "item": square }))), ["Cube"]);
}

#[test]
fn type_definition() {
    let mut c = client();
    c.open("src/DocVar.php", None);
    let (uri, text) = c.open("src/Calls.php", None);
    let cases: &[(&str, usize, Option<u64>)] = &[
        ("$item?->", 2, Some(10)),
        ("$repo->find", 2, Some(4)),
        ("$this->load", 2, Some(4)),
        ("find($id)", 1, Some(10)),
        ("title()", 1, None),
        ("$list", 2, Some(10)),
        ("CRepo $repo;", 8, Some(4)),
        ("$this->load", 1, Some(4)),
        ("$this->repo", 8, Some(4)),
        ("$r->find", 1, Some(4)),
    ];
    for (needle, delta, line) in cases {
        let r = c.request(
            "textDocument/typeDefinition",
            json!({ "textDocument": { "uri": uri }, "position": pos(&text, needle, 0, *delta) }),
        );
        let want: Vec<(String, u64)> = line.iter().map(|l| ("Calls.php".to_string(), *l)).collect();
        assert_eq!(locations(&r), want, "{needle}");
    }
}

/// `(name, detail, 1-based lines)` of incoming / outgoing call entries.
fn calls(v: &Value, side: &str) -> Vec<(String, String, Vec<u64>)> {
    v.as_array()
        .into_iter()
        .flatten()
        .map(|x| {
            let item = &x[side];
            let lines =
                x["fromRanges"].as_array().unwrap().iter().map(|r| r["start"]["line"].as_u64().unwrap() + 1).collect();
            (item["name"].as_str().unwrap().into(), item["detail"].as_str().unwrap_or("").into(), lines)
        })
        .collect()
}

fn call(name: &str, detail: &str, lines: &[u64]) -> (String, String, Vec<u64>) {
    (name.into(), detail.into(), lines.to_vec())
}

#[test]
fn call_hierarchy() {
    let mut c = client();
    c.open("src/DocVar.php", None);
    let (uri, text) = c.open("src/Calls.php", None);
    let prepare = |c: &mut Client, needle: &str, delta: usize| -> Value {
        let items = c.request(
            "textDocument/prepareCallHierarchy",
            json!({ "textDocument": { "uri": uri }, "position": pos(&text, needle, 0, delta) }),
        );
        items[0].clone()
    };
    let find = prepare(&mut c, "function find", 9);
    assert_eq!(
        calls(&c.request("callHierarchy/incomingCalls", json!({ "item": find })), "from"),
        vec![
            call("show", "App\\CService", &[15, 16]),
            call("again", "App\\CService", &[20]),
            call("chelper", "App\\chelper", &[23])
        ]
    );
    assert_eq!(
        calls(&c.request("callHierarchy/outgoingCalls", json!({ "item": find })), "to"),
        vec![call("load", "App\\CRepo", &[6])]
    );

    let ctor = prepare(&mut c, "function __construct", 9);
    assert_eq!(
        calls(&c.request("callHierarchy/incomingCalls", json!({ "item": ctor })), "from"),
        vec![
            call("show", "App\\CService", &[15]),
            call("again", "App\\CService", &[20]),
            call("Calls.php", "(top level)", &[25]),
            call("dv", "App\\dv", &[8]),
        ]
    );

    let helper = prepare(&mut c, "function chelper", 10);
    assert_eq!(
        calls(&c.request("callHierarchy/incomingCalls", json!({ "item": helper })), "from"),
        vec![call("show", "App\\CService", &[18]), call("Calls.php", "(top level)", &[25])]
    );
    assert_eq!(
        calls(&c.request("callHierarchy/outgoingCalls", json!({ "item": helper })), "to"),
        vec![call("find", "App\\CRepo", &[23])]
    );

    let show = prepare(&mut c, "function show", 9);
    assert_eq!(
        calls(&c.request("callHierarchy/outgoingCalls", json!({ "item": show })), "to"),
        vec![
            call("__construct", "App\\CRepo", &[15]),
            call("find", "App\\CRepo", &[15, 16]),
            call("title", "App\\CItem", &[18]),
            call("chelper", "App\\chelper", &[18]),
        ]
    );
}

/// Class names in `@param` / `@return` / `@var` / `@throws` … tags go to their declaration.
#[test]
fn docblock_class_names_are_navigable() {
    let mut c = client();
    c.open("src/Calls.php", None);
    let (uri, text) = c.open("src/DocRefs.php", None);
    // (needle, occurrence, offset into the needle, file, line)
    let cases: &[(&str, usize, usize, &str, u64)] = &[
        ("@property CItem", 0, 11, "Calls.php", 10),
        ("\\App\\CItem[]", 0, 6, "Calls.php", 10),
        ("static CRepo", 0, 8, "Calls.php", 4),
        ("findIt(CItem", 0, 8, "Calls.php", 10),
        ("a: CService", 0, 4, "Calls.php", 12),
        ("@mixin CService", 0, 8, "Calls.php", 12),
        ("int, CItem>", 0, 6, "Calls.php", 10),
        ("@param R ", 0, 7, "Calls.php", 4),
        ("|CService", 0, 2, "Calls.php", 12),
        ("\\RuntimeException", 0, 2, "SPL.php", 59),
        ("@see CRepo", 0, 6, "Calls.php", 4),
        ("@var CItem $x", 0, 6, "Calls.php", 10),
    ];
    for (needle, nth, delta, file, line) in cases {
        let r = c.request(
            "textDocument/definition",
            json!({ "textDocument": { "uri": uri }, "position": pos(&text, needle, *nth, *delta) }),
        );
        assert_eq!(locations(&r), vec![(file.to_string(), *line)], "{needle}");
    }
    let undefined: Vec<Value> = c.diagnostics(&uri).into_iter().filter(|d| d["code"] == "undefined-class").collect();
    assert!(undefined.is_empty(), "{undefined:?}");
}

#[test]
fn doc_var_types_a_variable() {
    let mut c = client();
    let (uri, text) = c.open("src/DocVar.php", None);
    let h = c
        .request("textDocument/hover", json!({ "textDocument": { "uri": uri }, "position": pos(&text, "$a->", 0, 1) }));
    assert!(h["contents"]["value"].as_str().unwrap_or("").contains("$a: App\\CRepo"), "{h}");
}

#[test]
fn inlay_hints_name_parameters() {
    let mut c = client();
    let (uri, _) = c.open("src/Inlay.php", None);
    let r = c.request(
        "textDocument/inlayHint",
        json!({ "textDocument": { "uri": uri }, "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 100, "character": 0 } } }),
    );
    let got: Vec<(u64, u64, String)> = r
        .as_array()
        .unwrap()
        .iter()
        .map(|h| {
            let p = &h["position"];
            (
                p["line"].as_u64().unwrap() + 1,
                p["character"].as_u64().unwrap(),
                h["label"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    let want: Vec<(u64, u64, String)> = [
        (13, 16, "host:"),
        (13, 24, "port:"),
        (14, 9, "to:"),
        (14, 18, "subject:"),
        (14, 24, "...extra:"),
        (16, 6, "name:"),
        (16, 13, "times:"),
        (18, 13, "times:"),
        (19, 13, "haystack:"),
        (19, 20, "needle:"),
    ]
    .into_iter()
    .map(|(l, c, s)| (l, c, s.to_string()))
    .collect();
    assert_eq!(got, want);
}

#[test]
fn inlay_hints_show_variable_types() {
    let mut c = client();
    let (uri, _) = c.open("src/InlayTypes.php", None);
    let r = c.request(
        "textDocument/inlayHint",
        json!({ "textDocument": { "uri": uri }, "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 100, "character": 0 } } }),
    );
    let got: Vec<(u64, u64, String)> = r
        .as_array()
        .unwrap()
        .iter()
        .filter(|h| h["kind"] == 1)
        .map(|h| {
            let p = &h["position"];
            (
                p["line"].as_u64().unwrap() + 1,
                p["character"].as_u64().unwrap(),
                h["label"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    let want: Vec<(u64, u64, String)> = [
        (13, 11, ": Author"),
        (15, 10, ": Post[]"),
        (16, 28, ": Post"),
        (21, 19, ": Post"),
        (22, 9, ": Author"),
        (23, 9, ": array"),
        (24, 10, ": string"),
        (26, 7, ": bool"),
    ]
    .into_iter()
    .map(|(l, c, s)| (l, c, s.to_string()))
    .collect();
    assert_eq!(got, want);

    // hover on a read shows the scalar type too (literals have no inlay hint, but a hover type)
    let (_, text) = c.open("src/InlayTypes.php", None);
    for (needle, want) in [("$data, $count", "$data: array"), ("$count]", "$count: int")] {
        let h = c.request(
            "textDocument/hover",
            json!({ "textDocument": { "uri": uri }, "position": common::pos(&text, needle, 0, 2) }),
        );
        let md = h["contents"]["value"].as_str().unwrap_or("");
        assert!(md.contains(want), "{needle}: {md}");
    }
}

#[test]
fn code_lenses_count_references_and_implementations() {
    let mut c = client();
    let (uri, _) = c.open("src/Impl.php", None);
    let lenses = c.request("textDocument/codeLens", json!({ "textDocument": { "uri": uri } }));
    let mut got: Vec<(u64, u64, String)> = Vec::new();
    for lens in lenses.as_array().unwrap() {
        let lens = if lens["command"].is_null() { c.request("codeLens/resolve", lens.clone()) } else { lens.clone() };
        let start = &lens["range"]["start"];
        got.push((
            start["line"].as_u64().unwrap() + 1,
            start["character"].as_u64().unwrap(),
            lens["command"]["title"].as_str().unwrap().to_string(),
        ));
    }
    got.sort();
    let want: Vec<(u64, u64, String)> = [
        (4, 10, "2 references"),
        (4, 10, "4 implementations"),
        (4, 34, "1 reference"),
        (4, 34, "3 implementations"),
        (5, 15, "3 references"),
        (5, 15, "3 subclasses"),
        (5, 64, "0 references"),
        (5, 64, "2 implementations"),
        (5, 95, "1 override"),
        (5, 95, "1 reference"),
        (6, 6, "0 references"),
        (6, 44, "0 references"),
        (7, 6, "1 reference"),
        (7, 6, "1 subclass"),
        (7, 44, "0 references"),
        (7, 90, "0 references"),
        (8, 6, "0 references"),
        (10, 9, "0 references"),
    ]
    .into_iter()
    .map(|(l, c, s)| (l, c, s.to_string()))
    .collect();
    assert_eq!(got, want);
}

/// Every line marked `//@ BAD` (or `//@ BAD x2`) in Undef.php has that many undefined variables,
/// and no other line has any.
#[test]
fn undefined_variables() {
    let mut c = client();
    let (uri, text) = c.open("src/Undef.php", None);
    let mut want: Vec<u64> = Vec::new();
    for (ln, line) in text.lines().enumerate() {
        if let Some(m) = line.find("//@ BAD") {
            let n = if line[m..].contains("x2") { 2 } else { 1 };
            want.extend(std::iter::repeat_n(ln as u64 + 1, n));
        }
    }
    let mut got: Vec<u64> = c
        .diagnostics(&uri)
        .iter()
        .filter(|d| d["code"] == "undefined-variable")
        .map(|d| d["range"]["start"]["line"].as_u64().unwrap() + 1)
        .collect();
    got.sort();
    assert_eq!(got, want);
}

#[test]
fn unknown_request_is_method_not_found() {
    let mut c = client();
    let r = c.request_raw("php/noSuchMethod", json!({}));
    assert_eq!(r.response_result.err().map(|e| e.code), Some(lsp_server::ErrorCode::MethodNotFound as i32));
}
