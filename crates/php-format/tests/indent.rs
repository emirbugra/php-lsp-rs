//! Indentation with the PSR-12 preset. Every case is also checked for idempotency.

mod common;

use common::indent_only;
use php_format::{format, line_indent, FormatError, FormatOptions};

fn fmt(src: &str) -> String {
    let o = indent_only();
    let out = format(src, &o).unwrap_or_else(|e| panic!("format failed: {e}"));
    assert_eq!(format(&out, &o).unwrap(), out, "not idempotent");
    out
}

/// Strip the common indentation of a raw string literal so cases read naturally.
fn code(s: &str) -> String {
    let s = s.strip_prefix('\n').unwrap_or(s);
    let min = s.lines().filter(|l| !l.trim().is_empty()).map(|l| l.len() - l.trim_start().len()).min().unwrap_or(0);
    s.trim_end().lines().map(|l| if l.len() >= min { &l[min..] } else { l.trim_start() }).collect::<Vec<_>>().join("\n")
        + "\n"
}

fn check(input: &str, expected: &str) {
    let (input, expected) = (code(input), code(expected));
    assert_eq!(fmt(&input), expected);
}

#[test]
fn method_chains_get_continuation_indent() {
    check(
        r#"
        <?php
        function f() {
        $isExist = DB::table('password_resets')
        ->where('email', $data['email'])
        ->exists();
        }
        "#,
        r#"
        <?php
        function f() {
            $isExist = DB::table('password_resets')
                ->where('email', $data['email'])
                ->exists();
        }
        "#,
    );
}

#[test]
fn closures_and_arrays_inside_calls() {
    check(
        r#"
        <?php
        return $q->where(1)
        ->map(function ($x) {
        return [
        'a' => $x,
        'b' => foo(
        1,
        2
        ),
        ];
        })
        ->all();
        "#,
        r#"
        <?php
        return $q->where(1)
            ->map(function ($x) {
                return [
                    'a' => $x,
                    'b' => foo(
                        1,
                        2
                    ),
                ];
            })
            ->all();
        "#,
    );
}

#[test]
fn switch_match_and_wrapped_conditions() {
    check(
        r#"
        <?php
        switch ($a) {
        case 1:
        case 2:
        foo();
        break;
        default:
        bar();
        }
        $x = match ($y) {
        1, 2 => 'a',
        default => 'b',
        };
        if ($a &&
        $b) {
        baz();
        }
        "#,
        r#"
        <?php
        switch ($a) {
            case 1:
            case 2:
                foo();
                break;
            default:
                bar();
        }
        $x = match ($y) {
            1, 2 => 'a',
            default => 'b',
        };
        if ($a &&
            $b) {
            baz();
        }
        "#,
    );
}

#[test]
fn classes_attributes_and_else_chains() {
    check(
        r#"
        <?php
        namespace App;
        final class A extends B
        {
        #[Route('/x')]
        public function x(): int
        {
        if ($a) {
        return 1;
        }
        else {
        return 2;
        }
        }
        }
        "#,
        r#"
        <?php
        namespace App;
        final class A extends B {
            #[Route('/x')]
            public function x(): int {
                if ($a) {
                    return 1;
                } else {
                    return 2;
                }
            }
        }
        "#,
    );
}

#[test]
fn alternative_syntax() {
    check(
        r#"
        <?php
        if ($a):
        foo();
        if ($b) {
        x();
        } else {
        y();
        }
        elseif ($c):
        bar();
        else:
        baz();
        endif;
        foreach ($l as $i):
        echo $i;
        endforeach;
        "#,
        r#"
        <?php
        if ($a):
            foo();
            if ($b) {
                x();
            } else {
                y();
            }
        elseif ($c):
            bar();
        else:
            baz();
        endif;
        foreach ($l as $i):
            echo $i;
        endforeach;
        "#,
    );
}

#[test]
fn unbraced_bodies_and_closures_ending_a_statement() {
    check(
        r#"
        <?php
        if ($a)
        foo();
        $f = function () {
        return 1;
        };
        $g = 2;
        "#,
        r#"
        <?php
        if ($a)
            foo();
        $f = function () {
            return 1;
        };
        $g = 2;
        "#,
    );
}

