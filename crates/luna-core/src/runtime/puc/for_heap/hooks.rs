//! The model's side of the heap's debug hooks (`gc-trace`, `gc-verify`):
//! the trace lines and checks that read the model, and the only names the
//! heap's own debug code reaches the model by. None of it is in a product
//! build.

use super::*;
#[cfg(feature = "gc-trace")]
use crate::runtime::puc::Running;
#[cfg(feature = "gc-trace")]
use crate::runtime::puc::records::env51::{is_phantom, phantom_is_table};
#[cfg(feature = "gc-verify")]
use crate::runtime::puc::records::parse::is_load_tab;
#[cfg(feature = "gc-trace")]
use crate::runtime::string;

#[cfg(feature = "gc-verify")]
pub(in crate::runtime) use crate::runtime::puc::verify::is_kept;

#[cfg(feature = "gc-trace")]
impl Heap {
    /// `LUNA_GC_PROP`: how many objects PUC's list holds as the sweep finds it.
    pub(crate) fn trace_listed(&mut self) {
        if !self.hooks.trace.prop {
            return;
        }
        let mut n = 0i64;
        let mut by = [0i64; 9];
        let mut cur = self.list_head();
        // SAFETY: the links of `all` lead to live objects
        unsafe {
            while !cur.is_null() {
                if self.puc_counts_in_sweep(cur) {
                    n += 1;
                    by[(*cur).tag as usize] += 1;
                } else if (*cur).tag == ObjTag::Ghost {
                    let g = Self::ghost_objects(&*(cur as *const Ghost));
                    n += g;
                    by[8] += g;
                }
                cur = (*cur).next_obj();
            }
        }
        n += self.env51_live();
        self.hooks.trace.line(format_args!("n {n} {by:?}"));
    }

    /// `LUNA_GC_SWEEP`: the sweep goes over `h`.
    ///
    /// # Safety
    /// `h` heads a live object of this heap.
    pub(crate) unsafe fn trace_swept(&mut self, h: *mut GcHeader, dead: bool) {
        if !self.hooks.trace.sweep {
            return;
        }
        // SAFETY: the caller's contract
        unsafe {
            let dead = u8::from(dead);
            if self.puc_counts_in_sweep(h) && (*h).tag == ObjTag::Str {
                let b = string::bytes_of(h as *const LuaStr);
                let b = String::from_utf8_lossy(&b[..b.len().min(24)])
                    .escape_debug()
                    .to_string();
                self.hooks.trace.line(format_args!("w 0 {dead} {b}"));
            } else if self.puc_counts_in_sweep(h) {
                let tag = (*h).tag as u8;
                self.hooks.trace.line(format_args!("w {tag} {dead}"));
            } else if (*h).tag == ObjTag::Ghost && (*h).sub == 0 {
                let n = Self::ghost_objects(&*(h as *const Ghost));
                self.hooks.trace.line(format_args!("w g{n} {dead}"));
            }
        }
    }

    /// `LUNA_GC_REFUSE` refuses a table resize from `old_a` array slots to
    /// `new_a` and `new_h` nodes: one of the parts PUC makes is refused.
    pub(crate) fn refuse_table(&mut self, old_a: usize, new_a: usize, new_h: usize) -> bool {
        let (oa, na, nh) = (
            self.array_bytes(old_a),
            self.array_bytes(new_a),
            self.hash_bytes(new_h),
        );
        self.hooks.trace.refuse_now(oa, na) || self.hooks.trace.refuse_now(0, nh)
    }

    /// `LUNA_GC_REFUSE` refuses a new long string of `len` bytes.
    pub(crate) fn refuse_string(&mut self, len: usize) -> bool {
        let size = self.puc_size(crate::runtime::puc::PucObj::Str { len, long: true });
        len > crate::runtime::string::MAX_SHORT_LEN && self.hooks.trace.refuse_now(0, size)
    }

