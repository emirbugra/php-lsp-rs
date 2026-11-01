//! Doc comments with the PSR-12 preset (PhpStorm's values), and the PHPDoc options.

mod common;

use common::indent_only;
use php_format::{format, FormatOptions};

fn fmt_with(src: &str, o: &FormatOptions) -> String {
    let out = format(src, o).unwrap_or_else(|e| panic!("format failed: {e}"));
    assert_eq!(format(&out, o).unwrap(), out, "not idempotent");
    out
}

/// A doc comment in front of `function f() {}`, formatted; returns the comment lines.
fn doc(lines: &[&str], o: &FormatOptions) -> Vec<String> {
    let src = format!(
        "<?php\n/**\n{}\n */\nfunction f() {{}}\n",
        lines.iter().map(|l| format!(" * {l}")).collect::<Vec<_>>().join("\n")
    );
    let out = fmt_with(&src, o);
    out.lines()
        .skip(2)
        .take_while(|l| !l.starts_with(" */"))
        .map(|l| l.strip_prefix(" * ").unwrap_or(l.trim_start_matches(" *")).to_string())
        .collect()
}

#[test]
fn param_spaces_and_tabs() {
    let o = indent_only();
    assert_eq!(
        doc(&["@param  string  $queue  The queue", "@param\tarray\t$a", "@return\tstring", "@var int  $x - y"], &o),
        ["@param string $queue The queue", "@param array $a", "@return    string", "@var int $x - y"]
    );
    // without a name only the space after the tag changes; types with spaces stay whole
    assert_eq!(
        doc(&["@param\tstring\tThe code", "@param array<string, int>   $map"], &o),
        ["@param string    The code", "@param array<string, int> $map"]
    );
    // a name ends where a name cannot go on
    assert_eq!(doc(&["@param string $envKey|null The env"], &o), ["@param string $envKey |null The env"]);
    // one-line doc comments
    assert_eq!(fmt_with("<?php\n/** @var array  */\n$a = [];\n", &o), "<?php\n/** @var array */\n$a = [];\n");
    // a no-break space is text, not a separator
    let nbsp = "<?php\n/**\n *\u{a0}@param int $length Bytes.\n */\nfunction f() {}\n";
    assert_eq!(fmt_with(nbsp, &o), nbsp);
}

#[test]
fn tags_are_sorted() {
    let o = indent_only();
    assert_eq!(
        doc(
            &["Clean the code.", "", "@throws ParseError if invalid", "", "@param array $lines", "", "@return string"],
            &o
        ),
        ["Clean the code.", "", "@param array $lines", "", "@return string", "@throws ParseError if invalid", ""]
    );
    // continuation lines move with their tag
    assert_eq!(
        doc(&["@see self::offer()", "@throws E when", "    it fails", "@param mixed $e"], &o),
        ["@param mixed $e", "@throws E when", "    it fails", "@see self::offer()"]
    );
    // not sorted: an unknown tag, or a `@see` with a URL (like PhpStorm)
    assert_eq!(
        doc(&["@access public", "@param string $a", "@return string"], &o),
        ["@access public", "@param string $a", "@return string"]
    );
    assert_eq!(doc(&["@see https://example.com", "@return int"], &o), ["@see https://example.com", "@return int"]);
    let mut off = o.clone();
    off.SORT_PHPDOC_ELEMENTS = false;
    assert_eq!(doc(&["@return int", "@param int $a"], &off), ["@return int", "@param int $a"]);
}

#[test]
fn pear_style_options() {
    let mut o = indent_only();
    o.ALIGN_PHPDOC_PARAM_NAMES = true;
    o.ALIGN_PHPDOC_COMMENTS = true;
    o.PHPDOC_BLANK_LINE_BEFORE_TAGS = true;
    o.PHPDOC_BLANK_LINES_AROUND_PARAMETERS = true;
    assert_eq!(
        doc(&["Does things.", "@param string $name The name", "@param int $count How many", "@return bool"], &o),
        ["Does things.", "", "@param string $name  The name", "@param int    $count How many", "", "@return bool"]
    );
    let mut laravel = indent_only();
    laravel.PHPDOC_PARAM_SPACES_BETWEEN_TAG_AND_TYPE = 2;
    laravel.PHPDOC_PARAM_SPACES_BETWEEN_TYPE_AND_NAME = 2;
    assert_eq!(doc(&["@param string $queue"], &laravel), ["@param  string  $queue"]);
    let mut no_blank = indent_only();
    no_blank.PHPDOC_KEEP_BLANK_LINES = false;
    assert_eq!(
        doc(&["Text.", "", "@param int $a", "", "@return int"], &no_blank),
        ["Text.", "@param int $a", "@return int"]
    );
}
