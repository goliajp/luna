//! Full userdata as the C API makes them (PUC `lua_newuserdatauv`): a
//! block of raw memory the host owns the contents of, and the userdata's
//! user values.

use super::*;
use crate::runtime::userdata::UserdataPayload;
use std::alloc::Layout;

/// PUC `LUAI_MAXALIGN` rounded up: every block starts on this boundary.
const BLOCK_ALIGN: usize = 16;

/// The raw memory and user values of a userdata made by the C API.
pub struct HostBlock {
    mem: *mut u8,
    /// the allocation context the block came from
    ctx: crate::runtime::mem::MemRef,
    size: usize,
    /// The user values (`lua_getiuservalue`); one in 5.2 and 5.3, as many
    /// as the host asked for from 5.4 on.
    pub uservalues: Vec<Value>,
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
        // SAFETY: `mem` came from `ctx` with this layout in `host_new_block`,
        // and only this drop frees it
        unsafe {
            self.ctx.ctx().free(
                std::ptr::NonNull::new_unchecked(self.mem),
                Self::layout(self.size),
            )
        };
    }
}

fn trace_block(any: &(dyn std::any::Any + 'static), m: &mut crate::vm::UserdataMarker<'_>) {
    if let Some(b) = any.downcast_ref::<HostBlock>() {
        for &v in &b.uservalues {
            m.mark_value(v);
        }
    }
}

#[doc(hidden)]
impl Vm {
    /// A new full userdata of `size` bytes, zeroed, with `nuv` nil user
    /// values and no metatable.
    pub fn host_new_block(&mut self, size: usize, nuv: usize) -> Gc<crate::runtime::Userdata> {
        let layout = HostBlock::layout(size);
        let ctx = self.heap.mem();
        let mem = match ctx.ctx().alloc(layout, crate::runtime::mem::BlockKind::Other) {
            Some(p) => p.as_ptr(),
            None => std::alloc::handle_alloc_error(layout),
        };
        // SAFETY: `mem` is a fresh block of `layout.size()` bytes
        unsafe { mem.write_bytes(0, layout.size()) };
        let block = HostBlock {
            mem,
            ctx,
            size,
            uservalues: vec![Value::Nil; nuv],
        };
        let payload = UserdataPayload::Host {
            type_id: std::any::TypeId::of::<HostBlock>(),
            data: Box::new(block),
            trace_fn: Some(trace_block),
        };
        let u = self.heap.new_userdata(payload, true);
        // the block and the user values count in the heap's size, as PUC's
        // `luaS_newudata` counts them
        let extra = size.saturating_add(nuv * std::mem::size_of::<Value>());
        // SAFETY: `u` was allocated above and is held only by this local;
        // the borrow covers one store
        unsafe { u.as_mut() }.extra_bytes = extra;
        self.heap.apply_bytes_delta(0, extra);
        u
    }

    /// The C API block of userdata `u`, if the C API made it.
    pub fn host_block(&self, u: Gc<crate::runtime::Userdata>) -> Option<&HostBlock> {
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
    pub fn host_userdata_ptr(&self, u: Gc<crate::runtime::Userdata>) -> *mut u8 {
        match self.host_block(u) {
            Some(b) => b.ptr(),
            None => u.as_ptr().cast(),
        }
    }

    /// User value `n` (from 1) of `u`; `None` when it has no such value.
    /// A userdata the C API did not make has the one 5.2/5.3 slot, and none
    /// from 5.4 on (PUC's library userdata are made with no user values).
    pub fn host_uservalue(&self, u: Gc<crate::runtime::Userdata>, n: usize) -> Option<Value> {
        match self.host_block(u) {
            Some(b) => b.uservalues.get(n.checked_sub(1)?).copied(),
            None => (n == 1 && self.version <= LuaVersion::Lua53).then_some(u.user_value),
        }
    }

    /// Set user value `n` (from 1) of `u`; `false` when it has no such
    /// value.
    pub fn host_set_uservalue(
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
