//! `math.max` / `math.min`.

use crate::runtime::Value;
use crate::version::LuaVersion as V;
use crate::vm::argcheck::{self, Args};
use crate::vm::builtins::arg_error;
use crate::vm::error::LuaError;
use crate::vm::exec::Vm;

/// `math.max` / `math.min`. ≤5.2 converts every argument with
/// `luaL_checknumber` and compares doubles; 5.3+ compares the arguments
/// themselves with `lua_compare` (metamethods included) and returns the
/// winner unconverted.
pub(super) fn minmax(vm: &mut Vm, fs: u32, nargs: u32, want_max: bool) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    if vm.version() <= V::Lua52 {
        let mut best = argcheck::check_number(vm, a, 0)?;
        for i in 1..nargs {
            let d = argcheck::check_number(vm, a, i)?;
            if if want_max { d > best } else { d < best } {
                best = d;
            }
        }
        return Ok(vm.nat_return(fs, &[Value::Float(best)]));
    }
    if nargs == 0 {
        return Err(arg_error(vm, 1, "value expected"));
    }
    // PUC `math_min` / `math_max`: compare the argument slots in place and
    // keep the index of the winner, copied out once at the end
    use crate::runtime::value::tag;
    let first = fs as usize + 1;
    let mut best = 0usize;
    for i in 1..nargs as usize {
        // the stack may have moved under a metamethod; take it again
        let (pb, pv) = (&vm.stack[first + best], &vm.stack[first + i]);
        let swap = match (pb.tag_byte(), pv.tag_byte()) {
            // SAFETY: the tags say which payload each holds
            (tag::INT, tag::INT) => unsafe {
                let (x, y) = (pb.as_int_unchecked(), pv.as_int_unchecked());
                if want_max { x < y } else { y < x }
            },
            (tag::FLOAT, tag::FLOAT) => {
                let (Value::Float(x), Value::Float(y)) = (*pb, *pv) else {
                    unreachable!("float tags")
                };
                if want_max { x < y } else { y < x }
            }
            _ => {
                let (b, v) = (*pb, *pv);
                if want_max {
                    vm.less_than(b, v, false)?
                } else {
                    vm.less_than(v, b, false)?
                }
            }
        };
        if swap {
            best = i;
        }
    }
    vm.stack[fs as usize] = vm.stack[first + best];
    Ok(1)
}
