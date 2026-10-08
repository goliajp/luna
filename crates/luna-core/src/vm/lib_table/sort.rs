//! `table.sort`: PUC's quicksort run in place, so the comparator sees the same
//! calls in the same order.

use super::{TAB_RW, aux_getn, key_slot, tab_geti, tab_seti};
use crate::runtime::Value;
use crate::runtime::mem::LVec;
use crate::version::LuaVersion as V;
use crate::vm::argcheck::{self, Args};
use crate::vm::builtins::{arg_error, raise_str};
use crate::vm::error::LuaError;
use crate::vm::exec::Vm;
mod auxsort_int;

pub(super) fn t_sort(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let ver = vm.version();
    let (tv, n) = aux_getn(vm, a, TAB_RW)?;
    // 5.3+ checks the size and the comparator only for a non-trivial array.
    if ver >= V::Lua53 && n <= 1 {
        return Ok(0);
    }
    if ver >= V::Lua53 && n >= i64::from(i32::MAX) {
        return Err(arg_error(vm, 1, "array too big"));
    }
    let comp = if a.is_none_or_nil(vm, 1) {
        None
    } else {
        Some(argcheck::check_function(vm, a, 1)?)
    };
    // PUC `lua_settop(L, 2)`, then every element it holds goes on the
    // stack: this frame of `sort_scratch` is that stack, so what it holds
    // above the two arguments is what the native has pushed
    let off = 2 - nargs as i32;
    // PUC keeps every element it is holding on the Lua stack; this frame
    // of `sort_scratch` is that stack, traced by `gc_roots`, so a
    // `collectgarbage()` inside the comparator cannot free them.
    let frame = match comp {
        None => pure_snapshot(vm, tv, n),
        Some(_) => None,
    };
    let snapshot = frame.as_ref().map(|f| f.len());
    let frame = frame.unwrap_or_else(|| LVec::new(vm.heap.mem()));
    vm.sort_scratch.push_or_abort(frame);
    let s = Sorter {
        tv,
        comp,
        snapshot,
        stored: std::cell::Cell::new(false),
        off,
    };
    let r = if ver <= V::Lua52 {
        s.auxsort_int(vm, 1, i64::from(n as i32))
    } else {
        s.auxsort(vm, 1, n as u32, 0)
    };
    let frame = vm.sort_scratch.pop().expect("sort frame");
    r?;
    // a run that stored nothing (an already sorted array of up to three)
    // leaves the table as it was without writing to it, as PUC's does, so
    // a read-only table raises only when PUC's sort would have stored
    if let Some(len) = snapshot
        && s.stored.get()
    {
        for (i, v) in frame[..len].iter().enumerate() {
            tab_seti(vm, tv, i as i64 + 1, *v, 0)?;
        }
    }
    Ok(0)
}

/// The elements, when sorting them cannot be observed: no comparator,
/// and `t[1..n]` all non-NaN numbers or all strings. Then no comparison
/// calls a metamethod or fails, and no element access reaches `__index`
/// or `__newindex` (every slot is present), so running the same
/// algorithm over a copy and storing the result gives the same table as
/// sorting in place, without a table access per step.
fn pure_snapshot(vm: &Vm, tv: Value, n: i64) -> Option<LVec<Value>> {
    let Value::Table(t) = tv else {
        return None;
    };
    // `n` may come from `__len`; only a real sequence fills the vector
    let mut out = LVec::new(vm.heap.mem());
    out.reserve_or_abort(usize::try_from(n.min(t.len())).ok()?);
    let mut strings = None;
    for i in 1..=n {
        let v = t.get(Value::Int(i));
        let is_str = match v {
            Value::Int(_) => false,
            Value::Float(f) if !f.is_nan() => false,
            Value::Str(_) => true,
            _ => return None,
        };
        if *strings.get_or_insert(is_str) != is_str {
            return None;
        }
        out.push_or_abort(v);
    }
    Some(out)
}

/// PUC's quicksort (`auxsort`), translated with its stack discipline: `geti`
/// pushes, `set2` stores and pops the top two, comparisons address stack
/// slots relative to the top.
struct Sorter {
    tv: Value,
    comp: Option<Value>,
    /// `Some(n)`: the elements were copied into the first `n` slots of the
    /// sort frame (see [`pure_snapshot`]) and are read and written there.
    snapshot: Option<usize>,
    /// a store went to the snapshot: PUC would have written the table
    stored: std::cell::Cell<bool>,
    /// what `lua_settop(L, 2)` left pushed past the arguments
    off: i32,
}

impl Sorter {
    fn stack(vm: &mut Vm) -> &mut LVec<Value> {
        vm.sort_scratch.last_mut().expect("sort frame")
    }

    fn at(vm: &mut Vm, rel: usize) -> Value {
        let st = Self::stack(vm);
        st[st.len() - rel]
    }

    fn pop(vm: &mut Vm, n: usize) {
        let st = Self::stack(vm);
        st.truncate(st.len() - n);
    }

    fn push(vm: &mut Vm, v: Value) {
        Self::stack(vm).push_or_abort(v);
    }

    /// Tell the native's record what the scratch stack holds, before a
    /// metamethod, a comparator or an error reads it.
    fn mark(&self, vm: &mut Vm) {
        let held = Self::stack(vm).len() as i32;
        vm.native_set_pushed(self.off + held);
    }

    fn invalid_order(&self, vm: &mut Vm) -> LuaError {
        self.mark(vm);
        raise_str(vm, "invalid order function for sorting")
    }

