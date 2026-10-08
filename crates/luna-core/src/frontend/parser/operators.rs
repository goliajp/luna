//! The operators of a token, and their priorities.

use crate::frontend::ast::{BinOp, UnOp};
use crate::frontend::token::{Tok, Token};

/// Binary operator priorities from lparser.c (left, right); right < left
/// means right-associative.
pub(super) fn bin_priority(op: BinOp) -> (u8, u8) {
    match op {
        BinOp::Or => (1, 1),
        BinOp::And => (2, 2),
        BinOp::Lt | BinOp::Gt | BinOp::Le | BinOp::Ge | BinOp::Ne | BinOp::Eq => (3, 3),
        BinOp::BOr => (4, 4),
        BinOp::BXor => (5, 5),
        BinOp::BAnd => (6, 6),
        BinOp::Shl | BinOp::Shr => (7, 7),
        BinOp::Concat => (9, 8),
        BinOp::Add | BinOp::Sub => (10, 10),
        BinOp::Mul | BinOp::Div | BinOp::IDiv | BinOp::Mod => (11, 11),
        BinOp::Pow => (14, 13),
    }
}

pub(super) const UNARY_PRIORITY: u8 = 12;

pub(super) fn bin_op_of(tok: &Tok) -> Option<BinOp> {
    Some(match tok {
        Token::Plus => BinOp::Add,
        Token::Minus => BinOp::Sub,
        Token::Star => BinOp::Mul,
        Token::Slash => BinOp::Div,
        Token::DSlash => BinOp::IDiv,
        Token::Percent => BinOp::Mod,
        Token::Caret => BinOp::Pow,
        Token::Concat => BinOp::Concat,
        Token::Eq => BinOp::Eq,
        Token::Ne => BinOp::Ne,
        Token::Lt => BinOp::Lt,
        Token::Le => BinOp::Le,
        Token::Gt => BinOp::Gt,
        Token::Ge => BinOp::Ge,
        Token::And => BinOp::And,
        Token::Or => BinOp::Or,
        Token::Amp => BinOp::BAnd,
        Token::Pipe => BinOp::BOr,
        Token::Tilde => BinOp::BXor,
        Token::Shl => BinOp::Shl,
        Token::Shr => BinOp::Shr,
        _ => return None,
    })
}

pub(super) fn un_op_of(tok: &Tok) -> Option<UnOp> {
    Some(match tok {
        Token::Minus => UnOp::Neg,
        Token::Not => UnOp::Not,
        Token::Hash => UnOp::Len,
        Token::Tilde => UnOp::BNot,
        _ => return None,
    })
}
