//! Syntax that real-world code (and PHP 8.4) uses and that once tripped the parser.

fn errors(src: &str) -> Vec<String> {
    php_syntax::parse(src).errors.iter().map(|e| e.message.clone()).collect()
}

#[test]
fn nested_quotes_in_interpolation() {
    assert!(errors(
        r#"<?php $x = " Found {$opts["Cipher"]}. Only 'gcm'."; $y = "a {$this->f($this->g(" {$z} "))} b";"#
    )
    .is_empty());
}

#[test]
fn instanceof_static_property() {
    assert!(errors("<?php class A { static $c; function f($x) { return $x instanceof static::$c; } }").is_empty());
}

#[test]
fn property_hooks_and_asymmetric_visibility() {
    let src = r#"<?php
class P {
    public string $a = 'x' { get => strtoupper($this->a); set { $this->a = $value; } }
    public private(set) \Foo $b;
    protected(set) int $c = 1;
    public function __construct(public private(set) string $d, public string $e { set => $value; }) {}
}
interface I { public string $name { get; } }
"#;
    assert_eq!(errors(src), Vec::<String>::new());
}

#[test]
fn docblock_survives_attributes() {
    let src = "<?php class A {\n /** @return int */\n #[Override]\n public function f() {}\n}";
    let p = php_syntax::parse(src);
    let php_syntax::StmtKind::Class(c) = &p.stmts[0].kind else { panic!("class") };
    let php_syntax::Member::Method { decl, .. } = &c.members[0] else { panic!("method") };
    assert!(decl.doc.is_some());
}

#[test]
fn new_from_static_property() {
    let src = "<?php class A { static $b; function f($q) { return new static::$b($q); } }";
    assert!(errors(src).is_empty());
    let p = php_syntax::parse(src);
    let dbg = format!("{:?}", p.stmts);
    assert!(dbg.contains("StaticProp"), "{dbg}");
}

#[test]
fn jump_keywords_with_multibyte_text() {
    let src = "<?php echo 'Ünal'; return 1; returned(); break;";
    let p = php_syntax::parse(src);
    let kws: Vec<_> = p.stmts.iter().map(|s| s.jump_keyword(src)).collect();
    assert_eq!(kws, vec![None, Some("return"), None, Some("break")]);
}

#[test]
fn trivia_lexing_leaves_only_whitespace_between_tokens() {
    use php_syntax::lexer::{lex_with_trivia, Tok};
    let src = "<html>\n<?php // a\n/** doc */\n$a = 1; # b\n/* c */ ?>\n<p><?= $x ?></p>\n<?PHP\necho 1;";
    let toks = lex_with_trivia(src);
    let mut at = 0;
    for t in &toks {
        assert!(src[at..t.span.start as usize].trim().is_empty(), "gap {:?}", &src[at..t.span.start as usize]);
        at = t.span.end as usize;
    }
    let kinds = |k: Tok| toks.iter().filter(|t| t.kind == k).count();
    assert_eq!(kinds(Tok::Comment), 4);
    assert_eq!(kinds(Tok::OpenTag), 2);
    // the doc comment is still attached to the next real token
    assert!(toks.iter().any(|t| t.kind == Tok::Variable && t.doc.is_some()));
}
