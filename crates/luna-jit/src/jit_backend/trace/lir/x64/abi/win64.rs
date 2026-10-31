//! Win64 calling convention registers.

pub(in super::super) const CALLER: &[u8] = &[8, 9];
pub(in super::super) const CALLEE: &[u8] = &[3, 5, 6, 7, 12, 13, 14, 15];
pub(in super::super) const ARGS: &[u8] = &[1, 2, 8, 9];
pub(in super::super) const FARGS: &[u8] = &[0, 1, 2, 3];
// xmm6-15 are callee-saved, and only the low halves would be saved
pub(in super::super) const FCALLER: &[u8] = &[0, 1, 2];
pub(in super::super) const FSCRATCH: [u8; 2] = [4, 5];
pub(in super::super) const XTMP: u8 = 3;
pub(in super::super) const SHADOW: u32 = 32;
pub(in super::super) const POSITIONAL: bool = true;
