//! Source → AST frontend (P01). Syntax only: scope resolution, constant
//! folding and code generation live in later phases.

pub mod ast;
pub mod error;
pub mod lexer;
pub mod macro_expander;
pub mod parser;
pub mod span;
pub mod token;

pub use error::SyntaxError;
pub use parser::{parse, parse_tokens};
