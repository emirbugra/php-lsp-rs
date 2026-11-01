//! PhpStorm code style schemes: read and write the XML that PhpStorm exports (*Settings → Editor →
//! Code Style → PHP → Export*) and keeps per project in `.idea/codeStyles/Project.xml`.
//!
//! A scheme only lists the values that differ from PhpStorm's defaults, so reading starts from
//! [`FormatOptions::phpstorm`]. Options live in different parts of the file:
//!
//! ```xml
//! <code_scheme name="Project" version="173">
//!   <option name="LINE_SEPARATOR" value="&#10;" />          <!-- root: all languages -->
//!   <PHPCodeStyleSettings>                                  <!-- PHP-only options -->
//!     <option name="CONCAT_SPACES" value="false" />
//!   </PHPCodeStyleSettings>
//!   <codeStyleSettings language="PHP">                      <!-- common options for PHP -->
//!     <option name="KEEP_BLANK_LINES_IN_CODE" value="1" />
//!     <indentOptions>
//!       <option name="INDENT_SIZE" value="2" />
//!     </indentOptions>
//!   </codeStyleSettings>
//! </code_scheme>
//! ```

use crate::options::FormatOptions;

/// Options that PhpStorm keeps in `<PHPCodeStyleSettings>` (all fields of its
/// `PhpCodeStyleSettings` class, also those the formatter does not support yet).
const PHP_SECTION: &[&str] = &[
    "COMMA_AFTER_LAST_CLOSURE_USE_VAR_STYLE",
    "COMMA_AFTER_LAST_PARAMETER_STYLE",
    "COMMA_AFTER_LAST_ARGUMENT_STYLE",
    "SORT_PHPDOC_ELEMENTS",
    "LINK_WEIGHT",
    "THROWS_WEIGHT",
    "PARAM_WEIGHT",
    "RETURN_WEIGHT",
    "AUTHOR_WEIGHT",
    "INTERNAL_WEIGHT",
    "USES_WEIGHT",
    "VERSION_WEIGHT",
    "API_WEIGHT",
    "CATEGORY_WEIGHT",
    "COPYRIGHT_WEIGHT",
    "DEPRECATED_WEIGHT",
    "EXAMPLE_WEIGHT",
    "FILESOURCE_WEIGHT",
    "GLOBAL_WEIGHT",
    "IGNORE_WEIGHT",
    "LICENSE_WEIGHT",
    "METHOD_WEIGHT",
    "PACKAGE_WEIGHT",
    "PROPERTY_WEIGHT",
    "PROPERTY_READ_WEIGHT",
    "PROPERTY_WRITE_WEIGHT",
    "SEE_WEIGHT",
    "SINCE_WEIGHT",
    "SUBPACKAGE_WEIGHT",
    "TODO_WEIGHT",
    "VAR_WEIGHT",
    "UNKNOWN_TAG_WEIGHT",
    "INDENT_CODE_IN_PHP_TAGS",
    "ALIGN_KEY_VALUE_PAIRS",
    "ALIGN_PHPDOC_PARAM_NAMES",
    "ALIGN_PHPDOC_COMMENTS",
    "ALIGN_ASSIGNMENTS",
    "ALIGN_INLINE_COMMENTS",
    "ALIGN_NAMED_ARGUMENTS",
    "ALIGN_MATCH_ARM_BODIES",
    "CONCAT_SPACES",
    "COMMA_AFTER_LAST_ARRAY_ELEMENT",
    "COMMA_AFTER_LAST_PARAMETER",
    "COMMA_AFTER_LAST_CLOSURE_USE_VAR",
    "COMMA_AFTER_LAST_ARGUMENT",
    "COMMA_AFTER_LAST_MATCH_ARM",
    "PHPDOC_BLANK_LINE_BEFORE_TAGS",
    "PHPDOC_KEEP_BLANK_LINES",
    "PHPDOC_BLANK_LINES_AROUND_PARAMETERS",
    "PHPDOC_WRAP_LONG_LINES",
    "BLANK_LINES_BETWEEN_IMPORTS",
    "PHPDOC_PARAM_SPACES_BETWEEN_TAG_AND_TYPE",
    "PHPDOC_PARAM_SPACES_BETWEEN_TYPE_AND_NAME",
    "PHPDOC_PARAM_SPACES_BETWEEN_NAME_AND_DESCRIPTION",
    "ANONYMOUS_BRACE_STYLE",
    "IMPORT_SORTING",
    "GROUP_USE_WRAP",
    "UPPER_CASE_BOOLEAN_CONST",
    "UPPER_CASE_NULL_CONST",
    "LOWER_CASE_BOOLEAN_CONST",
    "LOWER_CASE_NULL_CONST",
    "ELSE_IF_STYLE",
    "LOWER_CASE_KEYWORDS",
    "BLANK_LINE_BEFORE_RETURN_STATEMENT",
    "BLANK_LINES_BEFORE_RETURN_STATEMENT",
    "KEEP_RPAREN_AND_LBRACE_ON_ONE_LINE",
    "SPACES_AROUND_VAR_WITHIN_BRACKETS",
    "ALIGN_CLASS_CONSTANTS",
    "ALIGN_ENUM_CASES",
    "BLANK_LINES_AROUND_CONSTANTS",
    "BLANK_LINES_AROUND_ENUM_CASES",
    "BLANK_LINES_AFTER_OPENING_TAG",
    "BLANK_LINES_AFTER_FUNCTION",
    "KEEP_BLANK_LINES_AFTER_LBRACE",
    "SPACE_BEFORE_UNARY_NOT",
    "SPACE_AFTER_UNARY_NOT",
    "SPACE_BETWEEN_TERNARY_QUEST_AND_COLON",
    "SPACES_WITHIN_SHORT_ECHO_TAGS",
    "SPACE_BEFORE_CLOSURE_LEFT_PARENTHESIS",
    "SPACE_BEFORE_SHORT_CLOSURE_LEFT_PARENTHESIS",
    "FORCE_SHORT_DECLARATION_ARRAY_STYLE",
    "NEW_LINE_AFTER_PHP_OPENING_TAG",
    "TREAT_MULTILINE_ARRAYS_AND_LAMBDAS_MULTILINE",
    "SPACES_AROUND_ARROW",
    "SPACES_AROUND_NULL_COALESCE_OPERATOR",
    "SPACE_AROUND_ASSIGNMENT_IN_DECLARE",
    "SPACE_BEFORE_COLON_IN_RETURN_TYPE",
    "SPACE_AFTER_COLON_IN_RETURN_TYPE",
    "SPACE_BEFORE_COLON_IN_NAMED_ARGUMENT",
    "SPACE_AFTER_COLON_IN_NAMED_ARGUMENT",
    "SPACES_AROUND_PIPE_IN_UNION_TYPE",
    "SPACES_AROUND_AMPERSAND_IN_INTERSECTION_TYPE",
    "SPACES_WITHIN_COMPOUND_TYPE_PARENS",
    "UNION_TYPE_WRAP",
    "RETURN_TYPE_ON_NEW_LINE",
    "SPACE_BEFORE_COLON_IN_ENUM_BACKED_TYPE",
    "SPACE_AFTER_COLON_IN_ENUM_BACKED_TYPE",
    "FORCE_EMPTY_METHODS_IN_ONE_LINE",
    "FORCE_EMPTY_CLASSES_IN_ONE_LINE",
    "MULTILINE_CHAINED_CALLS_SEMICOLON_ON_NEW_LINE",
    "MULTILINE_CHAINED_CALLS_FIRST_CALL_ON_NEW_LINE",
    "MULTILINE_CLOSURE_LAMBDA_ON_NEW_LINE",
    "HEREDOC_ON_SAME_LINE",
    "NORMALIZE_CAST_PAREN_SPACING",
    "COLLAPSE_LINE_BREAK_AFTER_TYPE_CAST",
    "NAMESPACE_BRACE_STYLE",
    "IF_LPAREN_ON_NEXT_LINE",
    "IF_RPAREN_ON_NEXT_LINE",
    "ATTRIBUTES_WRAP",
    "PARAMETERS_ATTRIBUTES_WRAP",
];

