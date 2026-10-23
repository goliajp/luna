//! Full userdata as the C API makes them (PUC `lua_newuserdatauv`): a
//! block of raw memory the host owns the contents of, and the userdata's
//! user values.

use super::*;
use crate::runtime::mem::{BlockKind, LAny, LVec, Oom};
use crate::runtime::userdata::UserdataPayload;
use std::alloc::Layout;

/// PUC `LUAI_MAXALIGN` rounded up: every block starts on this boundary.
const BLOCK_ALIGN: usize = 16;

/// The raw memory and user values of a userdata made by the C API.
pub struct HostBlock {
    mem: *mut u8,
    size: usize,
    /// The user values (`lua_getiuservalue`); one in 5.2 and 5.3, as many
    /// as the host asked for from 5.4 on.
    pub uservalues: LVec<Value>,
}

impl HostBlock {
    fn layout(size: usize) -> Layout {
        Layout::from_size_align(size.max(1), BLOCK_ALIGN).expect("userdata size overflows layout")
    }

    /// The block's first byte.
    pub fn ptr(&self) -> *mut u8 {
        self.mem
    }

    /// The block's size in bytes.
    pub fn size(&self) -> usize {
        self.size
    }
}

impl Drop for HostBlock {
    fn drop(&mut self) {
        // SAFETY: `mem` came from the context `uservalues` allocates through,
        // with this layout, in `host_new_block`, and only this drop frees it
        unsafe {
            self.uservalues.mem().ctx().free(
                std::ptr::NonNull::new_unchecked(self.mem),
                Self::layout(self.size),
            )
        };
    }
}

fn trace_block(any: &crate::runtime::mem::LAny, m: &mut crate::vm::UserdataMarker<'_>) {
    if let Some(b) = any.downcast_ref::<HostBlock>() {
        for &v in &b.uservalues {
            m.mark_value(v);
        }
    }
}

impl Vm {
    /// A new full userdata of `size` bytes, zeroed, with `nuv` nil user
    /// values and no metatable; a memory error when it cannot be made, after
    /// an emergency collection.
    pub(crate) fn host_new_block(
        &mut self,
        size: usize,
        nuv: usize,
    ) -> Result<Gc<crate::runtime::Userdata>, LuaError> {
        self.retry_oom(|vm| vm.try_new_block(size, nuv))
    }

    /// `Vm::host_new_block`'s allocations; what was allocated before a
    /// failure is freed again.
    fn try_new_block(
        &mut self,
        size: usize,
        nuv: usize,
    ) -> Result<Gc<crate::runtime::Userdata>, Oom> {
        let layout = HostBlock::layout(size);
        let mem = match self.heap.mem_ctx().alloc_zeroed(layout, BlockKind::Other) {
            Some(p) => p.as_ptr(),
            None => return Err(Oom(self.heap.mem())),
        };
        // from here the block's drop frees `mem`
        let mut block = HostBlock {
            mem,
            size,
            uservalues: LVec::new(self.heap.mem()),
        };
        block.uservalues.resize(nuv, Value::Nil)?;
        let payload = UserdataPayload::Host {
            type_id: std::any::TypeId::of::<HostBlock>(),
            data: LAny::new(self.heap.mem(), block, BlockKind::Other)?,
            trace_fn: Some(trace_block),
        };
        // the block and the user values count as PUC's `luaS_newudata`
        // counts them
        let puc = self
            .heap
            .puc_size(crate::runtime::puc::PucObj::udata(nuv, size));
        self.heap.try_new_userdata_sized(payload, true, puc)
    }

    /// The C API block of userdata `u`, if the C API made it.
    pub(crate) fn host_block(&self, u: Gc<crate::runtime::Userdata>) -> Option<&HostBlock> {
        // SAFETY: the caller holds `u`, and nothing can collect it while
        // `&self` is borrowed; the reference only reads the payload
        let ud: &crate::runtime::Userdata = unsafe { &*u.as_ptr() };
        match &ud.payload {
            UserdataPayload::Host { data, .. } => data.downcast_ref::<HostBlock>(),
            _ => None,
        }
    }

    /// The address C sees for userdata `u` (`lua_touserdata`): its block,
    /// or the object itself for a userdata the C API did not make.
    pub(crate) fn host_userdata_ptr(&self, u: Gc<crate::runtime::Userdata>) -> *mut u8 {
        match self.host_block(u) {
            Some(b) => b.ptr(),
            None => u.as_ptr().cast(),
        }
    }

    /// User value `n` (from 1) of `u`; `None` when it has no such value.
    /// A userdata the C API did not make has the one 5.2/5.3 slot, and none
    /// from 5.4 on (PUC's library userdata are made with no user values).
    pub(crate) fn host_uservalue(
        &self,
        u: Gc<crate::runtime::Userdata>,
        n: usize,
    ) -> Option<Value> {
        match self.host_block(u) {
            Some(b) => b.uservalues.get(n.checked_sub(1)?).copied(),
            None => (n == 1 && self.version <= LuaVersion::Lua53).then_some(u.user_value),
        }
    }

    /// Set user value `n` (from 1) of `u`; `false` when it has no such
    /// value.
    pub(crate) fn host_set_uservalue(
        &mut self,
        u: Gc<crate::runtime::Userdata>,
        n: usize,
        v: Value,
    ) -> bool {
        let Some(i) = n.checked_sub(1) else {
            return false;
        };
        // SAFETY: `u` is held by the caller; no other reference into it is
        // live, and the borrow covers one store
        let ud = unsafe { u.as_mut() };
        let done = match &mut ud.payload {
            UserdataPayload::Host { data, .. } => match data.downcast_mut::<HostBlock>() {
                Some(b) => b.uservalues.get_mut(i).map(|slot| *slot = v).is_some(),
                None => false,
            },
            _ if i == 0 && self.version <= LuaVersion::Lua53 => {
                ud.user_value = v;
                true
            }
            _ => false,
        };
        if done {
            self.heap.barrier_back(u);
        }
        done
    }
}
