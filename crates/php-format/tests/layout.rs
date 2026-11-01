//! Line structure with the PSR-12 preset (PhpStorm's values): blank lines, braces, one statement
//! per line, wrapping, braces around single statements. Every case is checked for idempotency.

use php_format::{format, FormatOptions};

fn fmt_with(src: &str, o: &FormatOptions) -> String {
    let out = format(src, o).unwrap_or_else(|e| panic!("format failed: {e}"));
    assert_eq!(format(&out, o).unwrap(), out, "not idempotent");
    out
}

fn fmt(src: &str) -> String {
    fmt_with(src, &FormatOptions::psr12())
}

/// Strip the common indentation of a raw string literal.
fn code(s: &str) -> String {
    let s = s.strip_prefix('\n').unwrap_or(s);
    let min = s.lines().filter(|l| !l.trim().is_empty()).map(|l| l.len() - l.trim_start().len()).min().unwrap_or(0);
    s.trim_end().lines().map(|l| if l.len() >= min { &l[min..] } else { l.trim_start() }).collect::<Vec<_>>().join("\n")
        + "\n"
}

fn check(input: &str, expected: &str) {
    assert_eq!(fmt(&code(input)), code(expected));
}

#[test]
fn file_header_blank_lines() {
    check(
        r#"
        <?php
        declare(strict_types=1);
        namespace App;
        use A\B;
        use B\C;
        use function foo\bar;
        use const X\Y;
        class A
        {
        }
        function f()
        {
        }
        f();
        "#,
        r#"
        <?php

        declare(strict_types=1);

        namespace App;

        use A\B;
        use B\C;

        use function foo\bar;

        use const X\Y;

        class A
        {
        }

        function f()
        {
        }

        f();
        "#,
    );
    // a header comment right after `<?php` stays there; the blank line goes before `namespace`
    check("<?php\n/**\n * License\n */\nnamespace App;\n", "<?php\n/**\n * License\n */\n\nnamespace App;\n");
    // `<?php` alone on its line
    check("<?php declare(strict_types=1);\n", "<?php\n\ndeclare(strict_types=1);\n");
}

#[test]
fn class_members() {
    check(
        r#"
        <?php
        class A extends B {
            use T;
            const X = 1;
            const Y = 2;
            public $a;
            public $b;
            // a comment
            public function f() {}
            /**
             * doc
             */
            #[Attr]
            public function g() { return 1; }
            abstract public function h();
        }
        "#,
        r#"
        <?php

        class A extends B
        {
            use T;

            const X = 1;
            const Y = 2;
            public $a;
            public $b;

            // a comment
            public function f()
            {
            }

            /**
             * doc
             */
            #[Attr]
            public function g()
            {
                return 1;
            }

            abstract public function h();
        }
        "#,
    );
    check(
        "<?php\ninterface I {\npublic function a();\npublic function b();\n}\n",
        "<?php\n\ninterface I\n{\n    public function a();\n\n    public function b();\n}\n",
    );
}

#[test]
fn statements_braces_and_else() {
    check(
        r#"
        <?php
        function j() {
            $a = 1; $b = 2;
            if ($a) { foo(); }
            else { bar(); }
            try { x(); }
            catch (E $e) { y(); }
            finally { z(); }
            do { a(); }
            while ($x);
            switch ($a) { case 1: foo(); break; default: bar(); }
            $f = function () { return 1; };
            $x = new class { public $a; };
            if ($a)
            {
                foo();
            }
        }
        "#,
        r#"
        <?php

        function j()
        {
            $a = 1;
            $b = 2;
            if ($a) {
                foo();
            } else {
                bar();
            }
            try {
                x();
            } catch (E $e) {
                y();
            } finally {
                z();
            }
            do {
                a();
            } while ($x);
            switch ($a) {
                case 1:
                    foo();
                    break;
                default:
                    bar();
            }
            $f = function () {
                return 1;
            };
            $x = new class {
                public $a;
            };
            if ($a) {
                foo();
            }
        }
        "#,
    );
    // `case 1: {` keeps its block; an alternative-syntax `else:` is not joined to a `}`
    check(
        "<?php\nswitch ($a) {\ncase 1: {\nfoo();\n}\n}\nif ($a):\nif ($b) {\nx();\n}\nelse:\ny();\nendif;\n",
        "<?php\n\nswitch ($a) {\n    case 1: {\n        foo();\n    }\n}\nif ($a):\n    if ($b) {\n        x();\n    }\nelse:\n    y();\nendif;\n",
    );
    // PSR-12: with the parameters wrapped, `) {` stays on one line
    check(
        "<?php\nclass A\n{\n    public function f(\n        $a,\n        $b\n    ): int\n    {\n    }\n}\n",
        "<?php\n\nclass A\n{\n    public function f(\n        $a,\n        $b\n    ): int {\n    }\n}\n",
    );
}

