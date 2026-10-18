//! What PUC's `luaL_openlibs` allocates, replayed from its record.

use super::*;
use crate::runtime::puc_libs::Opening;
use crate::runtime::table::Dialect;

/// Where PUC's collector stood after the last step it took while the
/// libraries opened, when that step ended in the pause.
#[derive(Clone, Copy, Debug)]
pub(crate) struct LibsPace {
    /// the debt (5.1: the threshold) and the estimate
    debt: i64,
    estimate: i64,
    /// the count then, from which what the libraries make after adds to
    /// the debt
    bytes: usize,
    /// 5.1's `gcdept`
    dept: i64,
}

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
    /// shrank it meanwhile (5.3, whose `lua.c` leaves it running, and an
    /// embedder's opening).
    pub(crate) fn acct_replay_openlibs(
        &mut self,
        stack: u32,
    ) -> Result<Option<u32>, crate::runtime::mem::Oom> {
        #[cfg(feature = "gc-trace")]
        let no_auto = self.trace.no_auto;
        #[cfg(not(feature = "gc-trace"))]
        let no_auto = false;
        // the collector steps while the libraries open unless it is stopped
        let opening = if no_auto || self.gc_stopped {
            Opening::Stopped
        } else {
            self.puc.opening
        };
        let (ops, strs) = crate::runtime::puc_libs::record(self.table_dialect, opening);
        let mut live = Life::Kept;
        let mut size = 0;
        let mut shrunk = None;
        let stepping = opening != Opening::Stopped;
        let ops = ops.as_chunks::<3>().0;
        for (i, op) in ops.iter().enumerate() {
            let (x, y) = (op[1] as usize, op[2] as usize);
            match op[0] {
                3 => self.replay_string(strs[x], live, size)?,
                6 => {
                    // the pace the collector reached while the libraries opened
                    let debt = if self.table_dialect == Dialect::L51 {
                        i64::from(op[1])
                    } else {
                        i64::from(op[1] as i32)
                    };
                    let (estimate, bytes) = (i64::from(op[2]), self.bytes);
                    self.puc.libs_pace = Some(LibsPace {
                        debt,
                        estimate,
                        bytes,
                        dept: 0,
                    });
                }
                10 => {
                    if let Some(p) = &mut self.puc.libs_pace {
                        p.dept = i64::from(op[1]);
                    }
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
                // the collector shrinking the main thread's stack
                1 if stepping && x == self.stack_bytes(shrunk.unwrap_or(stack)) => {
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

    /// The libraries are open. When the collector stepped meanwhile and
    /// its last step ended in the pause, it stands where the record says:
    /// in the pause, with that debt (5.1: threshold and `gcdept`) and
    /// estimate. When it was stopped, it stays as it is. Otherwise it
    /// restarts from no debt.
    pub(crate) fn acct_libs_pace(&mut self) {
        let d = self.table_dialect;
        if let Some(p) = self.puc.libs_pace.take() {
            self.pace.state = crate::runtime::gc_pace::St::Pause;
            self.pace.estimate = p.estimate;
            if d == Dialect::L51 {
                self.pace.set_threshold(p.debt);
                self.pace.dept = p.dept;
            } else {
                self.pace.set_debt(d, p.bytes, p.debt);
            }
            self.refresh_gc_limit();
        } else if !self.gc_stopped {
            self.gc_set_stopped(false);
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
