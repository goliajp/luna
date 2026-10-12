//! The strings `lua_getinfo` hands out.

use super::super::*;
use luna_core::runtime::mem::{LSlice, LVec, MemRef};

/// The strings `lua_getinfo` hands out, kept as long as the thread whose
/// call returned them, in blocks of the state's allocation context: PUC's
/// point into strings of the function, which live as long as it does.
/// The default has no context and holds nothing (a thread's own is made
/// with [`CStrings::new`]).
#[derive(Default)]
pub(in super::super) struct CStrings(Option<LVec<LSlice<u8>>>);

impl CStrings {
    pub(in super::super) fn new(mem: MemRef) -> CStrings {
        CStrings(Some(LVec::new(mem)))
    }

    /// `b` NUL-terminated, at an address that stays put; `"?"` when there
    /// is no memory to keep it (the functions handing it out cannot raise).
    pub(in super::super) fn get(&mut self, b: &[u8]) -> *const c_char {
        let Some(list) = &mut self.0 else {
            return c"?".as_ptr();
        };
        if let Some(s) = list.iter().find(|s| s[..s.len() - 1] == *b) {
            return s.as_ptr().cast();
        }
        let mem = list.mem();
        let made = (|| {
            let mut z = LVec::with_capacity(mem, b.len() + 1)?;
            z.extend_from_slice(b)?;
            z.push(0)?;
            list.reserve(1)?;
            Ok::<_, luna_core::runtime::mem::Oom>(z.into_slice())
        })();
        match made {
            Ok(s) => {
                let p = s.as_ptr().cast();
                list.push_or_abort(s);
                p
            }
            Err(_) => c"?".as_ptr(),
        }
    }
}