#[test]
fn comments() {
    check(
        r#"
        <?php
        class A {
        /**
        * Doc.
        *
        * @return int
        */
        public function a() {
         // inside
        return 1;
         // before brace
        }
        }
        // first column stays
          // indented moves
        "#,
        r#"
        <?php
        class A {
            /**
             * Doc.
             *
             * @return int
             */
            public function a() {
                // inside
                return 1;
                // before brace
            }
        }
        // first column stays
        // indented moves
        "#,
    );
}

#[test]
fn strings_and_heredocs_are_untouched() {
    let src = "<?php\nfunction f() {\n$a = \"line one   \n  line two\";\n$b = <<<EOT\n  keep   \n    this\n  EOT;\nreturn $a;\n}\n";
    let out = fmt(src);
    assert_eq!(
        out,
        "<?php\nfunction f() {\n    $a = \"line one   \n  line two\";\n    $b = <<<EOT\n  keep   \n    this\n  EOT;\n    return $a;\n}\n"
    );
}

#[test]
fn blank_lines_trailing_spaces_and_final_newline() {
    let out = fmt("<?php\n\n\n\n$a = 1;   \n\n\n\n\n$b = 2;\n\n\n");
    assert_eq!(out, "<?php\n\n\n$a = 1;\n\n\n$b = 2;\n");
    let crlf = fmt("<?php\r\nif ($a) {\r\nfoo();\r\n}\r\n");
    assert_eq!(crlf, "<?php\r\nif ($a) {\r\n    foo();\r\n}\r\n");
}

#[test]
fn templates_are_left_alone_for_now() {
    let src = "<div>\n<?php if ($a): ?>\n  <p><?= $b ?></p>\n<?php endif; ?>\n</div>\n";
    assert_eq!(fmt(src), src);
}

#[test]
fn tabs_and_other_sizes() {
    let mut o = indent_only();
    o.USE_TAB_CHARACTER = true;
    o.CONTINUATION_INDENT_SIZE = 8;
    let out = format("<?php\nfunction f() {\n$a = b()\n->c();\n}\n", &o).unwrap();
    assert_eq!(out, "<?php\nfunction f() {\n\t$a = b()\n\t\t\t->c();\n}\n");
}

#[test]
fn broken_files_are_refused() {
    assert!(matches!(format("<?php\nfunction f() {\n", &FormatOptions::psr12()), Err(FormatError::Unbalanced(_))));
    assert!(matches!(format("<?php\nfoo(]);\n", &FormatOptions::psr12()), Err(FormatError::Unbalanced(_))));
}

#[test]
fn indent_for_a_new_line() {
    let o = FormatOptions::psr12();
    let src = "<?php\nfunction f() {\n    $a = b()\n\n}\n";
    assert_eq!(line_indent(src, 3, &o).unwrap().as_deref(), Some("        "), "continues `$a = b()`");
    let src = "<?php\nfunction f() {\n    $a = 1;\n\n}\n";
    assert_eq!(line_indent(src, 3, &o).unwrap().as_deref(), Some("    "));
}

#[test]
fn keywords_used_as_names() {
    check(
        r#"
        <?php
        interface R
        {
        public function for(Reflector $r): array;
        public static function match(): int;
        }
        $x->if(1)
        ->else();
        "#,
        r#"
        <?php
        interface R {
            public function for(Reflector $r): array;
            public static function match(): int;
        }
        $x->if(1)
            ->else();
        "#,
    );
}

#[test]
fn short_open_tags_are_refused() {
    assert_eq!(format("<?\nif ($a) {\nfoo();\n}\n", &FormatOptions::psr12()), Err(FormatError::ShortOpenTag));
    // `<?=` and `<?xml` are fine
    assert!(format("<p><?= $a ?></p>\n<?xml version=\"1.0\"?>\n", &FormatOptions::psr12()).is_ok());
}

#[test]
fn only_a_range_of_lines() {
    let src = "<?php\nif ($a) {\nfoo();\nbar();\n}\n";
    let out = php_format::format_lines(src, &FormatOptions::psr12(), 2, 2).unwrap();
    assert_eq!(out, "<?php\nif ($a) {\n    foo();\nbar();\n}\n");
}

#[test]
fn indent_while_typing_an_unclosed_block() {
    let src = "<?php\nfunction f() {\n    if ($a) {\n\n";
    assert_eq!(line_indent(src, 3, &FormatOptions::psr12()).unwrap().as_deref(), Some("        "));
}
