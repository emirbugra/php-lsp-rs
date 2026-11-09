//! Wrong number of arguments: `missing-argument` / `too-many-arguments`.

use php_index::{analyze, index_source, inspect, Index, InspectOptions};

fn problems(src: &str) -> Vec<(String, String)> {
    let mut idx = Index::new();
    idx.update_file(index_source("file:///t.php".into(), src));
    let parse = php_syntax::parse(src);
    let analysis = analyze(&idx, src, &parse.stmts);
    inspect(&idx, "file:///t.php", src, &parse, &analysis, &InspectOptions { unresolved: true })
        .into_iter()
        .filter(|p| p.code == "missing-argument" || p.code == "too-many-arguments")
        .map(|p| (p.code.to_string(), p.span.text(src).to_string()))
        .collect()
}

fn codes(src: &str) -> Vec<String> {
    problems(src).into_iter().map(|(c, t)| format!("{c} {t}")).collect()
}

#[test]
fn functions() {
    let src = r#"<?php
function two($a, $b) {}
function opt($a, $b = 1) {}
function rest($a, ...$more) {}
function loose() { return func_get_args(); }
two(1);
two(1, 2);
two(1, 2, 3);
opt(1);
opt(1, 2, 3);
rest(1, 2, 3, 4);
rest();
loose(1, 2, 3);
two(...[1, 2]);
$f = two(...);
two(b: 2, a: 1);
two(1, b: 2);
two(b: 2);
"#;
    assert_eq!(
        codes(src),
        vec![
            "missing-argument two",
            "too-many-arguments 3",
            "too-many-arguments 3",
            "missing-argument rest",
            "missing-argument two",
        ]
    );
}

#[test]
fn methods_and_constructors() {
    let src = r#"<?php
class P {
    public function __construct(int $id, string $name = '') {}
    public function save(array $opts) {}
    public static function make($x) {}
}
class C extends P {
    public function __construct() { parent::__construct(); }
    public function run() {
        $this->save();
        $this->save([]);
        $this->save([], 2);
        self::make(1, 2);
        static::make(1);
    }
}
new P();
new P(1, 'a');
new C();
$p = new P(1);
$p->save([], 1); // inferred receiver: extra arguments are not reported
"#;
    assert_eq!(
        codes(src),
        vec![
            "missing-argument __construct",
            "missing-argument save",
            "too-many-arguments 2",
            "too-many-arguments 2",
            "missing-argument P",
        ]
    );
}

#[test]
fn messages_name_the_parameters() {
    let src = "<?php\nfunction f($a, $b, $c = 1) {}\nf();\n";
    let mut idx = Index::new();
    idx.update_file(index_source("file:///t.php".into(), src));
    let parse = php_syntax::parse(src);
    let analysis = analyze(&idx, src, &parse.stmts);
    let p = inspect(&idx, "file:///t.php", src, &parse, &analysis, &InspectOptions { unresolved: true });
    let m: Vec<_> = p.iter().filter(|p| p.code == "missing-argument").map(|p| p.message.clone()).collect();
    assert_eq!(m, vec!["Missing arguments for parameters '$a', '$b' of 'f'"]);
}

#[test]
fn virtual_and_unknown_callees_are_skipped() {
    let src = r#"<?php
/** @method static where(string $col) */
class Model {
    public function __call($n, $a) {}
}
Model::where();
Model::where('a', 'b', 'c');
unknown_fn();
$x->whatever(1, 2, 3);
"#;
    assert!(codes(src).is_empty(), "{:?}", codes(src));
}