/// Options in `<indentOptions>`.
const INDENT_SECTION: &[&str] =
    &["INDENT_SIZE", "CONTINUATION_INDENT_SIZE", "TAB_SIZE", "USE_TAB_CHARACTER", "KEEP_INDENTS_ON_EMPTY_LINES"];

/// Options at the root of the scheme (they apply to every language).
const ROOT_SECTION: &[&str] = &["LINE_SEPARATOR", "FORMATTER_TAGS_ENABLED", "FORMATTER_OFF_TAG", "FORMATTER_ON_TAG"];

/// Editor settings in PhpStorm, not part of a code style scheme (`.editorconfig` has them).
const EDITOR_ONLY: &[&str] = &["STRIP_TRAILING_SPACES", "ENSURE_NEWLINE_AT_EOF"];

/// Where an option is stored in a PhpStorm scheme.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Section {
    Root,
    Php,
    Common,
    Indent,
    /// Not in schemes at all.
    Editor,
}

/// The part of a scheme that holds option `name`.
pub fn section_of(name: &str) -> Section {
    if ROOT_SECTION.contains(&name) {
        Section::Root
    } else if INDENT_SECTION.contains(&name) {
        Section::Indent
    } else if PHP_SECTION.contains(&name) {
        Section::Php
    } else if EDITOR_ONLY.contains(&name) {
        Section::Editor
    } else {
        Section::Common
    }
}

