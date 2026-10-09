//! System V calling convention registers.

pub(super) const CALLER: &[u8] = &[6, 7, 8, 9];
pub(super) const CALLEE: &[u8] = &[3, 5, 12, 13, 14, 15];
pub(super) const ARGS: &[u8] = &[7, 6, 2, 1, 8, 9];
pub(super) const FARGS: &[u8] = &[0, 1, 2, 3, 4, 5, 6, 7];
pub(super) const FCALLER: &[u8] = &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12];
pub(super) const FSCRATCH: [u8; 2] = [14, 15];
pub(super) const XTMP: u8 = 13;
pub(super) const SHADOW: u32 = 0;
pub(super) const POSITIONAL: bool = false;
