//! Where PUC's C API checks for a collection inside a library function:
//! 5.1 and 5.2 before the call allocates, 5.3 on after it pushed what it
//! made. The running C function's stack is live up to its top; a value
//! the library function holds is rooted while the collector may run.

use super::*;
use crate::runtime::LuaStr;
use crate::runtime::puc::api::PucText;

impl Vm {
    /// 5.1 and 5.2: the check before an API call allocates.
    #[inline]
    #[track_caller]
    pub(crate) fn api_check_before<const PRE: bool>(&mut self) -> Result<(), LuaError> {
        self.assert_rules::<PRE>();
        if PRE {
            return self.native_checkpoint();
        }
        Ok(())
    }

    /// 5.3 on: the check after an API call pushed `v`.
    #[inline]
    #[track_caller]
    pub(crate) fn api_check_after<const PRE: bool>(&mut self, v: Value) -> Result<(), LuaError> {
        self.assert_rules::<PRE>();
        if !PRE && !self.checkpoint_idle() {
            let r = self.rooted(&[v], |vm| vm.native_checkpoint_due());
            return r;
        }
        Ok(())
    }

    /// 5.3 on: the check after an API call pushed `v` on the running C
    /// function's stack, which is live up to `v` then.
    #[track_caller]
    pub(crate) fn api_check_pushed<const PRE: bool>(&mut self, v: Value) -> Result<(), LuaError> {
        if PRE || !self.native_on_top() {
            return self.api_check_after::<PRE>(v);
        }
        self.assert_rules::<PRE>();
        if self.checkpoint_idle() {
            return Ok(());
        }
        self.native_push_vals(&[v])?;
        let r = self.native_checkpoint_due();
        self.native_pop(1);
        r
    }

    /// 5.3 on: the check after an API call pushed a value already rooted
    /// (in `lib_roots`).
    #[inline]
    #[track_caller]
    pub(crate) fn api_check_after_rooted<const PRE: bool>(&mut self) -> Result<(), LuaError> {
        self.assert_rules::<PRE>();
        if !PRE {
            return self.native_checkpoint();
        }
        Ok(())
    }

    /// Code built for the rule `PRE` runs only in a VM of that rule.
    #[inline(always)]
    fn assert_rules<const PRE: bool>(&self) {
        debug_assert_eq!(PRE, self.heap.rules().api_checks_before());
    }

    /// PUC `luaL_getmetafield(L, idx, event)` on `v`: when `v` has a
    /// metatable it pushes the metatable, then the event's name
    /// (`lua_pushstring`, which makes the string and checks for a
    /// collection), the string given back.
    #[inline(always)]
    #[track_caller]
    pub(crate) fn metafield_name<const PRE: bool>(
        &mut self,
        v: Value,
        event: &[u8],
    ) -> Result<Option<Gc<LuaStr>>, LuaError> {
        let Some(mt) = self.metatable_of(v) else {
            return Ok(None);
        };
        // with no collection due, the pushes and checks change nothing seen
        if let (_, Some(s)) = self.name_cached(event)
            && self.checkpoint_idle()
        {
            return Ok(Some(s));
        }
        self.metafield_name_slow::<PRE>(mt, event)
    }

    /// [`Vm::metafield_name`] once the name has to be made or a collection
    /// may be due.
    #[inline(never)]
    #[track_caller]
    fn metafield_name_slow<const PRE: bool>(
        &mut self,
        mt: Gc<Table>,
        event: &[u8],
    ) -> Result<Option<Gc<LuaStr>>, LuaError> {
        // the metatable on the running C function's stack, where a
        // collection the checks run finds it (a table that just lost it as
        // its metatable holds it no more)
        let mt = [Value::Table(mt)];
        if !self.native_on_top() {
            return self
                .rooted(&mt, |vm| vm.metafield_name_pushed::<PRE>(event))
                .map(Some);
        }
        self.native_push_vals(&mt)?;
        let r = self.metafield_name_pushed::<PRE>(event);
        self.native_pop(1);
        r.map(Some)
    }

    /// [`Vm::metafield_name`] with the metatable pushed: the name made and
    /// pushed, as `lua_pushstring` makes it.
    #[track_caller]
    fn metafield_name_pushed<const PRE: bool>(
        &mut self,
        event: &[u8],
    ) -> Result<Gc<LuaStr>, LuaError> {
        self.api_check_before::<PRE>()?;
        let s = match self.name_cached(event) {
            (_, Some(s)) => s,
            (slot, None) => {
                let s = self.alloc_str(event)?;
                self.name_cache[slot] = (event.as_ptr() as usize, Some(s));
                s
            }
        };
        // pushed on the running C function's stack, where a collection the
        // check runs finds it (5.3 counts it with the thread)
        self.api_check_pushed::<PRE>(Value::Str(s))?;
        Ok(s)
    }