/// Result of reading a scheme or an `.editorconfig`.
#[derive(Clone, Debug)]
pub struct Imported {
    pub options: FormatOptions,
    /// Scheme name (`<code_scheme name="…">`), if any.
    pub name: Option<String>,
    /// Option names found that the formatter does not support (yet), in file order.
    pub unsupported: Vec<String>,
    /// Supported options whose value could not be used (`NAME = value`).
    pub invalid: Vec<String>,
}

#[derive(Debug)]
struct Element {
    name: String,
    attrs: Vec<(String, String)>,
    children: Vec<Element>,
}

impl Element {
    fn attr(&self, key: &str) -> Option<&str> {
        self.attrs.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    fn find<'a>(&'a self, name: &str) -> Option<&'a Element> {
        if self.name == name {
            return Some(self);
        }
        self.children.iter().find_map(|c| c.find(name))
    }
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let Some(end) = rest.find(';') else { break };
        let entity = &rest[1..end];
        let ch = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ if entity.starts_with("#x") => u32::from_str_radix(&entity[2..], 16).ok().and_then(char::from_u32),
            _ if entity.starts_with('#') => entity[1..].parse().ok().and_then(char::from_u32),
            _ => None,
        };
        match ch {
            Some(c) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\n' => out.push_str("&#10;"),
            '\r' => out.push_str("&#13;"),
            _ => out.push(c),
        }
    }
    out
}

/// A small XML reader: elements and attributes; text, comments, `<?xml?>` and `<!DOCTYPE>` are
/// skipped. Enough for code style files.
fn parse_xml(text: &str) -> Result<Element, String> {
    let mut stack = vec![Element { name: String::new(), attrs: Vec::new(), children: Vec::new() }];
    let mut rest = text;
    while let Some(i) = rest.find('<') {
        rest = &rest[i..];
        if let Some(r) = rest.strip_prefix("<!--") {
            rest = r.find("-->").map_or("", |e| &r[e + 3..]);
            continue;
        }
        if rest.starts_with("<?") || rest.starts_with("<!") {
            rest = rest.find('>').map_or("", |e| &rest[e + 1..]);
            continue;
        }
        // find the end of the tag, outside quoted attribute values
        let mut quote = None;
        let mut end = None;
        for (j, c) in rest.char_indices().skip(1) {
            match (quote, c) {
                (None, '"' | '\'') => quote = Some(c),
                (Some(q), c) if c == q => quote = None,
                (None, '>') => {
                    end = Some(j);
                    break;
                }
                _ => {}
            }
        }
        let end = end.ok_or("unterminated tag")?;
        let tag = &rest[1..end];
        rest = &rest[end + 1..];
        if let Some(name) = tag.strip_prefix('/') {
            let el = stack.pop().ok_or("unbalanced tags")?;
            if el.name != name.trim() || stack.is_empty() {
                return Err(format!("unexpected </{}>", name.trim()));
            }
            stack.last_mut().expect("checked").children.push(el);
            continue;
        }
        let self_closing = tag.ends_with('/');
        let tag = tag.trim_end_matches('/');
        let name_end = tag.find(|c: char| c.is_whitespace()).unwrap_or(tag.len());
        let mut el = Element { name: tag[..name_end].to_string(), attrs: Vec::new(), children: Vec::new() };
        let mut a = tag[name_end..].trim_start();
        while let Some(eq) = a.find('=') {
            let key = a[..eq].trim().to_string();
            let v = a[eq + 1..].trim_start();
            let q = v.chars().next().filter(|c| *c == '"' || *c == '\'').ok_or("unquoted attribute")?;
            let close = v[1..].find(q).ok_or("unterminated attribute")? + 1;
            el.attrs.push((key, unescape(&v[1..close])));
            a = v[close + 1..].trim_start();
        }
        if self_closing {
            stack.last_mut().expect("root").children.push(el);
        } else {
            stack.push(el);
        }
    }
    if stack.len() != 1 {
        return Err("unclosed tags".into());
    }
    Ok(stack.pop().expect("root"))
}

/// Apply `NAME = value` to `o`, recording names that are not supported and values that are not usable.
pub(crate) fn apply(
    o: &mut FormatOptions,
    name: &str,
    value: &str,
    unsupported: &mut Vec<String>,
    invalid: &mut Vec<String>,
) {
    // a language's `RIGHT_MARGIN` of -1 means "the general one"
    if name == "RIGHT_MARGIN" && value.trim() == "-1" {
        return;
    }
    if FormatOptions::KEYS.contains(&name) {
        if !o.set(name, value) {
            invalid.push(format!("{name} = {value}"));
        }
    } else if !unsupported.iter().any(|u| u == name) {
        unsupported.push(name.to_string());
    }
}

