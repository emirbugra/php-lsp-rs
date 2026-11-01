//! Type suggestions for "add return type / add type / add PHPDoc".

use php_index::{analyze, index_source, Index, PhpVersion, Place, TypeSuggester};
use php_syntax::walk::{walk_class, walk_function, walk_stmts, Visitor};
use php_syntax::{ClassDecl, FunctionDecl, Member, PropertyDecl};

const URI: &str = "file:///t.php";

fn short(fqn: &str) -> String {
    fqn.rsplit('\\').next().unwrap_or(fqn).to_string()
}

/// Finds the function / method named `name` (in class `class`, when given) and every property.
struct Find<'a> {
    class: Option<&'a str>,
    name: &'a str,
    current: Vec<String>,
    func: Option<&'a FunctionDecl>,
    props: Vec<(String, &'a PropertyDecl)>,
}

impl<'a> Visitor<'a> for Find<'a> {
    fn visit_class(&mut self, c: &'a ClassDecl) {
        let name = c.name.as_ref().map(|n| n.text.clone()).unwrap_or_default();
        for m in &c.members {
            if let Member::Property(p) = m {
                self.props.push((format!("{name}::{}", p.name.text), p));
            }
        }
        self.current.push(name);
        walk_class(self, c);
        self.current.pop();
    }
    fn visit_function(&mut self, f: &'a FunctionDecl) {
        let in_class = self.current.last().map(|s| s.as_str());
        if f.name.as_ref().is_some_and(|n| n.text == self.name) && in_class == self.class {
            self.func = Some(f);
        }
        walk_function(self, f);
    }
}

struct Fixture {
    index: Index,
    src: String,
    parse: php_syntax::Parse,
}

impl Fixture {
    fn new(src: &str) -> Fixture {
        let mut index = Index::new();
        index.update_file(index_source(URI.to_string(), src));
        Fixture { index, src: src.to_string(), parse: php_syntax::parse(src) }
    }

    fn with<R>(&self, f: impl FnOnce(&TypeSuggester, &Find) -> R, class: Option<&str>, name: &str) -> R {
        let analysis = analyze(&self.index, &self.src, &self.parse.stmts);
        let sg = TypeSuggester::new(&self.index, &analysis, &self.src, URI);
        let mut find = Find { class, name, current: Vec::new(), func: None, props: Vec::new() };
        walk_stmts(&mut find, &self.parse.stmts);
        f(&sg, &find)
    }

    /// (native type for `version`, doc type) of the return type of `class::name`.
    fn ret(&self, class: Option<&str>, name: &str, version: PhpVersion) -> Option<(Option<String>, String)> {
        self.with(
            |sg, find| {
                let t = sg.return_type(find.func.expect("function not found"))?;
                Some((t.native(version, Place::Return, &short), t.doc(&short)))
            },
            class,
            name,
        )
    }

    fn ret8(&self, name: &str) -> Option<String> {
        self.ret(None, name, PhpVersion(8, 3)).and_then(|t| t.0)
    }

    fn method8(&self, class: &str, name: &str) -> Option<String> {
        self.ret(Some(class), name, PhpVersion(8, 3)).and_then(|t| t.0)
    }

    fn param8(&self, class: Option<&str>, name: &str, i: usize) -> Option<String> {
        self.with(
            |sg, find| {
                sg.param_type(find.func.expect("function not found"), i)?.native(PhpVersion(8, 3), Place::Param, &short)
            },
            class,
            name,
        )
    }

    fn prop8(&self, key: &str) -> Option<(String, bool)> {
        self.with(
            |sg, find| {
                let p = find.props.iter().find(|(k, _)| k == key).expect("property not found").1;
                let t = sg.property_type(p)?;
                Some((t.ty.native(PhpVersion(8, 3), Place::Property, &short)?, t.add_null_default))
            },
            None,
            "",
        )
    }
}