    /// `event`'s slot in the name cache (by address, as PUC's string cache;
    /// emptied when a collection step begins) and the live string there.
    fn name_cached(&self, event: &[u8]) -> (usize, Option<Gc<LuaStr>>) {
        let key = event.as_ptr() as usize;
        let slot = (key >> 3) % self.name_cache.len();
        match self.name_cache[slot] {
            (k, Some(s)) if k == key => (slot, Some(s)),
            _ => (slot, None),
        }
    }

    /// [`Vm::metafield_name`] where only the push matters.
    #[inline(always)]
    #[track_caller]
    pub(crate) fn metafield_check<const PRE: bool>(
        &mut self,
        v: Value,
        event: &[u8],
    ) -> Result<(), LuaError> {
        self.metafield_name::<PRE>(v, event).map(drop)
    }

    /// [`Vm::metafield_check`] where the caller is built for neither rule.
    pub(crate) fn metafield_check_dyn(&mut self, v: Value, event: &[u8]) -> Result<(), LuaError> {
        with_rules!(self, P => self.metafield_check::<P>(v, event))
    }

    /// `f` as an API call that pushes the string it makes: checked for
    /// before (5.1, 5.2) or after (5.3 on), the string kept rooted in
    /// `lib_roots` for the caller, who truncates it.
    #[track_caller]
    pub(crate) fn api_pushing<const PRE: bool>(
        &mut self,
        f: impl FnOnce(&mut Vm) -> Result<Gc<LuaStr>, LuaError>,
    ) -> Result<Gc<LuaStr>, LuaError> {
        self.api_check_before::<PRE>()?;
        let r = f(self)?;
        self.lib_roots.push(Value::Str(r));
        self.api_check_after_rooted::<PRE>()?;
        Ok(r)
    }

    /// The check every version makes after `lua_tolstring` converted a
    /// number into `v`.
    #[inline]
    #[track_caller]
    pub(crate) fn api_check_converted(&mut self, v: Value) -> Result<(), LuaError> {
        self.rooted(&[v], |vm| vm.native_checkpoint())
    }

    /// `lua_createtable(L, narr, nrec)`, checked for as PUC checks.
    #[track_caller]
    pub(crate) fn api_new_table<const PRE: bool>(
        &mut self,
        narr: usize,
        nrec: usize,
    ) -> Result<Gc<Table>, LuaError> {
        self.api_check_before::<PRE>()?;
        let t = self.alloc_table_sized(narr, nrec)?;
        self.api_check_after::<PRE>(Value::Table(t))?;
        Ok(t)
    }

    /// `lua_pushfstring` of `conv` conversions: PUC's pieces made (see
    /// `Heap::pushfstring`), after the stack room its formatting checks
    /// for, and checked for as PUC checks.
    pub(crate) fn api_pushfstring<const PRE: bool>(
        &mut self,
        conv: u32,
        fmt: &[u8],
        args: &[crate::runtime::puc::fmt::FmtArg<'_>],
    ) -> Result<Gc<LuaStr>, LuaError> {
        if let Some(e) = self.format_pushes(&[Some(conv)]) {
            return Err(e);
        }
        self.api_check_before::<PRE>()?;
        let m = self.pushfstring(fmt, args)?;
        self.api_check_after::<PRE>(Value::Str(m))?;
        Ok(m)
    }

    /// `lua_pushlstring`: the string of `bytes`, checked for as PUC checks.
    #[track_caller]
    pub(crate) fn api_push_string<const PRE: bool>(
        &mut self,
        bytes: &[u8],
    ) -> Result<Value, LuaError> {
        self.api_check_before::<PRE>()?;
        let s = self.str_value(bytes)?;
        self.api_check_after::<PRE>(s)?;
        Ok(s)
    }

    /// [`Vm::api_push_string`] where the caller is built for neither rule.
    pub(crate) fn api_push_string_dyn(&mut self, bytes: &[u8]) -> Result<Value, LuaError> {
        with_rules!(self, P => self.api_push_string::<P>(bytes))
    }

    /// [`Vm::api_push_string`] of a C string that lives at `at`: PUC's
    /// `lua_pushstring`, through `luaS_new`'s cache.
    pub(crate) fn api_push_string_at<const PRE: bool>(
        &mut self,
        at: crate::runtime::puc::records::strcache::CStr,
        bytes: &[u8],
    ) -> Result<Value, LuaError> {
        self.api_check_before::<PRE>()?;
        let s = self.str_value_at(at, bytes)?;
        self.api_check_after::<PRE>(s)?;
        Ok(s)
    }

    /// [`Vm::str_value`] through `luaS_new`'s cache, the C string at `at`.
    pub(crate) fn str_value_at(
        &mut self,
        at: crate::runtime::puc::records::strcache::CStr,
        bytes: &[u8],
    ) -> Result<Value, LuaError> {
        let s = self.retry_oom(|vm| vm.heap.intern_at(at, bytes))?;
        Ok(Value::Str(s))
    }
}
