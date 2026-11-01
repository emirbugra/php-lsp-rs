//! Undefined-variable inspection and sub-type queries.

use php_index::{analyze, index_source, inspect, Index, InspectOptions};

fn build(src: &str) -> Index {
    let mut idx = Index::new();
    idx.update_file(index_source("file:///t.php".into(), src));
    idx
}

fn undefined(src: &str) -> Vec<String> {
    let idx = build(src);
    let parse = php_syntax::parse(src);
    let analysis = analyze(&idx, src, &parse.stmts);
    inspect(&idx, "file:///t.php", src, &parse, &analysis, &InspectOptions { unresolved: true })
        .into_iter()
        .filter(|p| p.code == "undefined-variable")
        .map(|p| p.args[0].clone())
        .collect()
}

#[test]
fn reports_only_real_misses() {
    let src = r#"<?php
class U {
    public function f($items) {
        foreach ($items as $k => [$a, $b]) { echo $k, $a, $b; }
        $arr[] = 1; $map['x']['y'] = 2;
        [$p, [$q]] = $items;
        preg_match('/a/', 'a', $m);
        try {} catch (\Exception $e) { echo $e; }
        global $g; static $s = 0;
        $fn = function ($z) use (&$out) { $out = $z; };
        $arrow = fn($n) => $n + $items;
        if (isset($nope) || empty($nope2)) {}
        $d = $maybe ?? 1; $i++;
        echo compact('items'), $_GET['x'], $this, $arr, $map, $p, $q, $m, $g, $s, $out, $fn, $arrow, $d;
        return $missing;
    }
    public function dyn() { extract([]); return $a; }
    public function g() { return fn() => $other; }
}
"#;
    assert_eq!(undefined(src), vec!["missing", "other"]);
}

#[test]
fn by_ref_callee_and_unknown_callee() {
    let src = r#"<?php
function fill(&$out) { $out = 1; }
function f($cb) { fill($x); $cb($y); return $x . $y . $z; }
"#;
    assert_eq!(undefined(src), vec!["z"]);
}

#[test]
fn sub_types() {
    let idx = build(
        "<?php interface S {} abstract class B implements S {} class C extends B {} class D extends C {} trait T {} class E { use T; }",
    );
    let names = |v: Vec<php_index::SymLoc>| {
        let mut n: Vec<String> = v.into_iter().map(|l| idx.sym(l).name.to_string()).collect();
        n.sort();
        n
    };
    assert_eq!(names(idx.subtypes("S")), vec!["B"]);
    assert_eq!(names(idx.descendants("S")), vec!["B", "C", "D"]);
    assert_eq!(names(idx.subtypes("T")), vec!["E"]);
}

#[test]
fn doc_property_without_dollar_points_at_tag() {
    let src = "<?php\n/**\n * @property Foo|null last_balance\n * @property $id\n * @method hasIt\n */\nclass R {}\nclass Foo {}\n";
    let idx = build(src);
    for (name, line) in [("last_balance", 2), ("id", 3), ("hasIt", 4)] {
        let loc = idx
            .find_member("R", name, php_index::MemberKind::Property)
            .or_else(|| idx.find_member("R", name, php_index::MemberKind::Method))
            .expect(name);
        let at = idx.sym(loc).name_span.start as usize;
        assert_eq!(src[..at].matches('\n').count(), line, "{name}");
    }
}

#[test]
fn class_names_in_docblocks_are_references() {
    let src = r#"<?php
namespace App;
class Item {}
class Repo {}
/**
 * @property Item|null $item
 * @method static Repo|null findIt(Item $x)
 * @template T of Item
 */
class Holder {
    /** @var array<int, Item> */
    public $list;
    /**
     * @param list<Item> $a
     * @param T $t
     * @return non-empty-string|Repo
     */
    public function m($a, $t) {
        /** @var Item $x */
        $x = null;
    }
}
"#;
    let idx = build(src);
    let parse = php_syntax::parse(src);
    let analysis = analyze(&idx, src, &parse.stmts);
    let mut found: Vec<(usize, String)> = analysis
        .refs
        .iter()
        .filter_map(|r| match &r.target {
            php_index::RefTarget::Class(f)
                if src[..r.span.start as usize].rfind("/**") > src[..r.span.start as usize].rfind("*/") =>
            {
                Some((src[..r.span.start as usize].matches('\n').count() + 1, format!("{}={}", r.span.text(src), f)))
            }
            _ => None,
        })
        .collect();
    found.sort();
    let want: Vec<(usize, String)> = [
        (6, "Item=App\\Item"),
        (7, "Repo=App\\Repo"),
        (7, "Item=App\\Item"),
        (8, "Item=App\\Item"),
        (11, "Item=App\\Item"),
        (14, "Item=App\\Item"),
        (16, "Repo=App\\Repo"),
        (19, "Item=App\\Item"),
    ]
    .iter()
    .map(|(l, s)| (*l, s.to_string()))
    .collect();
    let mut want = want;
    want.sort();
    assert_eq!(found, want);
    // doc-only mentions make the file a find-references candidate
    assert!(idx.file_by_uri("file:///t.php").unwrap().has_name(php_index::name_hash("Item")));
}

#[test]
fn doc_var_declares_at_the_comment() {
    let src =
        "<?php\nfunction f() {\n    /** @var \\Exception $e */\n\n    echo 1;\n    $e = g();\n    return $e;\n}\n";
    let idx = build(src);
    let parse = php_syntax::parse(src);
    let analysis = analyze(&idx, src, &parse.stmts);
    let decls: Vec<&str> = analysis
        .refs
        .iter()
        .filter_map(|r| match &r.target {
            php_index::RefTarget::Variable { name, decl: Some(d), .. } if name == "e" => Some(d.text(src)),
            _ => None,
        })
        .collect();
    assert_eq!(decls.len(), 3, "comment + assignment + read");
    assert!(decls.iter().all(|d| *d == "$e"));
    let first = analysis
        .refs
        .iter()
        .find(|r| matches!(&r.target, php_index::RefTarget::Variable { name, .. } if name == "e"))
        .unwrap();
    assert_eq!(src[..first.span.start as usize].matches('\n').count(), 2, "declared on the @var line");
}
