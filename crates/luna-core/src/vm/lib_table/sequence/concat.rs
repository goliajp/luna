//! `table.concat`.

use super::*;
use crate::vm::puc_buffer::PucBuffer;

/// The separator `concat` appends: the argument string itself (rooted by
/// the call frame) or a number's rendering.
enum Sep {
    Str(Gc<LuaStr>),
    Bytes(Vec<u8>),
}

impl Sep {
    fn bytes(&self) -> &[u8] {
        match self {
            Sep::Str(s) => s.as_bytes(),
            Sep::Bytes(b) => b,
        }
    }
}

/// `luaL_optlstring(L, 2, "")`.
fn concat_sep(vm: &mut Vm, a: Args) -> Result<Sep, LuaError> {
    Ok(match a.get(vm, 1) {
        Value::Str(s) => Sep::Str(s),
        _ if a.is_none_or_nil(vm, 1) => Sep::Bytes(Vec::new()),
        v => match argcheck::to_str_bytes(vm, v) {
            Some(b) => Sep::Bytes(b),
            None => return Err(argcheck::type_error(vm, a, 1, "string")),
        },
    })
}

pub(in crate::vm::lib_table) fn t_concat(
    vm: &mut Vm,
    fs: u32,
    nargs: u32,
) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let (tv, sep, i, last) = if vm.version() <= V::Lua52 {
        // ≤5.2 reads the separator before checking the table, and takes
        // the length only when no end index is given.
        let sep = concat_sep(vm, a)?;
        let tv = checktab(vm, a, 0, TAB_R)?;
        let i = argcheck::opt_int(vm, a, 2, 1)?;
        let last = if a.is_none_or_nil(vm, 3) {
            obj_len(vm, tv)? as i32
        } else {
            argcheck::check_int(vm, a, 3)?
        };
        (tv, sep, i64::from(i), i64::from(last))
    } else {
        let (tv, n) = aux_getn(vm, a, TAB_R)?;
        let sep = concat_sep(vm, a)?;
        let i = argcheck::opt_integer(vm, a, 2, 1)?;
        let last = argcheck::opt_integer(vm, a, 3, n)?;
        (tv, sep, i, last)
    };
    let mut out = vm.new_buf();
    let mut b = PucBuffer::new(vm);
    // PUC appends `[i, last)` each followed by the separator, then `last`
    // on its own, so `last == maxinteger` never overflows the counter.
    let mut k = i;
    while k < last {
        concat_field(vm, tv, k, &mut out, &mut b)?;
        vm.buf_push(&mut out, sep.bytes())?;
        b.add(vm, &out, sep.bytes().len())?;
        k += 1;
    }
    if k == last {
        concat_field(vm, tv, last, &mut out, &mut b)?;
    }
    if out.len() > crate::runtime::string::MAX_LEN {
        return Err(vm.rt_err("string length overflow"));
    }
    let s = b.finish(vm, &out)?;
    vm.recycle_buf(out);
    vm.nat_ret(fs, &[Value::Str(s)])
}

fn concat_field(
    vm: &mut Vm,
    tv: Value,
    k: i64,
    out: &mut LVec<u8>,
    b: &mut PucBuffer,
) -> Result<(), LuaError> {
    // the buffer's slot, then the value until it is added
    let buf = vm.buffer_slot(out.len());
    // `luaL_addvalue`, which turns a number into a string first
    let held = [Vm::buffer_box()];
    let s = match tab_geti_held(vm, tv, k, &held[..buf as usize])? {
        Value::Str(s) => s,
        v @ (Value::Int(_) | Value::Float(_)) => vm.number_string(v)?,
        // `luaL_typename`: the base type name, never `__name`.
        v => {
            let msg = vm.fmt_bytes(format_args!(
                "invalid value ({}) at index {k} in table for 'concat'",
                v.type_name()
            ))?;
            vm.native_push(buf + 1);
            // "invalid value (at index %d) in table for 'concat'"
            return Err(crate::vm::builtins::raise_bytes_fmt(vm, 1, &msg));
        }
    };
    // on the stack until added (`luaL_addvalue`), which roots it itself
    vm.lib_roots.push(Value::Str(s));
    let pushed = vm.buf_push(out, s.as_bytes());
    vm.lib_roots.pop();
    pushed?;
    b.add_value(vm, out, s)
}