#[test]
fn braces_around_single_statements_and_elseif() {
    check(
        r#"
        <?php
        if ($a) foo();
        if ($a) if ($b) foo(); else bar();
        if ($x) { a(); } else if ($y) { b(); } else { c(); }
        while ($i--) $j++;
        foreach ($l as $v) // note
            echo $v;
        do $x++; while ($x < 5);
        while ($x);
        "#,
        r#"
        <?php

        if ($a) {
            foo();
        }
        if ($a) {
            if ($b) {
                foo();
            } else {
                bar();
            }
        }
        if ($x) {
            a();
        } elseif ($y) {
            b();
        } else {
            c();
        }
        while ($i--) {
            $j++;
        }
        foreach ($l as $v) { // note
            echo $v;
        }
        do {
            $x++;
        } while ($x < 5);
        while ($x);
        "#,
    );
}

#[test]
fn wrapping_long_lines() {
    // a list that does not fit is chopped, `(` and `)` on lines of their own; the inner call then
    // fits
    check(
        r#"
        <?php
        $offset = array_search($requiredRouteParametersWithoutDefaultsOrNamedParametersAndMoreThings[0], array_keys($namedParameters));
        "#,
        r#"
        <?php

        $offset = array_search(
            $requiredRouteParametersWithoutDefaultsOrNamedParametersAndMoreThings[0],
            array_keys($namedParameters)
        );
        "#,
    );
    // nested: the outer list, then the one inside that still does not fit
    check(
        r#"
        <?php
        throw new InvalidArgumentException(sprintf('Invalid option specified: "%s". Expected one of (%s).', $option, implode(', ', array_keys(static::$availableOptions))));
        "#,
        r#"
        <?php

        throw new InvalidArgumentException(
            sprintf(
                'Invalid option specified: "%s". Expected one of (%s).',
                $option,
                implode(', ', array_keys(static::$availableOptions))
            )
        );
        "#,
    );
    // a list broken by hand gets one item per line; a multi-line array argument does not count
    check(
        "<?php\nfoo($a, $b,\n    $c);\nBank::create([\n    'code' => 1,\n    'name' => 2,\n]);\n",
        "<?php\n\nfoo(\n    $a,\n    $b,\n    $c\n);\nBank::create([\n    'code' => 1,\n    'name' => 2,\n]);\n",
    );
    // an operand of a binary expression gets the continuation indent too
    check(
        r#"
        <?php
        function f() {
            return \is_string($resource) && 'php' === pathinfo($resource, \PATHINFO_EXTENSION_WITH_A_MUCH_LONGER_NAME_THAN_USUAL) && $b;
        }
        "#,
        r#"
        <?php

        function f()
        {
            return \is_string($resource) && 'php' === pathinfo(
                    $resource,
                    \PATHINFO_EXTENSION_WITH_A_MUCH_LONGER_NAME_THAN_USUAL
                ) && $b;
        }
        "#,
    );
    // `isset()` / `empty()` are not calls: never wrapped
    let long = format!("<?php\n\nunset($a['{}'], $b);\n", "x".repeat(120));
    assert_eq!(fmt(&long), long);
}

#[test]
fn options_turn_rules_off() {
    let mut o = FormatOptions::phpstorm();
    o.RIGHT_MARGIN = 40;
    // PhpStorm's defaults: no wrapping, no forced braces, `else if` as written
    let src = "<?php\nif ($a) foo(); else if ($b) bar();\nfoo($aaaaaaaaaaaaaaa, $bbbbbbbbbbbbbbbbbbbbb, $ccccccccc);\n";
    assert_eq!(
        fmt_with(src, &o),
        "<?php\nif ($a) foo(); else if ($b) bar();\nfoo($aaaaaaaaaaaaaaa, $bbbbbbbbbbbbbbbbbbbbb, $ccccccccc);\n"
    );
    o.CALL_PARAMETERS_WRAP = 1;
    assert_eq!(
        fmt_with("<?php\nfoo($aaaaaaaaaaaaaaa, $bbbbbbbbbbbbbbbbbbbbb, $ccccccccc);\n", &o),
        "<?php\nfoo($aaaaaaaaaaaaaaa,\n    $bbbbbbbbbbbbbbbbbbbbb, $ccccccccc);\n"
    );
}
