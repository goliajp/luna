//! The box of a 5.3+ `luaL_Buffer` (`newbox`): a full userdata with a
//! `__gc` metamethod, on the C function's stack until the buffer is done,
//! then garbage that the collector finalizes before freeing it.

use crate::runtime::table::Dialect;
use crate::runtime::userdata::UserdataPayload;
use crate::runtime::{Gc, Table, Userdata, Value};
use crate::vm::error::LuaError;
use crate::vm::exec::Vm;

impl Vm {
    /// `newbox`: the userdata, with the metatable PUC keeps in the registry
    /// for boxes (made the first time), registered for finalization.
    pub(crate) fn new_box(&mut self) -> Result<Gc<Userdata>, LuaError> {
        let ubox = 2 * std::mem::size_of::<usize>();
        let size = if self.heap.table_dialect >= Dialect::L54 {
            self.heap.sizes.userdata(0, ubox)
        } else {
            self.heap.sizes.userdata(1, ubox)
        };
        let u = self.lib_userdata(UserdataPayload::Empty, false, size)?;
        // on the stack of the running native until the buffer is done
        self.buffer_boxes
            .push((self.running_natives.len() as u32, u));
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
            .rposition(|&(_, b)| b.as_ptr() == u.as_ptr())
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
            .filter(move |&&(at, _)| at <= natives)
            .map(|&(_, u)| u)
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
        let base = self.lib_roots.len();
        self.lib_roots.push(mtv);
        let r = self.fill_box_metatable(mt, name, reg, l53);
        self.lib_roots.truncate(base);
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

    /// A box's `__close` (5.4+, `lua_closeslot`): a call one level up, of a
    /// C function, made above the box and the string just pushed with the
    /// method, the box and the error (`callclosemethod`), which wants
    /// `LUA_MINSTACK` slots above those (`precallC`).
    pub(crate) fn box_closed(&mut self) -> Result<(), LuaError> {
        if let Some(top) = self.native_top() {
            self.check_lua_stack(top + 2 + 3, crate::vm::exec::stack_size::MIN_STACK, true)?;
        }
        self.note_c_call();
        Ok(())
    }
}

/// `boxgc`: luna keeps no block in a box, so there is nothing to free.
fn box_gc(_vm: &mut Vm, _fs: u32, _nargs: u32) -> Result<u32, LuaError> {
    Ok(0)
}