impl FormatOptions {
    /// Read a PhpStorm code style scheme (an exported scheme or `.idea/codeStyles/Project.xml`).
    pub fn from_phpstorm_xml(text: &str) -> Result<Imported, String> {
        let root = parse_xml(text)?;
        let scheme = root.find("code_scheme").ok_or("no <code_scheme> element: not a PhpStorm code style file")?;
        let mut o = FormatOptions::phpstorm();
        let (mut unsupported, mut invalid) = (Vec::new(), Vec::new());
        let options = |el: &Element| -> Vec<(String, String)> {
            el.children
                .iter()
                .filter(|c| c.name == "option")
                .filter_map(|c| Some((c.attr("name")?.to_string(), c.attr("value")?.to_string())))
                .collect()
        };
        // root options first (older schemes keep common options there too), then the PHP parts
        for (name, value) in options(scheme) {
            apply(&mut o, &name, &value, &mut unsupported, &mut invalid);
        }
        for el in &scheme.children {
            let php = match el.name.as_str() {
                "PHPCodeStyleSettings" => true,
                "codeStyleSettings" => el.attr("language") == Some("PHP"),
                _ => false,
            };
            if !php {
                continue;
            }
            for (name, value) in options(el) {
                apply(&mut o, &name, &value, &mut unsupported, &mut invalid);
            }
            for indent in el.children.iter().filter(|c| c.name == "indentOptions") {
                for (name, value) in options(indent) {
                    apply(&mut o, &name, &value, &mut unsupported, &mut invalid);
                }
            }
        }
        Ok(Imported { options: o, name: scheme.attr("name").map(str::to_string), unsupported, invalid })
    }

    /// Write these options as a PhpStorm code style scheme: like PhpStorm, only values that differ
    /// from its defaults are listed. Import it in *Settings → Editor → Code Style → PHP*.
    pub fn to_phpstorm_xml(&self, name: &str) -> String {
        let defaults = FormatOptions::phpstorm();
        let mut by_section: [Vec<String>; 4] = Default::default();
        for key in FormatOptions::KEYS {
            let (Some(value), Some(default)) = (self.get(key), defaults.get(key)) else { continue };
            if value == default {
                continue;
            }
            let slot = match section_of(key) {
                Section::Root => 0,
                Section::Php => 1,
                Section::Common => 2,
                Section::Indent => 3,
                Section::Editor => continue,
            };
            by_section[slot].push(format!("<option name=\"{key}\" value=\"{}\" />", escape(&value)));
        }
        let [root, php, common, indent] = by_section;
        let mut out = format!("<code_scheme name=\"{}\" version=\"173\">\n", escape(name));
        for line in &root {
            out += &format!("  {line}\n");
        }
        if !php.is_empty() {
            out += "  <PHPCodeStyleSettings>\n";
            for line in &php {
                out += &format!("    {line}\n");
            }
            out += "  </PHPCodeStyleSettings>\n";
        }
        if !common.is_empty() || !indent.is_empty() {
            out += "  <codeStyleSettings language=\"PHP\">\n";
            for line in &common {
                out += &format!("    {line}\n");
            }
            if !indent.is_empty() {
                out += "    <indentOptions>\n";
                for line in &indent {
                    out += &format!("      {line}\n");
                }
                out += "    </indentOptions>\n";
            }
            out += "  </codeStyleSettings>\n";
        }
        out += "</code_scheme>\n";
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xml_entities_and_comments() {
        let root = parse_xml("<?xml version='1.0'?><!-- c --><a x=\"1 &amp; &#10;\"><b y='&lt;'/></a>").unwrap();
        let a = root.find("a").unwrap();
        assert_eq!(a.attr("x"), Some("1 & \n"));
        assert_eq!(a.find("b").unwrap().attr("y"), Some("<"));
        assert!(parse_xml("<a><b></a>").is_err());
    }

    #[test]
    fn sections() {
        assert_eq!(section_of("CONCAT_SPACES"), Section::Php);
        assert_eq!(section_of("SPACE_AFTER_TYPE_CAST"), Section::Common);
        assert_eq!(section_of("INDENT_SIZE"), Section::Indent);
        assert_eq!(section_of("LINE_SEPARATOR"), Section::Root);
        assert_eq!(section_of("STRIP_TRAILING_SPACES"), Section::Editor);
    }
}
