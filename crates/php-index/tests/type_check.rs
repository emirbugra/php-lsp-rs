//! Values that surely don't fit a native type: `type-mismatch`.

use php_index::{analyze, index_source, inspect, Index, InspectOptions};

fn mismatches(src: &str) -> Vec<String> {
    let mut idx = Index::new();
    idx.update_file(index_source("file:///t.php".into(), src));
    let parse = php_syntax::parse(src);
    let analysis = analyze(&idx, src, &parse.stmts);
    inspect(&idx, "file:///t.php", src, &parse, &analysis, &InspectOptions { unresolved: true })
        .into_iter()
        .filter(|p| p.code == "type-mismatch")
        .map(|p| format!("{} | {}", p.span.text(src), p.message))
        .collect()
}

const CLASSES: &str = r#"
interface Shape {}
class Circle implements Shape {}
class Dog {}
class Named { public function __toString(): string { return ''; } }
"#;

#[test]
fn arguments_loose_mode() {
    let src = format!(
        r#"<?php
{CLASSES}
function num(int $n) {{}}
function name(string $s) {{}}
function shape(Shape $s) {{}}
function maybe(?Shape $s) {{}}
function opt(Shape $s = null) {{}}
function list_(array $a) {{}}
function any($x) {{}}
function union(int|string $x) {{}}
num('5');
num(1.5);
num(true);
num(null);
num([]);
num(new Dog);
name(12);
name(new Named);
name(new Dog);
shape(new Circle);
shape(new Dog);
shape('circle');
maybe(null);
opt(null);
list_('a');
list_([1]);
any(null);
union(1.5);
union([]);
function circle(Circle $c) {{}}
function pass(Dog $d, $loose) {{
    shape($d);
    circle($d);
    $loose = new Dog;
    circle($loose);
}}
num(s: 1);
"#
    );
    assert_eq!(
        mismatches(&src),
        vec![
            "null | Argument #1 ($n) of 'num' must be of type int, null given",
            "[] | Argument #1 ($n) of 'num' must be of type int, array given",
            "new Dog | Argument #1 ($n) of 'num' must be of type int, Dog given",
            "new Dog | Argument #1 ($s) of 'name' must be of type string, Dog given",
            "new Dog | Argument #1 ($s) of 'shape' must be of type Shape, Dog given",
            "'circle' | Argument #1 ($s) of 'shape' must be of type Shape, string given",
            "'a' | Argument #1 ($a) of 'list_' must be of type array, string given",
            "[] | Argument #1 ($x) of 'union' must be of type int|string, array given",
            // `$d` is a `Dog` parameter: a subclass of it may implement `Shape`, but can't be a `Circle`
            "$d | Argument #1 ($c) of 'circle' must be of type Circle, Dog given",
        ]
    );
}

#[test]
fn arguments_strict_mode() {
    let src = r#"<?php
declare(strict_types=1);
function num(int $n) {}
function real(float $f) {}
function name(string $s) {}
num('5');
real(1);
name(12);
num(1.5);
"#;
    assert_eq!(
        mismatches(src),
        vec![
            "'5' | Argument #1 ($n) of 'num' must be of type int, string given",
            "12 | Argument #1 ($s) of 'name' must be of type string, int given",
            "1.5 | Argument #1 ($n) of 'num' must be of type int, float given",
        ]
    );
}

#[test]
fn methods_and_constructors() {
    let src = format!(
        r#"<?php
{CLASSES}
class Box {{
    public function __construct(Shape $s) {{}}
    public function put(int $n): void {{}}
    public static function make(array $a): static {{ return new static; }}
}}
new Box(new Dog);
new Box(new Circle);
Box::make('x');
$b = new Box(new Circle);
$b->put([]);
"#
    );
    assert_eq!(
        mismatches(&src),
        vec![
            "new Dog | Argument #1 ($s) of '__construct' must be of type Shape, Dog given",
            "'x' | Argument #1 ($a) of 'make' must be of type array, string given",
            "[] | Argument #1 ($n) of 'put' must be of type int, array given",
        ]
    );
}

#[test]
fn return_values() {
    let src = format!(
        r#"<?php
{CLASSES}
function a(): Shape {{ return new Dog; }}
function b(): ?Shape {{ return null; }}
function c(): Shape {{ return null; }}
function d(): int {{ return 'x'; }}
function e(): array {{ return 'x'; }}
function f(): iterable {{ yield 1; }}
function g() {{ return 1; }}
function h(): Shape {{
    $f = function (): int {{ return []; }};
    return new Circle;
}}
$i = fn(): string => [];
"#
    );
    assert_eq!(
        mismatches(&src),
        vec![
            "new Dog | Return value must be of type Shape, Dog returned",
            "null | Return value must be of type Shape, null returned",
            "'x' | Return value must be of type array, string returned",
            "[] | Return value must be of type int, array returned",
            "[] | Return value must be of type string, array returned",
        ]
    );
}

#[test]
fn version_guards() {
    let src = r#"<?php
function num(int $n) {}
if (class_exists(Foo::class)) {
    num('x', []);
    num([]);
} else {
    num([]);
}
$v = function_exists('x') ? num([]) : 1;
num([]);
"#;
    assert_eq!(mismatches(src), vec!["[] | Argument #1 ($n) of 'num' must be of type int, array given"]);
}
