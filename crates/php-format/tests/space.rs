//! Spaces between tokens and letter case with the PSR-12 preset (values as in PhpStorm). Every
//! case is also checked for idempotency.

mod common;

use common::indent_only;
use php_format::{format, FormatOptions};

fn fmt_with(src: &str, o: &FormatOptions) -> String {
    let out = format(src, o).unwrap_or_else(|e| panic!("format failed: {e}"));
    assert_eq!(format(&out, o).unwrap(), out, "not idempotent");
    out
}

/// Format `<?php\n` + each line on its own and compare with the expected lines.
fn lines(cases: &[(&str, &str)]) {
    let o = indent_only();
    for (input, expected) in cases {
        let out = fmt_with(&format!("<?php\n{input}\n"), &o);
        assert_eq!(out, format!("<?php\n{expected}\n"), "input: {input}");
    }
}

#[test]
fn operators() {
    lines(&[
        ("$x=$a+$b*$c-$d/$e%$f;", "$x = $a + $b * $c - $d / $e % $f;"),
        ("$y = -$x+(+$z);", "$y = -$x + (+$z);"),
        ("$s='a'.$b.'c';", "$s = 'a' . $b . 'c';"),
        ("$ok=!$a&&! $b||$c and $d;", "$ok = !$a && !$b || $c and $d;"),
        ("$v=$a??$b;", "$v = $a ?? $b;"),
        ("$a=$b<<2>>1&3|4^5;", "$a = $b << 2 >> 1 & 3 | 4 ^ 5;"),
        ("$x=$a<=>$b;", "$x = $a <=> $b;"),
        ("$i ++; -- $j;", "$i++; --$j;"),
        ("$x .= $y; $x??=1;", "$x .= $y; $x ??= 1;"),
        ("$k=(int) $v;", "$k = (int)$v;"),
        ("$x = $a - -1;", "$x = $a - -1;"),
        ("$x = @ foo();", "$x = @foo();"),
        // tokens that would run together keep a space
        ("$x = 1 . 2;", "$x = 1 . 2;"),
    ]);
}

#[test]
fn ternaries_and_colons() {
    lines(&[
        ("$t=$a?$b:$c;", "$t = $a ? $b : $c;"),
        ("$u=$a? :$b;", "$u = $a ?: $b;"),
        ("$t = $a ? ($b ? 1 : 2) : [3];", "$t = $a ? ($b ? 1 : 2) : [3];"),
        ("foo( named:1 , other : $a?1:2 );", "foo(named: 1, other: $a ? 1 : 2);"),
        ("function f( ?int $a ) :?Foo {}", "function f(?int $a): ?Foo {}"),
        ("$f = fn( $x ) :int=>$x*2;", "$f = fn($x): int => $x * 2;"),
        ("enum Suit:string {}", "enum Suit: string {}"),
        ("switch ($a) { case 1: break; case 'x' : break; }", "switch ($a) { case 1: break; case 'x' : break; }"),
    ]);
}

#[test]
fn parentheses_brackets_and_braces() {
    lines(&[
        ("if( $a==$b ){ }elseif( $c ){ }else{ }", "if ($a == $b) {} elseif ($c) {} else {}"),
        ("for( $i=0;$i<10;$i++ ){ }", "for ($i = 0; $i < 10; $i++) {}"),
        ("for(;;){ }", "for (; ;) {}"),
        ("foreach( $a as $k=>&$v ){ }", "foreach ($a as $k => &$v) {}"),
        ("while( $x ){ }", "while ($x) {}"),
        ("try{ }catch( A|B $e ){ }finally{ }", "try {} catch (A|B $e) {} finally {}"),
        ("do{ }while( $x );", "do {} while ($x);"),
        ("$r=match( $x ){ default=>1 };", "$r = match ($x) { default => 1 };"),
        ("foo ( $a,$b ); $o -> bar( ); A :: baz();", "foo($a, $b); $o->bar(); A::baz();"),
        ("$cl=function( $a )use( $b ){ };", "$cl = function ($a) use ($b) {};"),
        (
            "$arr=[ 1,2,'a'=>3, ]; $q=$arr [ $i ]; $l = array( 1 );",
            "$arr = [1, 2, 'a' => 3,]; $q = $arr[$i]; $l = array(1);",
        ),
        ("[$a,, $b] = $x;", "[$a, , $b] = $x;"),
        ("return($x);", "return ($x);"),
        ("$o->{$name}; $s{0}; $o->p{$i};", "$o->{$name}; $s{0}; $o->p{$i};"),
        ("isset( $a ); empty ($b); exit (1);", "isset($a); empty($b); exit(1);"),
    ]);
}