#[test]
fn scalar_returns() {
    let f = Fixture::new(
        r#"<?php
function lit() { return 1; }
function str($a, $b) { return $a . $b; }
function cmp($a) { return $a > 1; }
function num(): int { return 1; }
function call() { return num(); }
function mixed($x) { if ($x) { return 1; } else { return 'a'; } }
function cast($x) { return (int) $x; }
function flt() { return 1.5 * 2; }
function interp($x) { return "a $x"; }
function cnt() { $t = 0; foreach ([1] as $v) { $t += 1.5; } return $t; }
function arr() { $r = []; $r[] = 1; return $r; }
function elem() { $r = null; $r[] = 1; return $r; }
function coalesce(?string $x) { return $x ?? 'd'; }
function cst() { return PHP_X; }
const PHP_X = 'x';
"#,
    );
    assert_eq!(f.ret8("lit").as_deref(), Some("int"));
    assert_eq!(f.ret8("str").as_deref(), Some("string"));
    assert_eq!(f.ret8("cmp").as_deref(), Some("bool"));
    assert_eq!(f.ret8("call").as_deref(), Some("int"));
    assert_eq!(f.ret8("mixed").as_deref(), Some("int|string"));
    assert_eq!(f.ret(None, "mixed", PhpVersion(7, 4)).map(|t| t.0), Some(None), "unions need PHP 8");
    assert_eq!(f.ret8("cast").as_deref(), Some("int"));
    assert_eq!(f.ret8("flt").as_deref(), Some("float"));
    assert_eq!(f.ret8("interp").as_deref(), Some("string"));
    assert_eq!(f.ret8("cnt").as_deref(), Some("int|float"));
    assert_eq!(f.ret8("arr").as_deref(), Some("array"));
    assert_eq!(f.ret8("elem").as_deref(), Some("?array"));
    assert_eq!(f.ret8("coalesce").as_deref(), Some("string"));
    assert_eq!(f.ret8("cst").as_deref(), Some("string"));
}

#[test]
fn unsafe_returns_are_not_suggested() {
    let f = Fixture::new(
        r#"<?php
function bare($x) { if ($x) { return; } return 1; }
function falls($x) { if ($x) { return 1; } }
function each($a) { foreach ($a as $v) {} return $v; }
function untyped($x) { return $x; }
function unknown() { return foo(); }
function dyn($a) { extract($a); $x = 1; return $x; }
function byref() { $x = 1; $f = function () use (&$x) { $x = 'a'; }; return $x; }
"#,
    );
    for name in ["bare", "falls", "each", "untyped", "unknown", "dyn", "byref"] {
        assert_eq!(f.ret8(name), None, "{name}");
    }
}

#[test]
fn void_generator_and_objects() {
    let f = Fixture::new(
        r#"<?php
namespace App;
class User {
    public function me() { return $this; }
    public function make() { return new User(); }
    public function maybe($x) { $u = null; if ($x) { $u = new User(); } return $u; }
    public function arrow() { return fn($x) => $x * 2; }
}
function v() { echo 1; }
function gen() { yield 1; }
function thrower() { throw new \Exception('x'); }
function choose($x) { if ($x) { return new User(); } throw new \Exception('no'); }
"#,
    );
    assert_eq!(f.ret8("v").as_deref(), Some("void"));
    assert_eq!(f.ret8("gen").as_deref(), Some("Generator"));
    assert_eq!(f.ret8("thrower").as_deref(), Some("void"));
    assert_eq!(f.ret8("choose").as_deref(), Some("User"));
    assert_eq!(f.method8("User", "me").as_deref(), Some("static"));
    assert_eq!(f.ret(Some("User"), "me", PhpVersion(7, 4)).map(|t| t.0), Some(None), "static needs PHP 8");
    assert_eq!(f.method8("User", "make").as_deref(), Some("User"));
    assert_eq!(f.method8("User", "maybe").as_deref(), Some("?User"));
    assert_eq!(f.method8("User", "arrow").as_deref(), Some("Closure"));
}

#[test]
fn doc_types_keep_array_elements() {
    let f = Fixture::new(
        r#"<?php
namespace App;
class User {}
function users() { return [new User(), new User()]; }
"#,
    );
    let (native, doc) = f.ret(None, "users", PhpVersion(8, 3)).unwrap();
    assert_eq!(native.as_deref(), Some("array"));
    assert_eq!(doc, "User[]");
}

#[test]
fn inherited_return_types() {
    let f = Fixture::new(
        r#"<?php
interface Sized { public function size(): int; }
class Box implements Sized { public function size() { return 'x'; } }
class Base { public function id() { return 1; } public function name() { return 'n'; } }
class Child extends Base { public function id() { return 2; } public function name(): string { return 'c'; } }
"#,
    );
    assert_eq!(f.method8("Box", "size").as_deref(), Some("int"), "the interface decides");
    assert_eq!(f.method8("Base", "id"), None, "the override in Child has no return type");
    assert_eq!(f.method8("Child", "id").as_deref(), Some("int"));
    assert_eq!(f.method8("Base", "name").as_deref(), Some("string"), "the override already declares string");
}

