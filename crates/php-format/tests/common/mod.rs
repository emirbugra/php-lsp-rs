//! Shared by the formatter tests.

use php_format::FormatOptions;

/// PSR-12 with the line structure of the input kept (braces where they are, no
/// blank lines added, one-line blocks kept), for cases about indentation and spaces only.
pub fn indent_only() -> FormatOptions {
    let mut o = FormatOptions::psr12();
    for key in FormatOptions::KEYS.iter().filter(|k| k.starts_with("BLANK_LINES_")) {
        o.set(key, "0");
    }
    o.NEW_LINE_AFTER_PHP_OPENING_TAG = false;
    o.CLASS_BRACE_STYLE = 1;
    o.METHOD_BRACE_STYLE = 1;
    o.KEEP_SIMPLE_BLOCKS_IN_ONE_LINE = true;
    o.KEEP_SIMPLE_METHODS_IN_ONE_LINE = true;
    o.KEEP_SIMPLE_LAMBDAS_IN_ONE_LINE = true;
    o.KEEP_SIMPLE_CLASSES_IN_ONE_LINE = true;
    o.KEEP_MULTIPLE_EXPRESSIONS_IN_ONE_LINE = true;
    o.CASE_STATEMENT_ON_NEW_LINE = false;
    o.IF_BRACE_FORCE = 0;
    o.FOR_BRACE_FORCE = 0;
    o.WHILE_BRACE_FORCE = 0;
    o.DOWHILE_BRACE_FORCE = 0;
    o.ELSE_IF_STYLE = "AS_IS".into();
    o.CALL_PARAMETERS_WRAP = 0;
    o.METHOD_PARAMETERS_WRAP = 0;
    o.ARRAY_INITIALIZER_WRAP = 0;
    o.METHOD_CALL_CHAIN_WRAP = 0;
    o
}