    fn geti(&self, vm: &mut Vm, i: i64) -> Result<(), LuaError> {
        let v = match self.snapshot {
            Some(_) => Self::stack(vm)[(i - 1) as usize],
            // raw before 5.3; a metamethod runs above what the scratch
            // stack holds
            None if vm.version() <= V::Lua52 => tab_geti(vm, self.tv, i, 0)?,
            None => {
                self.mark(vm);
                vm.index_value_pushed(self.tv, Value::Int(i), key_slot(vm))?
            }
        };
        Self::push(vm, v);
        Ok(())
    }

    /// `t[i] = top`, then pop.
    fn seti(&self, vm: &mut Vm, i: i64) -> Result<(), LuaError> {
        let v = Self::at(vm, 1);
        match self.snapshot {
            Some(_) => {
                Self::stack(vm)[(i - 1) as usize] = v;
                self.stored.set(true);
            }
            None if vm.version() <= V::Lua52 => tab_seti(vm, self.tv, i, v, 0)?,
            None => {
                self.mark(vm);
                vm.newindex_value_pushed(self.tv, Value::Int(i), v, key_slot(vm))?;
            }
        }
        Self::pop(vm, 1);
        Ok(())
    }

    /// `set2`: `t[i] = top`, pop, `t[j] = top`, pop.
    fn set2(&self, vm: &mut Vm, i: i64, j: i64) -> Result<(), LuaError> {
        self.seti(vm, i)?;
        self.seti(vm, j)
    }

    /// `sort_comp(L, -a, -b)`: is the value `a` slots down less than the one
    /// `b` slots down?
    fn lt(&self, vm: &mut Vm, a: usize, b: usize) -> Result<bool, LuaError> {
        let x = Self::at(vm, a);
        let y = Self::at(vm, b);
        match self.comp {
            // sort is an unprotected C call: the comparator runs non-yieldable.
            Some(f) => {
                self.mark(vm);
                Ok(vm
                    .call_value(f, &[x, y])?
                    .first()
                    .is_some_and(|r| r.truthy()))
            }
            None => match (x, y) {
                (Value::Int(a), Value::Int(b)) => Ok(a < b),
                (Value::Float(a), Value::Float(b)) => Ok(a < b),
                _ => {
                    self.mark(vm);
                    vm.less_than(x, y, false)
                }
            },
        }
    }

    /// 5.3+ `partition`: pivot P on top of the stack, a[lo] <= P == a[up-1]
    /// <= a[up].
    fn partition(&self, vm: &mut Vm, lo: u32, up: u32) -> Result<u32, LuaError> {
        let mut i = lo;
        let mut j = up - 1;
        loop {
            i += 1;
            self.geti(vm, i.into())?;
            while self.lt(vm, 1, 2)? {
                if i == up - 1 {
                    return Err(self.invalid_order(vm));
                }
                Self::pop(vm, 1);
                i += 1;
                self.geti(vm, i.into())?;
            }
            j -= 1;
            self.geti(vm, j.into())?;
            while self.lt(vm, 3, 1)? {
                if j < i {
                    return Err(self.invalid_order(vm));
                }
                Self::pop(vm, 1);
                j -= 1;
                self.geti(vm, j.into())?;
            }
            if j < i {
                Self::pop(vm, 1);
                self.set2(vm, (up - 1).into(), i.into())?;
                return Ok(i);
            }
            self.set2(vm, i.into(), j.into())?;
        }
    }

    /// 5.3+ `auxsort` on `unsigned int` indices, with PUC's randomized pivot
    /// once a partition comes out badly unbalanced.
    fn auxsort(&self, vm: &mut Vm, mut lo: u32, mut up: u32, mut rnd: u32) -> Result<(), LuaError> {
        while lo < up {
            self.geti(vm, lo.into())?;
            self.geti(vm, up.into())?;
            if self.lt(vm, 1, 2)? {
                self.set2(vm, lo.into(), up.into())?;
            } else {
                Self::pop(vm, 2);
            }
            if up - lo == 1 {
                return Ok(());
            }
            let mut p = if up - lo < 100 || rnd == 0 {
                (lo + up) / 2
            } else {
                let r4 = (up - lo) / 4;
                let r = if vm.version() == V::Lua53 {
                    rnd
                } else {
                    rnd ^ lo ^ up
                };
                r % (r4 * 2) + (lo + r4)
            };
            self.geti(vm, p.into())?;
            self.geti(vm, lo.into())?;
            if self.lt(vm, 2, 1)? {
                self.set2(vm, p.into(), lo.into())?;
            } else {
                Self::pop(vm, 1);
                self.geti(vm, up.into())?;
                if self.lt(vm, 1, 2)? {
                    self.set2(vm, p.into(), up.into())?;
                } else {
                    Self::pop(vm, 2);
                }
            }
            if up - lo == 2 {
                return Ok(());
            }
            self.geti(vm, p.into())?;
            let pivot = Self::at(vm, 1);
            Self::push(vm, pivot);
            self.geti(vm, (up - 1).into())?;
            self.set2(vm, p.into(), (up - 1).into())?;
            p = self.partition(vm, lo, up)?;
            let n;
            if p - lo < up - p {
                self.auxsort(vm, lo, p - 1, rnd)?;
                n = p - lo;
                lo = p + 1;
            } else {
                self.auxsort(vm, p + 1, up, rnd)?;
                n = up - p;
                up = p - 1;
            }
            if up.wrapping_sub(lo) / 128 > n {
                rnd = randomize_pivot();
            }
        }
        Ok(())
    }
}

/// PUC `l_randomizePivot`: any cheap varying value (5.3/5.4 mix `clock()`
/// and `time()`).
fn randomize_pivot() -> u32 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos() ^ d.as_secs() as u32)
}
