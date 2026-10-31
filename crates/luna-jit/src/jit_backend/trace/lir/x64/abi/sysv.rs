//! System V calling convention registers.

pub(in super::super) const CALLER: &[u8] = &[6, 7, 8, 9];
pub(in super::super) const CALLEE: &[u8] = &[3, 5, 12, 13, 14, 15];
pub(in super::super) const ARGS: &[u8] = &[7, 6, 2, 1, 8, 9];
pub(in super::super) const FARGS: &[u8] = &[0, 1, 2, 3, 4, 5, 6, 7];
pub(in super::super) const FCALLER: &[u8] = &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12];
pub(in super::super) const FSCRATCH: [u8; 2] = [14, 15];
pub(in super::super) const XTMP: u8 = 13;
pub(in super::super) const SHADOW: u32 = 0;
pub(in super::super) const POSITIONAL: bool = false;
