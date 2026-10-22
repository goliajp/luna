//! The box of a 5.3+ `luaL_Buffer` (`newbox`): a full userdata with a
//! `__gc` metamethod, on the C function's stack until the buffer is done,
//! then garbage that the collector finalizes before freeing it.

use crate::runtime::table::Dialect;
use crate::runtime::userdata::UserdataPayload;
use crate::runtime::{Gc, LuaStr, Table, Userdata, Value};
use crate::version::LuaVersion;
use crate::vm::error::LuaError;
use crate::vm::exec::Vm;

impl Vm {
    /// `newbox`: the userdata, with the metatable PUC keeps in the registry
    /// for boxes (made the first time), registered for finalization.
    pub(crate) fn new_box(&mut self) -> Result<Gc<Userdata>, LuaError> {
        let ubox = 2 * std::mem::size_of::<usize>();
        let size = if self.heap.table_dialect >= Dialect::L54 {
            self.heap.puc.sizes.userdata(0, ubox)
        } else {
            self.heap.puc.sizes.userdata(1, ubox)
        };
        let u = self.lib_userdata(UserdataPayload::Empty, false, size)?;
        // on the stack of the running native until the buffer is done
        self.buffer_boxes
            .push((self.running_natives.len() as u32, u, 0));
        self.api_check_after_rooted()?;
        let mt = self.box_metatable()?;
        // SAFETY: `u` is a live userdata held in `buffer_boxes`; the borrow covers one store, which allocates nothing
        unsafe { u.as_mut() }.set_metatable(Some(mt));
        self.heap.barrier_forward(u, Value::Table(mt));
        self.retry_oom(|vm| vm.heap.register_finalizable_userdata(u))?;
        Ok(u)
    }

    /// The buffer whose box is `u` is done: PUC removes the box from the
    /// stack.
    pub(crate) fn drop_box(&mut self, u: Gc<Userdata>) {
        if let Some(i) = self
            .buffer_boxes
            .iter()
            .rposition(|&(_, b, _)| b.as_ptr() == u.as_ptr())
        {
            self.buffer_boxes.remove(i);
        }
    }

