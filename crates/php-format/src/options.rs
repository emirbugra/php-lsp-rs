//! Formatter settings. Field names are PhpStorm's own option names (`INDENT_SIZE`,
//! `KEEP_BLANK_LINES_IN_CODE`, …), so PhpStorm code style schemes and `.editorconfig` files
//! (`ij_php_*` keys) map onto them one to one, and [`FormatOptions::get`] / [`FormatOptions::set`]
//! give import/export by name.
#![allow(non_snake_case)]

macro_rules! options {
    ($($(#[doc = $doc:literal])* $name:ident : $ty:ty = $psr12:expr;)*) => {
        #[derive(Clone, Debug, PartialEq)]
        pub struct FormatOptions {
            $($(#[doc = $doc])* pub $name: $ty,)*
        }

        impl FormatOptions {
            /// PSR-12, the default.
            pub fn psr12() -> Self {
                FormatOptions { $($name: $psr12,)* }
            }

            /// Every option name, in declaration order.
            pub const KEYS: &'static [&'static str] = &[$(stringify!($name),)*];

            /// The value of option `key` as text (`"4"`, `"true"`); None for unknown keys.
            pub fn get(&self, key: &str) -> Option<String> {
                match key {
                    $(stringify!($name) => Some(self.$name.to_string()),)*
                    _ => None,
                }
            }

            /// Set option `key` from text. False when the key is unknown or the value does not parse.
            pub fn set(&mut self, key: &str, value: &str) -> bool {
                match key {
                    // untrimmed first: `LINE_SEPARATOR` is whitespace itself
                    $(stringify!($name) => match value.parse::<$ty>().or_else(|_| value.trim().parse::<$ty>()) {
                        Ok(v) => {
                            self.$name = v;
                            true
                        }
                        Err(_) => false,
                    },)*
                    _ => false,
                }
            }
        }
    };
}

options! {
    /// Columns per indentation level.
    INDENT_SIZE: usize = 4;
    /// Extra columns for a line that continues a statement or an element (`->` chains, wrapped
    /// conditions, operands).
    CONTINUATION_INDENT_SIZE: usize = 4;
    /// Width of a tab character.
    TAB_SIZE: usize = 4;
    /// Indent with tabs instead of spaces.
    USE_TAB_CHARACTER: bool = false;
    /// In templates, code inside `<?php … ?>` one level deeper than the `<?php` line.
    INDENT_CODE_IN_PHP_TAGS: bool = false;
    /// `case` / `default` one level deeper than `switch`.
    INDENT_CASE_FROM_SWITCH: bool = true;
    /// Most blank lines kept in a row inside function bodies.
    KEEP_BLANK_LINES_IN_CODE: usize = 2;
    /// Most blank lines kept in a row between declarations (top level, class bodies).
    KEEP_BLANK_LINES_IN_DECLARATIONS: usize = 2;
    /// Most blank lines kept before a closing `}`.
    KEEP_BLANK_LINES_BEFORE_RBRACE: usize = 0;
    /// Most blank lines kept right after an opening `{`.
    KEEP_BLANK_LINES_AFTER_LBRACE: usize = 0;
    /// Leave line comments (and `/* */` comments) that start in the first column there.
    KEEP_FIRST_COLUMN_COMMENT: bool = true;
    /// Keep the indentation on blank lines.
    KEEP_INDENTS_ON_EMPTY_LINES: bool = false;
    /// Remove whitespace at the end of lines.
    STRIP_TRAILING_SPACES: bool = true;
    /// End the file with exactly one line break.
    ENSURE_NEWLINE_AT_EOF: bool = true;
    /// `\n`, `\r\n`, or empty to keep the file's own line separator.
    LINE_SEPARATOR: String = String::new();
    /// Leave regions between `// @formatter:off` and `// @formatter:on` comments alone.
    FORMATTER_TAGS_ENABLED: bool = true;
    FORMATTER_OFF_TAG: String = "@formatter:off".to_string();
    FORMATTER_ON_TAG: String = "@formatter:on".to_string();

    // ---- blank lines (minimum; the KEEP_BLANK_LINES_* options are the maximum) ----
    BLANK_LINES_AFTER_OPENING_TAG: usize = 1;
    /// Before `namespace`.
    BLANK_LINES_BEFORE_PACKAGE: usize = 1;
    /// After `namespace X;`.
    BLANK_LINES_AFTER_PACKAGE: usize = 1;
    /// Before the `use` imports (and trait `use` in classes).
    BLANK_LINES_BEFORE_IMPORTS: usize = 1;
    BLANK_LINES_AFTER_IMPORTS: usize = 1;
    /// Between `use`, `use function` and `use const` groups.
    BLANK_LINES_BETWEEN_IMPORTS: usize = 1;
    BLANK_LINES_AROUND_CLASS: usize = 1;
    /// After a function declared at the top level.
    BLANK_LINES_AFTER_FUNCTION: usize = 1;
    BLANK_LINES_AROUND_FIELD: usize = 0;
    BLANK_LINES_AROUND_METHOD: usize = 1;
    BLANK_LINES_AROUND_CONSTANTS: usize = 0;
    BLANK_LINES_AROUND_ENUM_CASES: usize = 0;
    BLANK_LINES_AROUND_FIELD_IN_INTERFACE: usize = 0;
    BLANK_LINES_AROUND_METHOD_IN_INTERFACE: usize = 1;
    BLANK_LINES_AFTER_CLASS_HEADER: usize = 0;
    BLANK_LINES_AFTER_ANONYMOUS_CLASS_HEADER: usize = 0;
    BLANK_LINES_BEFORE_CLASS_END: usize = 0;
    BLANK_LINES_BEFORE_METHOD_BODY: usize = 0;
    BLANK_LINES_BEFORE_RETURN_STATEMENT: usize = 0;
    /// One blank line before `return` (older form of the option above).
    BLANK_LINE_BEFORE_RETURN_STATEMENT: bool = false;
    // ---- line breaks and braces ----
    /// `<?php` alone on its line.
    NEW_LINE_AFTER_PHP_OPENING_TAG: bool = true;
    /// Several statements on one line stay there.
    KEEP_MULTIPLE_EXPRESSIONS_IN_ONE_LINE: bool = false;
    /// `case 1: foo();` → `foo();` on its own line.
    CASE_STATEMENT_ON_NEW_LINE: bool = true;
    /// Where `{` goes: 1 end of line, 2 next line, 5 next line if the header is wrapped (3 and 4,
    /// "next line shifted", are treated as 2). For classes, interfaces, traits, enums.
    CLASS_BRACE_STYLE: usize = 2;
    /// Functions and methods.
    METHOD_BRACE_STYLE: usize = 2;
    /// Control statements (`if`, `for`, `try` …).
    BRACE_STYLE: usize = 1;
    /// Closures.
    LAMBDA_BRACE_STYLE: usize = 1;
    /// Anonymous classes.
    ANONYMOUS_BRACE_STYLE: usize = 1;
    /// `namespace X { … }`.
    NAMESPACE_BRACE_STYLE: usize = 1;
    /// With the parameters wrapped, `) {` stays on one line even when `{` goes on the next line.
    KEEP_RPAREN_AND_LBRACE_ON_ONE_LINE: bool = true;
    /// One-line blocks stay on one line: `if ($a) { return; }`.
    KEEP_SIMPLE_BLOCKS_IN_ONE_LINE: bool = false;
    KEEP_SIMPLE_METHODS_IN_ONE_LINE: bool = false;
    KEEP_SIMPLE_LAMBDAS_IN_ONE_LINE: bool = false;
    KEEP_SIMPLE_CLASSES_IN_ONE_LINE: bool = false;
    /// Empty method bodies become `{}` on the declaration line.
    FORCE_EMPTY_METHODS_IN_ONE_LINE: bool = false;
    FORCE_EMPTY_CLASSES_IN_ONE_LINE: bool = false;
    /// Braces around a single-statement body: 0 never, 1 when it spans lines, 3 always. `if` /
    /// `elseif` / `else`.
    IF_BRACE_FORCE: usize = 3;
    /// `for` / `foreach`.
    FOR_BRACE_FORCE: usize = 3;
    WHILE_BRACE_FORCE: usize = 3;
    /// `do … while`.
    DOWHILE_BRACE_FORCE: usize = 3;
    /// `COMBINE`: `else if` → `elseif`; `SEPARATE`: `elseif` → `else if`; `AS_IS`.
    ELSE_IF_STYLE: String = "COMBINE".to_string();
    /// `}` and `else` on separate lines.
    ELSE_ON_NEW_LINE: bool = false;
    CATCH_ON_NEW_LINE: bool = false;
    FINALLY_ON_NEW_LINE: bool = false;
    /// `}` and the `while` of `do … while` on separate lines.
    WHILE_ON_NEW_LINE: bool = false;
    // ---- wrapping ----
    /// Lines longer than this are wrapped where the options below allow it.
    RIGHT_MARGIN: usize = 120;
    /// How lists wrap: 0 never, 1 where needed, 2 always (every item on its own line), 5 every
    /// item on its own line when the list does not fit on one line ("chop down if long").
    /// Arguments of calls.
    CALL_PARAMETERS_WRAP: usize = 5;
    /// A wrapped list starts on a new line after `(`.
    CALL_PARAMETERS_LPAREN_ON_NEXT_LINE: bool = true;
    /// A wrapped list ends with `)` on a line of its own.
    CALL_PARAMETERS_RPAREN_ON_NEXT_LINE: bool = true;
    /// Parameters of functions, methods and closures.
    METHOD_PARAMETERS_WRAP: usize = 5;
    METHOD_PARAMETERS_LPAREN_ON_NEXT_LINE: bool = true;
    METHOD_PARAMETERS_RPAREN_ON_NEXT_LINE: bool = true;
    /// Array literals (`[…]`, `array(…)`).
    ARRAY_INITIALIZER_WRAP: usize = 5;
    ARRAY_INITIALIZER_LBRACE_ON_NEXT_LINE: bool = true;
    ARRAY_INITIALIZER_RBRACE_ON_NEXT_LINE: bool = true;
    /// `->` chains: 0 never, 1 where needed, 2 always.
    METHOD_CALL_CHAIN_WRAP: usize = 1;
    /// Also wrap before the first `->` of a chain (`$this->a()->b()`).
    WRAP_FIRST_METHOD_IN_CALL_CHAIN: bool = false;
    /// A closure or array spanning several lines makes the list holding it count as wrapped.
    TREAT_MULTILINE_ARRAYS_AND_LAMBDAS_MULTILINE: bool = false;
    // ---- trailing commas and arrays (these add / change tokens, all no-ops for PHP) ----
    /// A comma after the last element of a multi-line array.
    COMMA_AFTER_LAST_ARRAY_ELEMENT: bool = false;
    /// A comma after the last argument of a call (PHP 7.3+).
    COMMA_AFTER_LAST_ARGUMENT: bool = false;
    /// `WHEN_MULTILINE` or `ALWAYS`.
    COMMA_AFTER_LAST_ARGUMENT_STYLE: String = "WHEN_MULTILINE".to_string();
    /// A comma after the last parameter (PHP 8.0+).
    COMMA_AFTER_LAST_PARAMETER: bool = false;
    COMMA_AFTER_LAST_PARAMETER_STYLE: String = "WHEN_MULTILINE".to_string();
    /// A comma after the last variable of a closure's `use (…)` (PHP 8.0+).
    COMMA_AFTER_LAST_CLOSURE_USE_VAR: bool = false;
    COMMA_AFTER_LAST_CLOSURE_USE_VAR_STYLE: String = "WHEN_MULTILINE".to_string();
    /// A comma after the last arm of a multi-line `match`.
    COMMA_AFTER_LAST_MATCH_ARM: bool = false;
    /// `array(…)` → `[…]`.
    FORCE_SHORT_DECLARATION_ARRAY_STYLE: bool = false;
    // ---- PHPDoc ----
    /// Spaces in `@param` / `@var` / `@property` lines: tag → type, type → name, name → description.
    PHPDOC_PARAM_SPACES_BETWEEN_TAG_AND_TYPE: usize = 1;
    PHPDOC_PARAM_SPACES_BETWEEN_TYPE_AND_NAME: usize = 1;
    PHPDOC_PARAM_SPACES_BETWEEN_NAME_AND_DESCRIPTION: usize = 1;
    /// Line up the names of the `@param` lines.
    ALIGN_PHPDOC_PARAM_NAMES: bool = false;
    /// Line up the descriptions of the `@param` lines.
    ALIGN_PHPDOC_COMMENTS: bool = false;
    /// A blank line between the description and the tags.
    PHPDOC_BLANK_LINE_BEFORE_TAGS: bool = false;
    /// Blank lines around the `@param` lines.
    PHPDOC_BLANK_LINES_AROUND_PARAMETERS: bool = false;
    /// Keep blank lines inside doc comments.
    PHPDOC_KEEP_BLANK_LINES: bool = true;
    /// Sort tags by the weights below (lower first; equal weights keep their order).
    SORT_PHPDOC_ELEMENTS: bool = true;
    LINK_WEIGHT: usize = 28;
    THROWS_WEIGHT: usize = 2;
    PARAM_WEIGHT: usize = 0;
    RETURN_WEIGHT: usize = 1;
    AUTHOR_WEIGHT: usize = 28;
    INTERNAL_WEIGHT: usize = 28;
    USES_WEIGHT: usize = 28;
    VERSION_WEIGHT: usize = 28;
    API_WEIGHT: usize = 28;
    CATEGORY_WEIGHT: usize = 28;
    COPYRIGHT_WEIGHT: usize = 28;
    DEPRECATED_WEIGHT: usize = 28;
    EXAMPLE_WEIGHT: usize = 28;
    FILESOURCE_WEIGHT: usize = 28;
    GLOBAL_WEIGHT: usize = 28;
    IGNORE_WEIGHT: usize = 28;
    LICENSE_WEIGHT: usize = 28;
    METHOD_WEIGHT: usize = 28;
    PACKAGE_WEIGHT: usize = 28;
    PROPERTY_WEIGHT: usize = 28;
    PROPERTY_READ_WEIGHT: usize = 28;
    PROPERTY_WRITE_WEIGHT: usize = 28;
    SEE_WEIGHT: usize = 28;
    SINCE_WEIGHT: usize = 28;
    SUBPACKAGE_WEIGHT: usize = 28;
    TODO_WEIGHT: usize = 28;
    VAR_WEIGHT: usize = 28;
    UNKNOWN_TAG_WEIGHT: usize = 28;
    // ---- alignment ----
    /// Line up `=>` of the array items that start their line.
    ALIGN_KEY_VALUE_PAIRS: bool = false;
    /// Line up `=` of consecutive one-line assignments.
    ALIGN_ASSIGNMENTS: bool = false;
    /// Line up `=` of consecutive class constants.
    ALIGN_CLASS_CONSTANTS: bool = false;
    /// Line up trailing `//` comments of consecutive lines.
    ALIGN_INLINE_COMMENTS: bool = false;
    /// Wrapped parameters line up with the first one when it stays after `(`.
    ALIGN_MULTILINE_PARAMETERS: bool = false;
    /// Wrapped arguments line up with the first one when it stays after `(`.
    ALIGN_MULTILINE_PARAMETERS_IN_CALLS: bool = false;
    /// Wrapped array items line up with the first one when it stays after `[`.
    ALIGN_MULTILINE_ARRAY_INITIALIZER_EXPRESSION: bool = false;
    // ---- spaces (between tokens on one line) ----
    /// `=`, `+=`, `??=` … and `=>`.
    SPACE_AROUND_ASSIGNMENT_OPERATORS: bool = true;
    /// `&&`, `||`.
    SPACE_AROUND_LOGICAL_OPERATORS: bool = true;
    /// `==`, `!=`, `===`, `!==`, `<>`, `<=>`.
    SPACE_AROUND_EQUALITY_OPERATORS: bool = true;
    /// `<`, `>`, `<=`, `>=`.
    SPACE_AROUND_RELATIONAL_OPERATORS: bool = true;
    /// Binary `&`, `|`, `^`.
    SPACE_AROUND_BITWISE_OPERATORS: bool = true;
    /// Binary `+`, `-`.
    SPACE_AROUND_ADDITIVE_OPERATORS: bool = true;
    /// `*`, `/`, `%`, `**`.
    SPACE_AROUND_MULTIPLICATIVE_OPERATORS: bool = true;
    /// `<<`, `>>`.
    SPACE_AROUND_SHIFT_OPERATORS: bool = true;
    /// Between a unary operator (`-`, `+`, `~`, `++`, `--`, `@`, `&`, `...`) and its operand.
    SPACE_AROUND_UNARY_OPERATOR: bool = false;
    /// Around the concatenation operator `.`.
    CONCAT_SPACES: bool = true;
    /// `??`.
    SPACES_AROUND_NULL_COALESCE_OPERATOR: bool = true;
    /// `<?= $x ?>` instead of `<?=$x?>`.
    SPACES_WITHIN_SHORT_ECHO_TAGS: bool = true;
    /// `=` in `declare(strict_types=1)`.
    SPACE_AROUND_ASSIGNMENT_IN_DECLARE: bool = false;
    /// `->`, `?->`, `::`.
    SPACES_AROUND_ARROW: bool = false;
    SPACE_BEFORE_COMMA: bool = false;
    SPACE_AFTER_COMMA: bool = true;
    SPACE_BEFORE_SEMICOLON: bool = false;
    /// After `;` inside `for (…)`.
    SPACE_AFTER_SEMICOLON: bool = true;
    /// Before `!`.
    SPACE_BEFORE_UNARY_NOT: bool = false;
    /// After `!`.
    SPACE_AFTER_UNARY_NOT: bool = false;
    /// After `(int)`, `(string)` ….
    SPACE_AFTER_TYPE_CAST: bool = false;
    // before `(`
    SPACE_BEFORE_METHOD_CALL_PARENTHESES: bool = false;
    SPACE_BEFORE_METHOD_PARENTHESES: bool = false;
    /// `function (` of a closure.
    SPACE_BEFORE_CLOSURE_LEFT_PARENTHESIS: bool = true;
    /// `fn (` of an arrow function.
    SPACE_BEFORE_SHORT_CLOSURE_LEFT_PARENTHESIS: bool = false;
    /// Also `elseif`, `match`.
    SPACE_BEFORE_IF_PARENTHESES: bool = true;
    SPACE_BEFORE_WHILE_PARENTHESES: bool = true;
    /// Also `foreach`.
    SPACE_BEFORE_FOR_PARENTHESES: bool = true;
    SPACE_BEFORE_SWITCH_PARENTHESES: bool = true;
    SPACE_BEFORE_CATCH_PARENTHESES: bool = true;
    // within `( )`
    /// Grouping parentheses and everything without its own option.
    SPACE_WITHIN_PARENTHESES: bool = false;
    SPACE_WITHIN_METHOD_CALL_PARENTHESES: bool = false;
    SPACE_WITHIN_EMPTY_METHOD_CALL_PARENTHESES: bool = false;
    SPACE_WITHIN_METHOD_PARENTHESES: bool = false;
    SPACE_WITHIN_EMPTY_METHOD_PARENTHESES: bool = false;
    SPACE_WITHIN_IF_PARENTHESES: bool = false;
    SPACE_WITHIN_WHILE_PARENTHESES: bool = false;
    SPACE_WITHIN_FOR_PARENTHESES: bool = false;
    SPACE_WITHIN_SWITCH_PARENTHESES: bool = false;
    SPACE_WITHIN_CATCH_PARENTHESES: bool = false;
    // brackets
    /// `$a[ 0 ]`.
    SPACE_WITHIN_BRACKETS: bool = false;
    /// `[ 1, 2 ]` and `array( 1, 2 )`.
    SPACE_WITHIN_ARRAY_INITIALIZER_BRACES: bool = false;
    SPACE_WITHIN_EMPTY_ARRAY_INITIALIZER_BRACES: bool = false;
    /// `$a[ $i ]`: a lone variable as the index.
    SPACES_AROUND_VAR_WITHIN_BRACKETS: bool = false;
    // before `{`
    SPACE_BEFORE_CLASS_LBRACE: bool = true;
    /// Also closures.
    SPACE_BEFORE_METHOD_LBRACE: bool = true;
    /// Also `elseif`, `match`.
    SPACE_BEFORE_IF_LBRACE: bool = true;
    SPACE_BEFORE_ELSE_LBRACE: bool = true;
    SPACE_BEFORE_WHILE_LBRACE: bool = true;
    /// Also `foreach`.
    SPACE_BEFORE_FOR_LBRACE: bool = true;
    SPACE_BEFORE_DO_LBRACE: bool = true;
    SPACE_BEFORE_SWITCH_LBRACE: bool = true;
    SPACE_BEFORE_TRY_LBRACE: bool = true;
    SPACE_BEFORE_CATCH_LBRACE: bool = true;
    SPACE_BEFORE_FINALLY_LBRACE: bool = true;
    // before keywords after `}`
    /// `} else`, `} elseif`.
    SPACE_BEFORE_ELSE_KEYWORD: bool = true;
    /// `} while` of `do … while`.
    SPACE_BEFORE_WHILE_KEYWORD: bool = true;
    SPACE_BEFORE_CATCH_KEYWORD: bool = true;
    SPACE_BEFORE_FINALLY_KEYWORD: bool = true;
    // ternary
    SPACE_BEFORE_QUEST: bool = true;
    SPACE_AFTER_QUEST: bool = true;
    SPACE_BEFORE_COLON: bool = true;
    SPACE_AFTER_COLON: bool = true;
    /// `?:` instead of `? :`.
    SPACE_BETWEEN_TERNARY_QUEST_AND_COLON: bool = false;
    // other colons
    SPACE_BEFORE_COLON_IN_RETURN_TYPE: bool = false;
    SPACE_AFTER_COLON_IN_RETURN_TYPE: bool = true;
    SPACE_BEFORE_COLON_IN_NAMED_ARGUMENT: bool = false;
    SPACE_AFTER_COLON_IN_NAMED_ARGUMENT: bool = true;
    SPACE_BEFORE_COLON_IN_ENUM_BACKED_TYPE: bool = false;
    SPACE_AFTER_COLON_IN_ENUM_BACKED_TYPE: bool = true;
    // types
    /// `int | string`.
    SPACES_AROUND_PIPE_IN_UNION_TYPE: bool = false;
    /// `A & B`.
    SPACES_AROUND_AMPERSAND_IN_INTERSECTION_TYPE: bool = false;
    /// `( A&B )|null`.
    SPACES_WITHIN_COMPOUND_TYPE_PARENS: bool = false;
    // ---- letter case ----
    /// `IF`, `Function`, `NEW` → lower case.
    LOWER_CASE_KEYWORDS: bool = true;
    /// `TRUE`, `False` → `true`, `false`.
    LOWER_CASE_BOOLEAN_CONST: bool = true;
    /// `NULL` → `null`.
    LOWER_CASE_NULL_CONST: bool = true;
    /// `true` → `TRUE` (when the lower-case option is off).
    UPPER_CASE_BOOLEAN_CONST: bool = false;
    /// `null` → `NULL` (when the lower-case option is off).
    UPPER_CASE_NULL_CONST: bool = false;
}

impl Default for FormatOptions {
    fn default() -> Self {
        Self::psr12()
    }
}

/// Preset names that [`FormatOptions::preset`] knows.
pub const PRESETS: &[&str] =
    &["psr12", "per-cs", "per-cs3", "laravel", "symfony", "pear", "zend", "codeigniter", "phpstorm"];

impl FormatOptions {
    /// Options of a named preset (case-insensitive, `psr-12` works too).
    pub fn preset(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().replace(['-', '_', ' '], "").as_str() {
            "psr12" | "" => Some(Self::psr12()),
            "phpstorm" | "default" => Some(Self::phpstorm()),
            "per" | "percs" | "percs2" | "percs20" => Some(Self::per_cs()),
            "per3" | "percs3" | "percs30" => Some(Self::per_cs3()),
            "laravel" => Some(Self::laravel()),
            "symfony" => Some(Self::symfony()),
            "pear" => Some(Self::pear()),
            "zend" => Some(Self::zend()),
            "codeigniter" => Some(Self::codeigniter()),
            _ => None,
        }
    }

    /// PhpStorm's own defaults for PHP (its "Default" scheme). Code style files only store what
    /// differs from these.
    pub fn phpstorm() -> Self {
        FormatOptions {
            KEEP_BLANK_LINES_BEFORE_RBRACE: 2,
            KEEP_BLANK_LINES_AFTER_LBRACE: 2,
            LOWER_CASE_BOOLEAN_CONST: false,
            LOWER_CASE_NULL_CONST: false,
            BLANK_LINES_AFTER_OPENING_TAG: 0,
            BLANK_LINES_AFTER_PACKAGE: 0,
            BLANK_LINES_BETWEEN_IMPORTS: 0,
            NEW_LINE_AFTER_PHP_OPENING_TAG: false,
            KEEP_RPAREN_AND_LBRACE_ON_ONE_LINE: false,
            IF_BRACE_FORCE: 0,
            FOR_BRACE_FORCE: 0,
            WHILE_BRACE_FORCE: 0,
            DOWHILE_BRACE_FORCE: 0,
            ELSE_IF_STYLE: "AS_IS".to_string(),
            CALL_PARAMETERS_WRAP: 0,
            CALL_PARAMETERS_LPAREN_ON_NEXT_LINE: false,
            CALL_PARAMETERS_RPAREN_ON_NEXT_LINE: false,
            METHOD_PARAMETERS_WRAP: 0,
            METHOD_PARAMETERS_LPAREN_ON_NEXT_LINE: false,
            METHOD_PARAMETERS_RPAREN_ON_NEXT_LINE: false,
            ARRAY_INITIALIZER_WRAP: 0,
            ARRAY_INITIALIZER_LBRACE_ON_NEXT_LINE: false,
            ARRAY_INITIALIZER_RBRACE_ON_NEXT_LINE: false,
            METHOD_CALL_CHAIN_WRAP: 0,
            ALIGN_MULTILINE_PARAMETERS: true,
            ..Self::psr12()
        }
    }

    /// PER Coding Style 2.0 (the values of PhpStorm's predefined style).
    pub fn per_cs() -> Self {
        FormatOptions {
            FORCE_EMPTY_METHODS_IN_ONE_LINE: true,
            FORCE_EMPTY_CLASSES_IN_ONE_LINE: true,
            METHOD_CALL_CHAIN_WRAP: 0,
            COMMA_AFTER_LAST_ARRAY_ELEMENT: true,
            COMMA_AFTER_LAST_ARGUMENT: true,
            COMMA_AFTER_LAST_PARAMETER: true,
            COMMA_AFTER_LAST_CLOSURE_USE_VAR: true,
            COMMA_AFTER_LAST_MATCH_ARM: true,
            FORCE_SHORT_DECLARATION_ARRAY_STYLE: true,
            ..Self::psr12()
        }
    }

    /// PER Coding Style 3.0 (the values of PhpStorm's predefined style).
    pub fn per_cs3() -> Self {
        FormatOptions {
            FORCE_EMPTY_METHODS_IN_ONE_LINE: true,
            FORCE_EMPTY_CLASSES_IN_ONE_LINE: true,
            SPACE_AFTER_TYPE_CAST: true,
            COMMA_AFTER_LAST_ARRAY_ELEMENT: true,
            COMMA_AFTER_LAST_ARGUMENT: true,
            COMMA_AFTER_LAST_PARAMETER: true,
            COMMA_AFTER_LAST_CLOSURE_USE_VAR: true,
            COMMA_AFTER_LAST_MATCH_ARM: true,
            FORCE_SHORT_DECLARATION_ARRAY_STYLE: true,
            ..Self::psr12()
        }
    }

    /// PhpStorm's Laravel style (the values of PhpStorm's predefined style).
    pub fn laravel() -> Self {
        FormatOptions {
            KEEP_BLANK_LINES_BEFORE_RBRACE: 2,
            KEEP_BLANK_LINES_AFTER_LBRACE: 2,
            BLANK_LINES_AFTER_OPENING_TAG: 0,
            BLANK_LINES_BETWEEN_IMPORTS: 0,
            NEW_LINE_AFTER_PHP_OPENING_TAG: false,
            CALL_PARAMETERS_WRAP: 1,
            CALL_PARAMETERS_LPAREN_ON_NEXT_LINE: false,
            CALL_PARAMETERS_RPAREN_ON_NEXT_LINE: false,
            ARRAY_INITIALIZER_WRAP: 1,
            METHOD_CALL_CHAIN_WRAP: 0,
            PHPDOC_PARAM_SPACES_BETWEEN_TAG_AND_TYPE: 2,
            PHPDOC_PARAM_SPACES_BETWEEN_TYPE_AND_NAME: 2,
            PHPDOC_PARAM_SPACES_BETWEEN_NAME_AND_DESCRIPTION: 2,
            CONCAT_SPACES: false,
            SPACE_AFTER_TYPE_CAST: true,
            ..Self::psr12()
        }
    }

    /// PhpStorm's Symfony style (the values of PhpStorm's predefined style).
    pub fn symfony() -> Self {
        FormatOptions {
            KEEP_BLANK_LINES_BEFORE_RBRACE: 2,
            KEEP_BLANK_LINES_AFTER_LBRACE: 2,
            BLANK_LINES_AFTER_OPENING_TAG: 0,
            BLANK_LINES_BETWEEN_IMPORTS: 0,
            BLANK_LINES_BEFORE_RETURN_STATEMENT: 1,
            NEW_LINE_AFTER_PHP_OPENING_TAG: false,
            CONCAT_SPACES: false,
            COMMA_AFTER_LAST_ARRAY_ELEMENT: true,
            ..Self::psr12()
        }
    }

    /// PEAR (the values of PhpStorm's predefined style).
    pub fn pear() -> Self {
        FormatOptions {
            INDENT_CASE_FROM_SWITCH: false,
            KEEP_BLANK_LINES_BEFORE_RBRACE: 2,
            KEEP_BLANK_LINES_AFTER_LBRACE: 2,
            BLANK_LINES_AFTER_OPENING_TAG: 0,
            BLANK_LINES_AFTER_PACKAGE: 0,
            BLANK_LINES_BETWEEN_IMPORTS: 0,
            NEW_LINE_AFTER_PHP_OPENING_TAG: false,
            KEEP_RPAREN_AND_LBRACE_ON_ONE_LINE: false,
            ELSE_IF_STYLE: "AS_IS".to_string(),
            RIGHT_MARGIN: 80,
            CALL_PARAMETERS_WRAP: 1,
            METHOD_PARAMETERS_WRAP: 1,
            ARRAY_INITIALIZER_WRAP: 1,
            ALIGN_PHPDOC_PARAM_NAMES: true,
            ALIGN_PHPDOC_COMMENTS: true,
            PHPDOC_BLANK_LINE_BEFORE_TAGS: true,
            PHPDOC_BLANK_LINES_AROUND_PARAMETERS: true,
            ..Self::psr12()
        }
    }

    /// Zend Framework (the values of PhpStorm's predefined style).
    pub fn zend() -> Self {
        FormatOptions {
            KEEP_BLANK_LINES_BEFORE_RBRACE: 2,
            KEEP_BLANK_LINES_AFTER_LBRACE: 2,
            BLANK_LINES_AFTER_OPENING_TAG: 0,
            BLANK_LINES_AFTER_PACKAGE: 0,
            BLANK_LINES_BETWEEN_IMPORTS: 0,
            NEW_LINE_AFTER_PHP_OPENING_TAG: false,
            ELSE_IF_STYLE: "AS_IS".to_string(),
            RIGHT_MARGIN: 80,
            CALL_PARAMETERS_WRAP: 1,
            METHOD_PARAMETERS_WRAP: 1,
            METHOD_PARAMETERS_LPAREN_ON_NEXT_LINE: false,
            ARRAY_INITIALIZER_WRAP: 1,
            ARRAY_INITIALIZER_LBRACE_ON_NEXT_LINE: false,
            ARRAY_INITIALIZER_RBRACE_ON_NEXT_LINE: false,
            ALIGN_PHPDOC_PARAM_NAMES: true,
            ALIGN_PHPDOC_COMMENTS: true,
            PHPDOC_BLANK_LINE_BEFORE_TAGS: true,
            PHPDOC_BLANK_LINES_AROUND_PARAMETERS: true,
            ALIGN_KEY_VALUE_PAIRS: true,
            ALIGN_MULTILINE_ARRAY_INITIALIZER_EXPRESSION: true,
            ..Self::psr12()
        }
    }

    /// CodeIgniter (the values of PhpStorm's predefined style).
    pub fn codeigniter() -> Self {
        FormatOptions {
            USE_TAB_CHARACTER: true,
            KEEP_BLANK_LINES_BEFORE_RBRACE: 2,
            KEEP_BLANK_LINES_AFTER_LBRACE: 2,
            BLANK_LINES_AFTER_OPENING_TAG: 0,
            BLANK_LINES_AFTER_PACKAGE: 0,
            BLANK_LINES_BETWEEN_IMPORTS: 0,
            NEW_LINE_AFTER_PHP_OPENING_TAG: false,
            CLASS_BRACE_STYLE: 1,
            BRACE_STYLE: 2,
            KEEP_RPAREN_AND_LBRACE_ON_ONE_LINE: false,
            IF_BRACE_FORCE: 0,
            FOR_BRACE_FORCE: 0,
            WHILE_BRACE_FORCE: 0,
            DOWHILE_BRACE_FORCE: 0,
            ELSE_IF_STYLE: "AS_IS".to_string(),
            CALL_PARAMETERS_WRAP: 0,
            CALL_PARAMETERS_LPAREN_ON_NEXT_LINE: false,
            CALL_PARAMETERS_RPAREN_ON_NEXT_LINE: false,
            METHOD_PARAMETERS_WRAP: 0,
            METHOD_PARAMETERS_LPAREN_ON_NEXT_LINE: false,
            METHOD_PARAMETERS_RPAREN_ON_NEXT_LINE: false,
            ARRAY_INITIALIZER_WRAP: 0,
            ARRAY_INITIALIZER_LBRACE_ON_NEXT_LINE: false,
            ARRAY_INITIALIZER_RBRACE_ON_NEXT_LINE: false,
            METHOD_CALL_CHAIN_WRAP: 0,
            SPACE_BEFORE_UNARY_NOT: true,
            SPACE_AFTER_UNARY_NOT: true,
            LOWER_CASE_BOOLEAN_CONST: false,
            LOWER_CASE_NULL_CONST: false,
            UPPER_CASE_BOOLEAN_CONST: true,
            UPPER_CASE_NULL_CONST: true,
            ..Self::psr12()
        }
    }

    /// Option values that differ from `base`, as `(name, value)` text.
    pub fn diff(&self, base: &FormatOptions) -> Vec<(&'static str, String)> {
        Self::KEYS
            .iter()
            .filter_map(|&k| {
                let v = self.get(k)?;
                (base.get(k).as_ref() != Some(&v)).then_some((k, v))
            })
            .collect()
    }

    /// Indentation of `columns` columns as text.
    pub fn indent_str(&self, columns: usize) -> String {
        if self.USE_TAB_CHARACTER && self.TAB_SIZE > 0 {
            "\t".repeat(columns / self.TAB_SIZE) + &" ".repeat(columns % self.TAB_SIZE)
        } else {
            " ".repeat(columns)
        }
    }

    /// Width of leading whitespace in columns (tabs advance to the next tab stop).
    pub fn width(&self, ws: &str) -> usize {
        let tab = self.TAB_SIZE.max(1);
        ws.chars().fold(0, |w, c| if c == '\t' { (w / tab + 1) * tab } else { w + 1 })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_and_set_by_name() {
        let mut o = FormatOptions::psr12();
        assert_eq!(o.get("INDENT_SIZE").as_deref(), Some("4"));
        assert!(o.set("USE_TAB_CHARACTER", "true"));
        assert!(o.USE_TAB_CHARACTER);
        assert!(!o.set("INDENT_SIZE", "many"));
        assert!(!o.set("NO_SUCH_OPTION", "1"));
        assert!(FormatOptions::KEYS.contains(&"KEEP_BLANK_LINES_IN_CODE"));
        assert_eq!(o.indent_str(6), "\t  ");
        assert_eq!(o.width("\t  "), 6);
    }
}
