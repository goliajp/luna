//! What PUC's `luaL_openlibs` allocates, replayed from its record.

use super::*;
use crate::runtime::table::Dialect;

impl Heap {
    fn raw_event(&mut self, op: u32, x: usize, y: usize) {
        match op {
            1 => self.bytes = self.bytes + y - x,
            2 => self.bytes -= x,
            _ => self.bytes += y,
        }
        #[cfg(feature = "gc-trace")]
        match op {
            1 => self.trace.line(format_args!("r {x} {y}")),
            2 => self.trace.line(format_args!("d {x}")),
            _ => self.trace.line(format_args!("a {x} {y}")),
        }
    }

    /// What PUC's `luaL_openlibs` allocates, replayed from its record
    /// (`puc_libs_data.rs`), after luna opened its libraries without
    /// counting. Each string PUC made is made counted in luna too (luna
    /// may not have it), and the ones PUC keeps are kept, so a later
    /// string of the same bytes finds it as in PUC.
    /// Returns the size of the main thread's stack when the collector
    /// shrank it meanwhile (5.3, whose `lua.c` leaves it running).
    pub(crate) fn acct_replay_openlibs(
        &mut self,
        stack: u32,
    ) -> Result<Option<u32>, crate::runtime::mem::Oom> {
        #[cfg(feature = "gc-trace")]
        let stopped = self.trace.no_auto;
        #[cfg(not(feature = "gc-trace"))]
        let stopped = false;
        let (ops, strs) = crate::runtime::puc_libs::record(self.table_dialect, stopped);
        let mut live = Life::Kept;
        let mut size = 0;
        let mut shrunk = None;
        let ops = ops.as_chunks::<3>().0;
        for (i, op) in ops.iter().enumerate() {
            let (x, y) = (op[1] as usize, op[2] as usize);
            match op[0] {
                3 => self.replay_string(strs[x], live, size)?,
                6 => {
                    // the pace the collector reached while the libraries opened
                    self.puc.libs_pace = Some((i64::from(op[1] as i32), i64::from(op[2])));
                }
                7 => {
                    self.raw_event(2, x, 0);
                    self.puc.strt_nuse -= 1;
                }
                // a step of PUC's, whose frees the trace compares as a set
                8 | 9 => {
                    #[cfg(feature = "gc-trace")]
                    self.trace
                        .line(format_args!("{}", if op[0] == 8 { "g" } else { "G" }));
                }
                1 if self.table_dialect == Dialect::L53
                    && x == self.stack_bytes(shrunk.unwrap_or(stack)) =>
                {
                    self.raw_event(1, x, y);
                    shrunk = Some((y / self.sizes.tvalue) as u32);
                }
                k => {
                    live = match k {
                        4 => Life::Garbage,
                        5 => Life::Freed,
                        _ => Life::Kept,
                    };
                    size = y;
                    self.raw_event(k, x, y);
                    // garbage that is not a string: 5.1's C closures of the
                    // `luaopen_` functions `luaL_openlibs` calls
                    if k == 4 && ops.get(i + 1).is_none_or(|n| n[0] != 3) {
                        self.ghost_record(PucKind::CClosure, y)?;
                    }
                }
            }
        }
        Ok(shrunk)
    }

    /// The libraries are open: the collector restarts from no debt, as
    /// `lua_gc(L, LUA_GCRESTART)` leaves it (5.1 and 5.2's `lua.c` stop it
    /// meanwhile, and so does luna's own opening), or, when it ran
    /// meanwhile (5.3), stands where the record says it had come to: in
    /// the pause, with that debt and estimate.
    pub(crate) fn acct_libs_pace(&mut self) {
        self.gc_set_stopped(false);
        if let Some((debt, estimate)) = self.puc.libs_pace.take() {
            self.pace.state = crate::runtime::gc_pace::St::Pause;
            self.pace.estimate = estimate;
            self.pace.set_debt(self.table_dialect, self.bytes, debt);
            self.refresh_gc_limit();
        }
    }

    /// The string of `bytes` PUC's record made, at `size` bytes.
    fn replay_string(
        &mut self,
        bytes: &'static [u8],
        life: Life,
        size: usize,
    ) -> Result<(), crate::runtime::mem::Oom> {
        let live = life == Life::Kept;
        let short = bytes.len() <= crate::runtime::string::MAX_SHORT_LEN
            || self.table_dialect == Dialect::L51;
        if short {
            // the string table's growth is in the record; follow its size
            if self.table_dialect != Dialect::L51 && self.puc.strt_nuse >= self.puc.strt_size {
                self.puc.strt_size *= 2;
            }
            self.puc.strt_nuse += 1;
            if self.table_dialect == Dialect::L51 && self.puc.strt_nuse > self.puc.strt_size {
                self.puc.strt_size *= 2;
            }
        }
        // one the collector freed before the libraries were open: nothing
        // of it is left (its free, in the record, leaves the string table)
        if life == Life::Freed {
            return Ok(());
        }
        if bytes.len() > crate::runtime::string::MAX_SHORT_LEN && self.table_dialect != Dialect::L51
        {
            // luna does not intern long strings (but 5.1's): nothing to
            // find later; one luna's libraries keep too is theirs
            // (`acct_adopt_long`)
            if live {
                self.puc.kept_long.push((bytes, size));
            }
            return Ok(());
        }
        if !live
            && let Some(s) = self.find_interned(bytes)
            && is_uncounted(s.as_ptr() as *mut GcHeader)
        {
            // a string luna keeps for itself and PUC drops: PUC's (counted
            // with the op before) is garbage, luna's stays out of the
            // count until the program makes one
            let size = self.sizes.string(bytes.len(), false);
            return self.ghost_record(PucKind::ShortStr, size);
        }
        self.puc.silent += 1;
        let s = self.try_intern(bytes);
        self.puc.silent -= 1;
        let s = s?;
        super::set_uncounted(s.as_ptr() as *mut GcHeader, false);
        if live {
            self.puc.pins.push(s);
        }
        Ok(())
    }
}

/// What becomes of an object PUC's record of `luaL_openlibs` made.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Life {
    /// kept by the libraries
    Kept,
    /// garbage once they are open
    Garbage,
    /// freed by the collector before they were all open
    Freed,
}
