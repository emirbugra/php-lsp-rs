//! Code style import / export: PhpStorm schemes and `.editorconfig`.

use php_format::{editorconfig::to_editorconfig, EditorConfig, FormatOptions};

/// The options PhpStorm's own PSR12 style sets (from its `PSR12CodeStyle` class), as a scheme.
const PHPSTORM_PSR12: &str = include_str!("data/phpstorm-psr12.xml");

#[test]
fn phpstorm_psr12_scheme_is_our_preset() {
    let i = FormatOptions::from_phpstorm_xml(PHPSTORM_PSR12).unwrap();
    assert_eq!(i.name.as_deref(), Some("PSR12"));
    assert_eq!(i.options.diff(&FormatOptions::psr12()), vec![]);
    assert!(i.invalid.is_empty());
    // options the formatter does not have (yet) are reported, the others applied
    assert!(i.unsupported.iter().all(|u| !FormatOptions::KEYS.contains(&u.as_str())));
}

#[test]
fn scheme_round_trip_and_sections() {
    let mut o = FormatOptions::psr12();
    o.INDENT_SIZE = 2;
    o.CONCAT_SPACES = false;
    o.SPACE_AFTER_TYPE_CAST = true;
    o.LINE_SEPARATOR = "\r\n".into();
    let xml = o.to_phpstorm_xml("Team");
    // only what differs from PhpStorm's defaults, each in its own part of the scheme
    assert!(xml.contains("<option name=\"LINE_SEPARATOR\" value=\"&#13;&#10;\" />"));
    let php = &xml[xml.find("<PHPCodeStyleSettings>").unwrap()..xml.find("</PHPCodeStyleSettings>").unwrap()];
    assert!(php.contains("<option name=\"CONCAT_SPACES\" value=\"false\" />"));
    assert!(php.contains("<option name=\"LOWER_CASE_NULL_CONST\" value=\"true\" />"), "PSR-12, not PhpStorm's default");
    assert!(xml.contains("<option name=\"SPACE_AFTER_TYPE_CAST\" value=\"true\" />"));
    assert!(xml.contains("<indentOptions>\n      <option name=\"INDENT_SIZE\" value=\"2\" />"));
    assert!(!xml.contains("STRIP_TRAILING_SPACES"), "an editor setting, not code style");
    assert!(!xml.contains("SPACE_AROUND_ASSIGNMENT_OPERATORS"), "PhpStorm's default");
    let back = FormatOptions::from_phpstorm_xml(&xml).unwrap();
    assert_eq!(back.options, o);
    assert_eq!(back.name.as_deref(), Some("Team"));
}

#[test]
fn project_scheme_file_and_other_languages() {
    // `.idea/codeStyles/Project.xml` wraps the scheme; options of other languages are not ours
    let xml = r#"<component name="ProjectCodeStyleConfiguration">
      <code_scheme name="Project" version="173">
        <option name="RIGHT_MARGIN" value="100" />
        <option name="WRAP_COMMENTS" value="true" />
        <codeStyleSettings language="JavaScript"><option name="KEEP_BLANK_LINES_IN_CODE" value="0" /></codeStyleSettings>
        <codeStyleSettings language="PHP">
          <option name="KEEP_BLANK_LINES_IN_CODE" value="1" />
          <indentOptions><option name="USE_TAB_CHARACTER" value="true" /></indentOptions>
        </codeStyleSettings>
      </code_scheme>
    </component>"#;
    let i = FormatOptions::from_phpstorm_xml(xml).unwrap();
    assert_eq!(i.options.KEEP_BLANK_LINES_IN_CODE, 1);
    assert!(i.options.USE_TAB_CHARACTER);
    assert_eq!(i.options.RIGHT_MARGIN, 100);
    assert_eq!(i.unsupported, vec!["WRAP_COMMENTS".to_string()]);
    assert!(FormatOptions::from_phpstorm_xml("<settings/>").is_err());
    assert!(FormatOptions::from_phpstorm_xml("not xml <").is_err());
}

#[test]
fn editorconfig_keys_sections_and_priority() {
    let text = "root = true\n\
        [*]\nindent_style = tab\nindent_size = 2\nend_of_line = crlf\nij_continuation_indent_size = 8\n\
        [*.js]\nindent_size = 7\n\
        [{*.php,*.phtml}]\nindent_size = 4\nij_php_continuation_indent_size = 6\nij_php_concat_spaces = false\n\
        ij_php_wrap_long_lines = true\ntrim_trailing_whitespace = false\nij_php_keep_blank_lines_in_code = many\n";
    let i = EditorConfig::import(text);
    let o = &i.options;
    assert!(o.USE_TAB_CHARACTER);
    assert_eq!((o.INDENT_SIZE, o.TAB_SIZE), (4, 4), "tab_width follows indent_size");
    assert_eq!(o.CONTINUATION_INDENT_SIZE, 6, "ij_php_ wins over ij_");
    assert_eq!(o.LINE_SEPARATOR, "\r\n");
    assert!(!o.CONCAT_SPACES);
    assert!(!o.STRIP_TRAILING_SPACES);
    assert_eq!(i.unsupported, vec!["WRAP_LONG_LINES".to_string()]);
    assert_eq!(i.invalid, vec!["KEEP_BLANK_LINES_IN_CODE = many".to_string()]);
    assert!(EditorConfig::parse(text).root);
}

#[test]
fn editorconfig_round_trip() {
    let mut o = FormatOptions::psr12();
    o.USE_TAB_CHARACTER = true;
    o.INDENT_SIZE = 8;
    o.TAB_SIZE = 8;
    o.LINE_SEPARATOR = "\n".into();
    o.SPACE_BEFORE_IF_PARENTHESES = false;
    o.ENSURE_NEWLINE_AT_EOF = false;
    let text = to_editorconfig(&o);
    assert!(text.contains("indent_style = tab\n"));
    assert!(text.contains("ij_php_space_before_if_parentheses = false\n"));
    assert!(text.contains("ij_continuation_indent_size = 4\n"));
    let back = EditorConfig::import(&text);
    assert_eq!(back.options, o);
    assert!(back.unsupported.is_empty() && back.invalid.is_empty());
}