#[test]
fn declarations_and_types() {
    lines(&[
        ("declare(strict_types = 1);", "declare(strict_types=1);"),
        ("declare (ticks=1);", "declare (ticks=1);"),
        ("use Foo\\{Bar,Baz};", "use Foo\\{Bar, Baz};"),
        ("class A extends B implements C,D {}", "class A extends B implements C, D {}"),
        ("#[Attr( name:'x' )] function f() {}", "#[Attr(name: 'x')] function f() {}"),
        (
            "function f(array &$items,Foo&Bar $d,int | string ...$rest):static {}",
            "function f(array &$items, Foo&Bar $d, int|string ...$rest): static {}",
        ),
        ("function &ref() {}", "function &ref() {}"),
        (
            "class A { public ?int $id=null; private int | null $k; }",
            "class A { public ?int $id = null; private int|null $k; }",
        ),
        (
            "class A { public function __construct(private readonly ?Foo $f=null) {} }",
            "class A { public function __construct(private readonly ?Foo $f = null) {} }",
        ),
        ("$x = $a instanceof Foo;", "$x = $a instanceof Foo;"),
        ("echo $a ,$b;", "echo $a, $b;"),
    ]);
}

#[test]
fn letter_case() {
    lines(&[
        ("RETURN NULL;", "return null;"),
        ("$a = TRUE || False;", "$a = true || false;"),
        ("IF ($a) { } ELSE { }", "if ($a) {} else {}"),
        ("$x = New Foo; $y = Array(1);", "$x = new Foo; $y = array(1);"),
        // member and constant names keep their case
        ("$o->NULL; A::TRUE; $o->Return(); const NEW = 1;", "$o->NULL; A::TRUE; $o->Return(); const NEW = 1;"),
        ("enum E { case Default; }", "enum E { case Default; }"),
    ]);
    let mut o = indent_only();
    o.LOWER_CASE_BOOLEAN_CONST = false;
    o.LOWER_CASE_NULL_CONST = false;
    o.UPPER_CASE_BOOLEAN_CONST = true;
    o.UPPER_CASE_NULL_CONST = true;
    o.LOWER_CASE_KEYWORDS = false;
    assert_eq!(fmt_with("<?php\nRETURN true ?: null;\n", &o), "<?php\nRETURN TRUE ?: NULL;\n");
}

#[test]
fn options_change_spacing() {
    let mut o = indent_only();
    o.CONCAT_SPACES = false;
    o.SPACE_AFTER_TYPE_CAST = true;
    o.SPACE_WITHIN_PARENTHESES = true;
    o.SPACE_WITHIN_METHOD_CALL_PARENTHESES = true;
    o.SPACE_BEFORE_IF_PARENTHESES = false;
    o.SPACES_AROUND_PIPE_IN_UNION_TYPE = true;
    o.SPACE_AROUND_ASSIGNMENT_IN_DECLARE = true;
    assert_eq!(
        fmt_with(
            "<?php\ndeclare(ticks=1);\nif ($a) { $s = 'a' . (int)$b . foo($c); }\nfunction f(int|null $x) {}\n",
            &o
        ),
        "<?php\ndeclare( ticks = 1 );\nif($a) { $s = 'a'.(int) $b.foo( $c ); }\nfunction f(int | null $x) {}\n"
    );
}

#[test]
fn comments_strings_and_regions() {
    let o = indent_only();
    // tabs before a trailing comment become spaces; strings, comments and heredocs stay
    assert_eq!(
        fmt_with("<?php\n$a=1;\t// x  y\n$b = \"a+b\".'c,d'; /* a=b */\n", &o),
        "<?php\n$a = 1;    // x  y\n$b = \"a+b\" . 'c,d'; /* a=b */\n"
    );
    // the code after a multi-line string on the same line is spaced too
    assert_eq!(fmt_with("<?php\n$a = 'x\ny'.$b;\n", &o), "<?php\n$a = 'x\ny' . $b;\n");
    // `@formatter:off` … `@formatter:on` stays as written
    let src = "<?php\n// @formatter:off\n$a  =  [1,2];\n    $b=1;\n// @formatter:on\n$c=1;\n";
    assert_eq!(fmt_with(src, &o), "<?php\n// @formatter:off\n$a  =  [1,2];\n    $b=1;\n// @formatter:on\n$c = 1;\n");
    // a shebang line does not make the file a template
    assert_eq!(fmt_with("#!/usr/bin/env php\n<?php\n$a=1;\n", &o), "#!/usr/bin/env php\n<?php\n$a = 1;\n");
}