    /// `LUNA_GC_PROP`: the object `h` traversed with work `w`.
    pub(crate) unsafe fn trace_prop(&mut self, h: *mut GcHeader, w: i64, run: Running) {
        // SAFETY: the caller's contract: `h` heads an allocated object
        unsafe {
            if !self.hooks.trace.prop {
                return;
            }
            {
                let (a, n) = match (*h).tag {
                    ObjTag::Table => {
                        let t = &*(h as *const Table);
                        (t.asize as usize, t.node_count())
                    }
                    ObjTag::Coro if h == run.h => (run.size as usize, run.ci as usize),
                    ObjTag::Coro if h == run.main => (run.main_size as usize, run.main_ci as usize),
                    ObjTag::Coro => {
                        let c = &*(h as *const crate::runtime::Coro);
                        (c.stack_size as usize, c.ci_alloc as usize)
                    }
                    _ => (0, 0),
                };
                let mut k = String::from("-");
                if (*h).tag == ObjTag::Table {
                    let t = &*(h as *const Table);
                    if let Some(s) = first_str_key(t) {
                        k = String::from_utf8_lossy(s.as_bytes()).into_owned();
                    }
                } else if (*h).tag == ObjTag::Native {
                    let n = &*(h as *const crate::runtime::NativeClosure);
                    k = format!("{:p}", n.f as *const ());
                } else if (*h).tag == ObjTag::Userdata {
                    let u = &*(h as *const crate::runtime::Userdata);
                    let kind = match &u.payload {
                        crate::runtime::userdata::UserdataPayload::File(_) => "file",
                        crate::runtime::userdata::UserdataPayload::Empty => "empty",
                        _ => "host",
                    };
                    k = format!("{kind}/{}/{}", u.puc_size, (*h).is_uncounted());
                }
                // a record of 5.1's environments as the table or closure
                // it stands for
                let mut tag = (*h).tag as u8;
                if is_phantom(h) {
                    let t = phantom_is_table(h);
                    tag = if t { ObjTag::Table } else { ObjTag::Native } as u8;
                    k = String::from("env51");
                }
                self.hooks
                    .trace
                    .line(format_args!("p {tag} {w} {a} {n} {k}"));
            }
        }
    }
}

#[cfg(feature = "gc-trace")]
/// The first string key of `t` with a value, in node order (the
/// propagation probe names a table by it).
fn first_str_key(t: &crate::runtime::Table) -> Option<Gc<LuaStr>> {
    let mut k = Value::Nil;
    while let Ok(Some((key, _))) = t.next(k) {
        if let Value::Str(s) = key {
            return Some(s);
        }
        k = key;
    }
    None
}

#[cfg(feature = "gc-verify")]
impl Heap {
    /// The class byte of `h`, which the sweep goes over, says whether PUC
    /// lists it (`strt`: the string table holds strings).
    ///
    /// # Safety
    /// `h` heads an allocated object of this heap.
    pub(crate) unsafe fn verify_class_byte(&self, h: *mut GcHeader, strt: bool) {
        // SAFETY: the caller's contract
        unsafe {
            assert_eq!(
                self.puc_counts_in_sweep(h),
                ((*h).tag != ObjTag::Ghost || is_load_tab(h)) && self.puc_listed(h, strt),
                "[gc-verify] class byte of a {:?}",
                (*h).tag
            );
        }
    }

    /// gc-verify: what loads set aside, which is live though on no list.
    pub(in crate::runtime) fn puc_hook_unborn(&self) -> impl Iterator<Item = *mut GcHeader> + '_ {
        self.unborn_objs()
    }

    /// gc-verify: the strings only a load holds are still on the list.
    pub(in crate::runtime) fn puc_hook_verify_kept(&self, ctx: &str) {
        self.verify_kept(ctx);
    }
}

#[cfg(test)]
impl Heap {
    /// The bytes of PUC's string table.
    pub(in crate::runtime) fn puc_hook_strt_bytes(&self) -> usize {
        self.strt_bytes()
    }
}