    /// The boxes still on the stack of a native that is running: an error
    /// leaves the others behind.
    pub(crate) fn live_boxes(&self) -> impl Iterator<Item = Gc<Userdata>> + '_ {
        let natives = self.running_natives.len() as u32;
        self.buffer_boxes
            .iter()
            .filter(move |&&(at, ..)| at <= natives)
            .map(|&(_, u, _)| u)
    }

    /// gc-trace: `R38_REFUSE` refuses the growth of a box's block (5.3
    /// on) from `old` bytes (`None`: no box yet, which is made first):
    /// the allocation function is asked directly, so the buffer's error.
    #[cfg_attr(not(feature = "gc-trace"), allow(unused_variables))]
    pub(crate) fn refuse_box(
        &mut self,
        ubox: &mut Option<Gc<Userdata>>,
        d: Dialect,
        old: Option<usize>,
        new: usize,
    ) -> Result<(), LuaError> {
        #[cfg(feature = "gc-trace")]
        if d >= Dialect::L53 && self.heap.trace.refuse_now(old.unwrap_or(0), new) {
            if old.is_none() {
                *ubox = Some(self.new_box()?);
            }
            return Err(self.buf_error());
        }
        Ok(())
    }

    /// The block of box `u`, if any, is `size` bytes now.
    pub(crate) fn box_block(&mut self, u: Option<Gc<Userdata>>, size: usize) {
        let Some(u) = u else {
            return;
        };
        if let Some(b) = self
            .buffer_boxes
            .iter_mut()
            .rfind(|b| b.1.as_ptr() == u.as_ptr())
        {
            b.2 = size;
        }
    }

    /// The slot of the box native `i` of `running_natives` made (5.4+):
    /// the placeholder `luaL_buffinit` pushed (see [`Vm::buffer_box`]), or
    /// its top when there is none. Looked for when an error closes it.
    fn box_slot(&self, i: usize) -> u32 {
        let Some(a) = self.running_natives.get(i) else {
            return 0;
        };
        let (from, top) = (a.func_slot + 1 + a.nargs, a.top());
        (from..top)
            .rev()
            .find(|&s| {
                self.stack
                    .get(s as usize)
                    .is_some_and(|v| v.raw_eq(Vm::buffer_box()))
            })
            .unwrap_or(top)
    }

    /// An error leaves the boxes of the natives above `base`: garbage,
    /// which from 5.4 on the protected call that catches the error closes
    /// (`now`: one in this thread does), innermost first, each `__close`
    /// wanting its room above the box's slot and freeing its block. The
    /// others free it when they are finalized. The error that refusing that
    /// room raises, if any.
    pub(crate) fn close_left_boxes(&mut self, base: usize, now: bool) -> Option<LuaError> {
        let now = now && self.version() >= LuaVersion::Lua54;
        let mut err = None;
        for i in (0..self.buffer_boxes.len()).rev() {
            let (at, _, size) = self.buffer_boxes[i];
            if at == u32::MAX || at as usize <= base {
                continue;
            }
            let slot = self.box_slot(at as usize - 1);
            if !now {
                self.buffer_boxes[i].0 = u32::MAX;
                continue;
            }
            self.buffer_boxes.remove(i);
            if err.is_none() {
                err = self.box_close_room_at(slot).err();
            }
            self.heap.trace_raw(size, 0);
        }
        err
    }

    /// The box metatable, made when it is not in the registry: 5.3
    /// `luaL_newmetatable(L, "LUABOX")` with `__gc`; 5.4+ a table of
    /// `__gc` and `__close` under `"_UBOX*"`.
    fn box_metatable(&mut self) -> Result<Gc<Table>, LuaError> {
        let l53 = self.heap.table_dialect == Dialect::L53;
        let name: &[u8] = if l53 { b"LUABOX" } else { b"_UBOX*" };
        let reg = self.host_registry();
        let key = self.str_value(name)?;
        if let Value::Table(mt) = reg.get(key) {
            return Ok(mt);
        }
        let mt = self.alloc_table_sized(0, 2)?;
        let mtv = Value::Table(mt);
        self.api_check_after(mtv)?;
        // `luaL_newmetatable` pushes the name again: the string looked up
        // first may have been collected at the check just made
        let r = self.rooted(&[mtv], |vm| vm.fill_box_metatable(mt, name, reg, l53));
        r.map(|()| mt)
    }

    /// The new box metatable `mt` (rooted) filled and put in registry
    /// `reg` under `name`.
    fn fill_box_metatable(
        &mut self,
        mt: Gc<Table>,
        name: &[u8],
        reg: Gc<Table>,
        l53: bool,
    ) -> Result<(), LuaError> {
        let mtv = Value::Table(mt);
        let key = self.str_value(name)?;
        // 5.3's `luaL_newmetatable` pushes the name; 5.4's `getBoxMT` only
        // keys the registry with it
        if l53 {
            self.api_check_after(key)?;
        }
        self.lib_roots.push(key);
        // `boxgc`, a light C function: no object of PUC's
        let gc = self.builtin(box_gc, &[], crate::runtime::Builtin::None)?;
        self.lib_roots.push(gc);
        if l53 {
            let n = self.str_value(b"__name")?;
            self.raw_set(mt, n, key)?;
            self.raw_set(reg, key, mtv)?;
            let k = self.str_value(b"__gc")?;
            self.raw_set(mt, k, gc)?;
        } else {
            for k in [&b"__gc"[..], b"__close"] {
                let k = self.str_value(k)?;
                self.raw_set(mt, k, gc)?;
            }
            self.raw_set(reg, key, mtv)?;
        }
        Ok(())
    }

    /// [`Vm::box_closed`] with the buffer's string `r` pushed below the
    /// call, kept by a collection that call's stack check makes.
    pub(crate) fn box_closed_over(&mut self, r: Gc<LuaStr>) -> Result<(), LuaError> {
        self.rooted(&[Value::Str(r)], |vm| vm.box_closed())
    }

    /// A box's `__close` (5.4+, `lua_closeslot`): a call one level up, of a
    /// C function, made above the box and the string just pushed with the
    /// method, the box and (5.4) the error (`callclosemethod`), which wants
    /// `LUA_MINSTACK` slots above those (`precallC`).
    fn box_closed(&mut self) -> Result<(), LuaError> {
        self.box_close_room()?;
        self.note_c_call();
        Ok(())
    }

    /// The room a box's `__close` wants (see [`Vm::box_closed`]); alone
    /// when an error closes it (see [`Vm::close_left_boxes`]), from the
    /// level of the protected call that catches it, whose next `CallInfo`
    /// the failed calls left.
    fn box_close_room(&mut self) -> Result<(), LuaError> {
        match self.native_top() {
            Some(top) => self.box_close_room_at(top),
            None => Ok(()),
        }
    }

    /// [`Vm::box_close_room`] for the box at `top`.
    fn box_close_room_at(&mut self, top: u32) -> Result<(), LuaError> {
        let pushed = if self.version() >= LuaVersion::Lua55 {
            2
        } else {
            3
        };
        let min = crate::vm::exec::stack_size::MIN_STACK;
        self.check_lua_stack(top + 2 + pushed, min, true)
    }
}

/// `boxgc`: the block of a box an error left behind is freed (luna keeps
/// none, only PUC's size of it for the trace).
fn box_gc(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let Value::Userdata(u) = vm.nat_arg(fs, nargs, 0) else {
        return Ok(0);
    };
    let left = vm
        .buffer_boxes
        .iter()
        .rposition(|b| b.0 == u32::MAX && b.1.as_ptr() == u.as_ptr());
    if let Some(i) = left {
        let size = vm.buffer_boxes.remove(i).2;
        vm.heap.trace_raw(size, 0);
    }
    Ok(0)
}
