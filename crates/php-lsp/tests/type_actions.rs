//! "Add return type", "Add type" and "Add PHPDoc" code actions on `tests/fixtures/proj/src/Hints.php`
//! (the fixture's composer.json targets PHP 8.1).

mod common;

use common::{fixture, pos, Client};
use serde_json::{json, Value};

/// Code actions at `needle` (+ `delta` characters): (title, edits).
fn actions(c: &mut Client, uri: &str, text: &str, needle: &str, delta: usize) -> Vec<(String, Vec<Value>)> {
    let at = pos(text, needle, 0, delta);
    let r = c.request(
        "textDocument/codeAction",
        json!({
            "textDocument": { "uri": uri },
            "range": { "start": at, "end": at },
            "context": { "diagnostics": [] },
        }),
    );
    r.as_array()
        .into_iter()
        .flatten()
        .map(|a| {
            let edits = a["edit"]["changes"][uri].as_array().cloned().unwrap_or_default();
            (a["title"].as_str().unwrap_or("").to_string(), edits)
        })
        .collect()
}

fn offset(text: &str, p: &Value) -> usize {
    let line = p["line"].as_u64().unwrap() as usize;
    let col = p["character"].as_u64().unwrap() as usize;
    text.split_inclusive('\n').take(line).map(str::len).sum::<usize>() + col
}

/// `text` with the edits applied.
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
    // apply back to front; edits at the same position keep their order
    spans.sort_by_key(|s| std::cmp::Reverse(s.0));
    let mut out = text.to_string();
    let mut i = 0;
    while i < spans.len() {
        let mut j = i;
        while j + 1 < spans.len() && spans[j + 1].0 == spans[i].0 {
            j += 1;
        }
        let joined: String = spans[i..=j].iter().rev().map(|s| s.2.as_str()).collect();
        out.replace_range(spans[i].0..spans[j].1.max(spans[i].1), &joined);
        i = j + 1;
    }
    out
}

fn find<'a>(list: &'a [(String, Vec<Value>)], title: &str) -> &'a [Value] {
    list.iter()
        .find(|(t, _)| t == title)
        .map(|(_, e)| e.as_slice())
        .unwrap_or_else(|| panic!("no action {title:?} in {:?}", list.iter().map(|a| &a.0).collect::<Vec<_>>()))
}

#[test]
fn return_types_and_docblocks() {
    let mut c = Client::start(&fixture("proj"));
    let (uri, text) = c.open("src/Hints.php", None);

    let on_total = actions(&mut c, &uri, &text, "function total", 10);
    let fixed = apply(&text, find(&on_total, "Add return type ': int'"));
    assert!(fixed.contains("public function total($items): int\n"), "{fixed}");
    let doc = apply(&text, find(&on_total, "Add PHPDoc"));
    assert!(
        doc.contains("    /**\n     * @param mixed $items\n     * @return int\n     */\n    public function total("),
        "{doc}"
    );

    // a class from another namespace is imported
    let on_make = actions(&mut c, &uri, &text, "function make", 10);
    let fixed = apply(&text, find(&on_make, "Add return type ': Gadget'"));
    assert!(fixed.contains("public function make($limit, ...$rest): Gadget\n"), "{fixed}");
    assert!(fixed.contains("namespace App;\n\nuse App\\Sub\\Gadget;\n"), "{fixed}");

    // the docblock goes above the attribute; native types are copied as written
    let on_names = actions(&mut c, &uri, &text, "function names", 10);
    assert!(find(&on_names, "Add return type ': array'").len() == 1);
    let doc = apply(&text, find(&on_names, "Add PHPDoc"));
    assert!(
        doc.contains("    /**\n     * @param array $list\n     * @return array\n     */\n    #[Deprecated]\n"),
        "{doc}"
    );
}

#[test]
fn parameter_and_property_types() {
    let mut c = Client::start(&fixture("proj"));
    let (uri, text) = c.open("src/Hints.php", None);

    let on_limit = actions(&mut c, &uri, &text, "$limit,", 1);
    let fixed = apply(&text, find(&on_limit, "Add type 'int' to $limit"));
    assert!(fixed.contains("public function make(int $limit, ...$rest)"), "{fixed}");
    assert!(!on_limit.iter().any(|(t, _)| t == "Add PHPDoc"), "make() already has a docblock");

    let on_count = actions(&mut c, &uri, &text, "$count", 1);
    let fixed = apply(&text, find(&on_count, "Add type 'int' to $count"));
    assert!(fixed.contains("    private int $count = 0;"), "{fixed}");
    let doc = apply(&text, find(&on_count, "Add PHPDoc"));
    assert!(doc.contains("    /** @var int */\n    private $count = 0;"), "{doc}");

    // nothing to suggest inside a body
    let in_body = actions(&mut c, &uri, &text, "$sum = 0", 1);
    assert!(in_body.is_empty(), "{:?}", in_body.iter().map(|a| &a.0).collect::<Vec<_>>());
}
