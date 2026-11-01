//! Templates (HTML with PHP): code inside multi-line `<?php … ?>` blocks is indented from the
//! `<?php` line, spaces between PHP tokens follow the rules; HTML and tags stay as written.

use php_format::{format, FormatOptions};

fn fmt(src: &str, o: &FormatOptions) -> String {
    let out = format(src, o).unwrap_or_else(|e| panic!("format failed: {e}"));
    assert_eq!(format(&out, o).unwrap(), out, "not idempotent");
    out
}

const SRC: &str = r#"<html>
<body>
    <div id="<?php echo $this->name(); ?>" class="<?=$a+1?>">
    <?php
foreach ($items as $item) {
if ($item->ok) {
echo $item->title.' - '.$item->price;
}
}
    ?>
    </div>
    <?php if ($user): ?>
        <p>Hello <?= htmlspecialchars( $user->name ) ?></p>
    <?php else: ?>
        <p>Guest</p>
    <?php endif; ?>
    <ul>
        <?php foreach ($rows as $r) { ?>
            <li><?= $r['a']+$r['b'] ?></li>
        <?php } ?>
    </ul>
</body>
</html>
"#;

#[test]
fn php_blocks_in_html() {
    let out = fmt(SRC, &FormatOptions::psr12());
    assert_eq!(
        out,
        SRC.replace("<?=$a+1?>", "<?= $a + 1 ?>")
            .replace(
                "foreach ($items as $item) {\nif ($item->ok) {\necho $item->title.' - '.$item->price;\n}\n}",
                "    foreach ($items as $item) {\n        if ($item->ok) {\n            echo $item->title . ' - ' . $item->price;\n        }\n    }"
            )
            .replace("htmlspecialchars( $user->name )", "htmlspecialchars($user->name)")
            .replace("$r['a']+$r['b']", "$r['a'] + $r['b']")
    );
    // no line structure changes in templates: braces, commas, `<?php` alone on its line
    assert!(out.contains("<?php if ($user): ?>") && out.contains("<?php foreach ($rows as $r) { ?>"));
}

#[test]
fn code_in_tags_and_echo_spaces_options() {
    let mut o = FormatOptions::psr12();
    o.INDENT_CODE_IN_PHP_TAGS = true;
    o.SPACES_WITHIN_SHORT_ECHO_TAGS = false;
    assert_eq!(fmt("<p><?= $a ?></p>\n  <?php\n$b = 1;\n  ?>\n", &o), "<p><?=$a?></p>\n  <?php\n      $b = 1;\n  ?>\n");
}

#[test]
fn a_block_opened_in_a_reindented_line_is_stable() {
    // the `<?php` of the second block sits on a code line that moves; formatting twice agrees
    let src = "<input class=\"x <?php if ($e) :\n        $m = 1; ?> bad <?php unset($m);\nif ($c) { $m = 2; }\nendif; ?>\">\n";
    fmt(src, &FormatOptions::psr12());
}