#[test]
fn parameter_types() {
    let f = Fixture::new(
        r#"<?php
namespace App;
class User {}
/**
 * @param int $a
 * @param User $u
 * @param string[] $names
 */
function p($a, $u = null, $names = [], $limit = 10, $flag = false, $none = null, $plain) {}
/** @param int ...$ids */
function v(...$ids) {}
class P { public function m($a, $b) {} }
interface I { public function n(array $a); }
class Q extends P implements I {
    public function m($a, $b) {}
    public function n($a) {}
}
"#,
    );
    assert_eq!(f.param8(None, "p", 0).as_deref(), Some("int"));
    assert_eq!(f.param8(None, "p", 1).as_deref(), Some("?User"));
    assert_eq!(f.param8(None, "p", 2).as_deref(), Some("array"));
    assert_eq!(f.param8(None, "p", 3).as_deref(), Some("int"));
    assert_eq!(f.param8(None, "p", 4).as_deref(), Some("bool"));
    assert_eq!(f.param8(None, "p", 5), None);
    assert_eq!(f.param8(None, "p", 6), None);
    assert_eq!(f.param8(None, "v", 0).as_deref(), Some("int"));
    assert_eq!(f.param8(Some("Q"), "m", 0), None, "the parent parameter is untyped");
    assert_eq!(f.param8(Some("Q"), "n", 0).as_deref(), Some("array"), "the interface decides");
}

#[test]
fn property_types() {
    let f = Fixture::new(
        r#"<?php
namespace App;
class User {}
class A {
    /** @var string */
    private $name;
    private $count = 0;
    /** @var User|null */
    protected $user = null;
    public $shared;
    public $any;
}
class B extends A {
    public $shared;
}
"#,
    );
    assert_eq!(f.prop8("A::name"), Some(("?string".into(), true)));
    assert_eq!(f.prop8("A::count"), Some(("int".into(), false)));
    assert_eq!(f.prop8("A::user"), Some(("?User".into(), false)));
    assert_eq!(f.prop8("A::shared"), None, "nothing to go on");
    assert_eq!(f.prop8("B::shared"), None, "the parent property is untyped");
    assert_eq!(f.prop8("A::any"), None);
}

#[test]
fn templates_aliases_and_unknown_receivers_are_not_trusted() {
    let f = Fixture::new(
        r#"<?php
namespace App;
class Coll {
    /**
     * @template R
     * @param callable(): R $cb
     * @return $this|R
     */
    public function when($cb) { return $this; }
    /** @return static */
    public function copy() { return clone $this; }
}
/** @phpstan-type Rules array{psr-4?: array<string, string>} */
class Package {
    /** @var Rules */
    private $rules;
    public function rules() { return $this->rules; }
}
function tpl(Coll $c) { return $c->when(fn() => 1); }
function fluent(Coll $c) { return $c->copy(); }
function fluentUnknown($c) { return $c->copy(); }
"#,
    );
    assert_eq!(f.ret8("tpl"), None, "an unbound template may be anything");
    assert_eq!(f.ret8("fluent").as_deref(), Some("Coll"));
    assert_eq!(f.ret8("fluentUnknown"), None, "the receiver's type is not fully known");
    assert_eq!(f.method8("Package", "rules"), None, "Rules is a type alias, not a class");
    assert_eq!(f.prop8("Package::rules"), None);
}

#[test]
fn duplicate_function_definitions_all_count() {
    let mut f = Fixture::new(
        r#"<?php
namespace App;
class A {}
class B {}
if (!function_exists('make')) { function make(): A { return new A(); } }
function useIt() { return make(); }
"#,
    );
    f.index.update_file(index_source(
        "file:///other.php".into(),
        "<?php namespace App; if (!function_exists('make')) { function make(): B { return new B(); } }",
    ));
    assert_eq!(f.ret8("useIt").as_deref(), Some("A|B"));
}

#[test]
fn trait_members_count_as_parents() {
    let f = Fixture::new(
        r#"<?php
namespace App;
trait Guards { protected $fillable = []; }
class Model { use Guards; }
class City extends Model { protected $fillable = ['name']; }
class Req { public function authorize() { return true; } }
"#,
    );
    assert_eq!(f.prop8("City::fillable"), None, "the trait declares it untyped");
    assert_eq!(f.method8("Req", "authorize").as_deref(), Some("bool"));
}
