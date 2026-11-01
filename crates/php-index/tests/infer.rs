//! Type-inference regression tests: arrays, generics (@template), narrowing, closure parameter hints.

use php_index::{analyze, index_source, Index, RefTarget};

const SRC: &str = r#"<?php
namespace App;

class User { public function team(): ?Team { return null; } }
class Team {}

/** @template T */
class Box {
    /** @param T $v */
    public function __construct(public $v) {}
    /** @return T */
    public function get() {}
}

/**
 * @template TKey of array-key
 * @template TValue
 */
class Coll {
    /** @return TValue|null */
    public function first() {}
    /**
     * @template U
     * @param callable(TValue): U $cb
     * @return Coll<TKey, U>
     */
    public function map(callable $cb) {}
}

/**
 * @template T
 * @param class-string<T> $c
 * @return T
 */
function make(string $c) {}

/**
 * @template V
 * @param iterable<V> $items
 * @return Coll<int, V>
 */
function wrap($items) {}

class Svc {
    /** @var User[] */
    private array $users = [];

    public function run(User|Team $either, $unknown) {
        foreach ($this->users as $u) { $u; }
        $lit = [new User, new User];
        $lit;
        [$first] = $lit;
        $first;
        $arr = [];
        $arr[] = new Team;
        $arr;
        $box = new Box(new User);
        $box;
        $got = $box->get();
        $got;
        $made = make(Team::class);
        $made;
        $w = wrap($this->users);
        $w;
        $head = $w->first();
        $head;
        $mapped = $w->map(fn($x) => $x->team());
        $mapped;
        $mapped_user = array_map(fn($z) => $z->team(), $this->users);
        $mapped_user;
        if ($either instanceof User) { $either; }
        if (!($unknown instanceof Team)) { return; }
        $unknown;
        $cond = $either instanceof Team ? $either : new User;
        $cond;
    }
}
"#;

fn types_of(var: &str, nth: usize) -> Vec<String> {
    let mut idx = Index::new();
    idx.update_file(index_source("file:///t.php".into(), SRC));
    let parse = php_syntax::parse(SRC);
    let a = analyze(&idx, SRC, &parse.stmts);
    let mut hits: Vec<Vec<String>> = a
        .refs
        .iter()
        .filter_map(|r| match &r.target {
            RefTarget::Variable { name, classes, .. } if name == var => Some(classes.clone()),
            _ => None,
        })
        .collect();
    assert!(hits.len() > nth, "no reference #{nth} for ${var}");
    hits.swap_remove(nth)
}

fn last(var: &str) -> Vec<String> {
    let mut idx = Index::new();
    idx.update_file(index_source("file:///t.php".into(), SRC));
    let parse = php_syntax::parse(SRC);
    let a = analyze(&idx, SRC, &parse.stmts);
    a.refs
        .iter()
        .rev()
        .find_map(|r| match &r.target {
            RefTarget::Variable { name, classes, .. } if name == var => Some(classes.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no ${var}"))
}

#[test]
fn arrays() {
    assert_eq!(last("u"), ["App\\User"]);
    assert_eq!(last("lit"), ["App\\User[]"]);
    assert_eq!(last("first"), ["App\\User"]);
    assert_eq!(last("arr"), ["App\\Team[]"]);
}

#[test]
fn generics() {
    assert_eq!(last("box"), ["App\\Box<App\\User>"]);
    assert_eq!(last("got"), ["App\\User"]);
    assert_eq!(last("made"), ["App\\Team"]);
    assert_eq!(last("w"), ["App\\Coll<mixed,App\\User>"]);
    assert_eq!(last("head"), ["App\\User"]);
    assert_eq!(last("mapped"), ["App\\Coll<mixed,App\\Team>"]);
    assert_eq!(last("mapped_user"), ["App\\Team[]"]);
}

#[test]
fn narrowing() {
    // `$either` inside the `instanceof User` branch (last use before the ternary)
    let hits = types_of("either", 2);
    assert_eq!(hits, ["App\\User"]);
    // guard clause: `if (!($unknown instanceof Team)) return;`
    assert_eq!(last("unknown"), ["App\\Team"]);
    let c = last("cond");
    assert!(c.contains(&"App\\Team".to_string()) && c.contains(&"App\\User".to_string()), "{c:?}");
}
