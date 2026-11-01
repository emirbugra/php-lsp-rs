//! Presets, trailing commas and short arrays.

use php_format::{format, format_for, FormatOptions, PRESETS};

fn idem(src: &str, o: &FormatOptions, php: (u8, u8)) -> String {
    let out = format_for(src, o, php).unwrap_or_else(|e| panic!("format failed: {e}"));
    assert_eq!(format_for(&out, o, php).unwrap(), out, "not idempotent");
    out
}

#[test]
fn every_preset_formats_and_is_stable() {
    let src = "<?php\nnamespace A;\nuse B;\nclass C extends D {\npublic function f($a,$b) { if ($a) return array(1,2); else if ($b) { return TRUE; } }\n}\n";
    for p in PRESETS {
        let o = FormatOptions::preset(p).unwrap();
        let out = idem(src, &o, (8, 3));
        assert!(out.contains("function f("), "{p}: {out}");
    }
    assert!(FormatOptions::preset("PER-CS").is_some());
    assert!(FormatOptions::preset("nope").is_none());
}

#[test]
fn per_trailing_commas_and_short_arrays() {
    let o = FormatOptions::per_cs();
    let src = "<?php\n$a = array(1, 2);\nfoo(\n    $x,\n    $y\n);\nfunction g(\n    int $a,\n    int $b\n) {\n}\n$m = match ($v) {\n    1 => 'a',\n    default => 'b'\n};\n$f = $o->match(...);\n[$p, $q] = $pair;\n";
    let out = idem(src, &o, (8, 3));
    assert_eq!(
        out,
        "<?php\n\n$a = [1, 2];\nfoo(\n    $x,\n    $y,\n);\nfunction g(\n    int $a,\n    int $b,\n) {}\n\n$m = match ($v) {\n    1 => 'a',\n    default => 'b',\n};\n$f = $o->match(...);\n[$p, $q] = $pair;\n"
    );
    // PHP 7.4: no commas after arguments only from 7.3, after parameters only from 8.0
    let out = idem(src, &o, (7, 2));
    assert!(out.contains("    $y\n);") && out.contains("    int $b\n)"), "{out}");
    let out = idem(src, &o, (7, 4));
    assert!(out.contains("    $y,\n);") && out.contains("    int $b\n)"), "{out}");
    // a method called `match` is not a match expression; destructuring gets no comma
    let src = "<?php\nclass M {\n    public function match($a)\n    {\n        return true;\n    }\n}\nlist(\n    $a,\n    $b\n) = $l;\n";
    let out = idem(src, &o, (8, 3));
    assert!(out.contains("return true;\n    }") && out.contains("    $b\n) = $l;"), "{out}");
}

#[test]
fn laravel_style() {
    let o = FormatOptions::laravel();
    assert_eq!(
        format("<?php\n/**\n * @param string $a\n */\nfunction f($a) { return 'x'.(int) $a; }\n", &o).unwrap(),
        "<?php\n/**\n * @param  string  $a\n */\nfunction f($a)\n{\n    return 'x'.(int) $a;\n}\n"
    );
}

#[test]
fn alignment_options() {
    let mut o = FormatOptions::psr12();
    o.ALIGN_KEY_VALUE_PAIRS = true;
    o.ALIGN_ASSIGNMENTS = true;
    o.ALIGN_CLASS_CONSTANTS = true;
    o.ALIGN_INLINE_COMMENTS = true;
    let src = "<?php\n$a = [\n    'id' => 1,\n    'name' => [\n        'x' => 1,\n        'long' => 2,\n    ],\n];\n$x = 1;\n$longer = 2; // two\n$y .= 3; // three\n\nclass C\n{\n    const A = 1;\n    const LONG = 2;\n}\n";
    assert_eq!(
        idem(src, &o, (8, 3)),
        "<?php\n\n$a = [\n    'id'   => 1,\n    'name' => [\n        'x'    => 1,\n        'long' => 2,\n    ],\n];\n$x      = 1;\n$longer = 2; // two\n$y     .= 3; // three\n\nclass C\n{\n    const A    = 1;\n    const LONG = 2;\n}\n"
    );
    let mut o = FormatOptions::phpstorm();
    o.ALIGN_MULTILINE_PARAMETERS_IN_CALLS = true;
    o.ALIGN_MULTILINE_ARRAY_INITIALIZER_EXPRESSION = true;
    assert_eq!(
        idem("<?php\nfunction f($a,\n$b) {}\nfoo( $x,\n$y);\n$a = [1,\n2];\n", &o, (8, 3)),
        "<?php\nfunction f($a,\n           $b)\n{\n}\n\nfoo($x,\n    $y);\n$a = [1,\n      2];\n"
    );
}
