//! PHP lexer + error-tolerant parser. PHP 4 .. 8.x syntax.

pub mod ast;
pub mod lexer;
pub mod line_index;
pub mod parser;
pub mod span;
pub mod walk;

pub use ast::*;
pub use line_index::LineIndex;
pub use parser::{parse, Parse, ParseError};
pub use span::Span;
