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

mod auxsort;
mod auxsort_int;
mod snapshot;
use snapshot::pure_snapshot;

/// `table.sort`'s table, its length and the comparator, if any; none for
/// an array 5.3 on leave as it is without looking at the comparator.
#[allow(clippy::type_complexity)]
fn sort_args(vm: &mut Vm, a: Args) -> Result<Option<(Value, i64, Option<Value>)>, LuaError> {
    let ver = vm.version();
    let (tv, n) = aux_getn(vm, a, TAB_RW)?;
    if ver <= V::Lua52 {
        // "assume array is smaller than 2^40"
        argcheck::check_stack(vm, a, 40, "")?;
    }
    if ver >= V::Lua53 && n <= 1 {
        return Ok(None);
    }
    if ver >= V::Lua53 && n >= i64::from(i32::MAX) {
        return Err(arg_error(vm, 1, "array too big"));
    }
    let comp = if a.is_none_or_nil(vm, 1) {
        None
    } else {
        Some(argcheck::check_function(vm, a, 1)?)
    };
    Ok(Some((tv, n, comp)))
}

pub(super) fn t_sort(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let ver = vm.version();
    let Some((tv, n, comp)) = sort_args(vm, a)? else {
        return Ok(0);
    };
    // PUC `lua_settop(L, 2)`, then every element it holds goes on the
    // stack: this frame of `sort_scratch` is that stack, traced by
    // `gc_roots`, and what it holds above the two arguments is what the
    // native has pushed
    let off = 2 - nargs as i32;
    let frame = match comp {
        None => pure_snapshot(vm, tv, n)?,
        Some(_) => None,
    };
    let snapshot = frame.as_ref().map(|f| f.len());
    let frame = frame.unwrap_or_else(|| LVec::new(vm.heap.mem()));
    vm.sort_scratch.push(frame)?;
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

    fn push(vm: &mut Vm, v: Value) -> Result<(), LuaError> {
        Self::stack(vm).push(v)?;
        Ok(())
    }

    /// Tell the native's record what the scratch stack holds, before a
    /// metamethod, a comparator or an error reads it.
    fn mark(&self, vm: &mut Vm) -> Result<(), LuaError> {
        let held = Self::stack(vm).len();
        vm.native_set_pushed(self.off + held as i32);
        if self.snapshot.is_some() || !vm.native_on_top() {
            return Ok(());
        }
        // and write it where PUC's stack has it: `lua_settop(L, 2)`'s nil,
        // then the elements
        let pad = self.off.max(0) as usize;
        vm.native_hold(0, &[Value::Nil; 2][..pad])?;
        let a = vm.running_natives.last().expect("sort runs");
        let at = (a.func_slot + 1 + a.nargs).wrapping_add_signed(self.off) as usize;
        vm.grow_stack(at + held)?;
        let scratch = vm.sort_scratch.last().expect("sort frame");
        vm.stack[at..at + held].copy_from_slice(scratch);
        Ok(())
    }

    fn invalid_order(&self, vm: &mut Vm) -> LuaError {
        if let Err(e) = self.mark(vm) {
            return e;
        }
        raise_str(vm, "invalid order function for sorting")
    }

    fn geti(&self, vm: &mut Vm, i: i64) -> Result<(), LuaError> {
        let v = match self.snapshot {
            Some(_) => Self::stack(vm)[(i - 1) as usize],
            // raw before 5.3; a metamethod runs above what the scratch
            // stack holds
            None if vm.version() <= V::Lua52 => tab_geti(vm, self.tv, i, 0)?,
            None => {
                self.mark(vm)?;
                let key = [Value::Int(i)];
                vm.index_value_holding(self.tv, key[0], &key[..key_slot(vm) as usize])?
            }
        };
        Self::push(vm, v)?;
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
                self.mark(vm)?;
                let key = [Value::Int(i)];
                vm.newindex_value_holding(self.tv, key[0], v, &key[..key_slot(vm) as usize])?;
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
                self.mark(vm)?;
                Ok(vm
                    .call_value(f, &[x, y])?
                    .first()
                    .is_some_and(|r| r.truthy()))
            }
            None => match (x, y) {
                (Value::Int(a), Value::Int(b)) => Ok(a < b),
                (Value::Float(a), Value::Float(b)) => Ok(a < b),
                _ => {
                    self.mark(vm)?;
                    vm.less_than(x, y, false)
                }
            },
        }
    }
}
