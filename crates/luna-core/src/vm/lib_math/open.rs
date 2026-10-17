//! Opening the `math` library: its functions in the order of PUC's
//! `mathlib` for each version, then its constants. The lists are code, not
//! tables of pointers, which the program would have to relocate as it
//! loads.

use super::*;

type Native = fn(&mut Vm, u32, u32) -> Result<u32, LuaError>;

/// Open the `math` library, failing when memory runs out.
pub(crate) fn try_open_math(vm: &mut Vm) -> Result<(), LuaError> {
    let ver = vm.version();
    let t = vm.lib_table_puc("math")?;
    let mut e: Vec<(&str, Value)> = Vec::with_capacity(36);
    let f = |vm: &mut Vm, e: &mut Vec<(&str, Value)>, name: &'static str, f: Native| {
        let b = inlinable_builtin(name.as_bytes()).unwrap_or(Builtin::None);
        e.push((name, vm.builtin(f, &[], b)?));
        Ok::<(), LuaError>(())
    };
    if ver <= V::Lua52 {
        f(vm, &mut e, "abs", m_abs)?;
        f(vm, &mut e, "acos", m_acos)?;
        f(vm, &mut e, "asin", m_asin)?;
        f(vm, &mut e, "atan2", m_atan2)?;
        f(vm, &mut e, "atan", m_atan)?;
        f(vm, &mut e, "ceil", m_ceil)?;
        f(vm, &mut e, "cosh", m_cosh)?;
        f(vm, &mut e, "cos", m_cos)?;
        f(vm, &mut e, "deg", m_deg)?;
        f(vm, &mut e, "exp", m_exp)?;
        f(vm, &mut e, "floor", m_floor)?;
        f(vm, &mut e, "fmod", m_fmod)?;
        f(vm, &mut e, "frexp", m_frexp)?;
        f(vm, &mut e, "ldexp", m_ldexp)?;
        f(vm, &mut e, "log10", m_log10)?;
        f(vm, &mut e, "log", m_log)?;
        f(vm, &mut e, "max", m_max)?;
        f(vm, &mut e, "min", m_min)?;
        f(vm, &mut e, "modf", m_modf)?;
        f(vm, &mut e, "pow", m_pow)?;
        f(vm, &mut e, "rad", m_rad)?;
        rand(vm, &mut e)?;
        f(vm, &mut e, "sinh", m_sinh)?;
        f(vm, &mut e, "sin", m_sin)?;
        f(vm, &mut e, "sqrt", m_sqrt)?;
        f(vm, &mut e, "tanh", m_tanh)?;
        f(vm, &mut e, "tan", m_tan)?;
    } else {
        f(vm, &mut e, "abs", m_abs)?;
        f(vm, &mut e, "acos", m_acos)?;
        f(vm, &mut e, "asin", m_asin)?;
        f(vm, &mut e, "atan", m_atan)?;
        f(vm, &mut e, "ceil", m_ceil)?;
        f(vm, &mut e, "cos", m_cos)?;
        f(vm, &mut e, "deg", m_deg)?;
        f(vm, &mut e, "exp", m_exp)?;
        f(vm, &mut e, "tointeger", m_tointeger)?;
        f(vm, &mut e, "floor", m_floor)?;
        f(vm, &mut e, "fmod", m_fmod)?;
        if ver >= V::Lua55 {
            f(vm, &mut e, "frexp", m_frexp)?;
        }
        f(vm, &mut e, "ult", m_ult)?;
        if ver >= V::Lua55 {
            f(vm, &mut e, "ldexp", m_ldexp)?;
        }
        f(vm, &mut e, "log", m_log)?;
        f(vm, &mut e, "max", m_max)?;
        f(vm, &mut e, "min", m_min)?;
        f(vm, &mut e, "modf", m_modf)?;
        f(vm, &mut e, "rad", m_rad)?;
        if ver == V::Lua53 {
            rand(vm, &mut e)?;
        }
        f(vm, &mut e, "sin", m_sin)?;
        f(vm, &mut e, "sqrt", m_sqrt)?;
        f(vm, &mut e, "tan", m_tan)?;
        f(vm, &mut e, "type", m_type)?;
        // the pre-5.3 functions, kept by the default LUA_COMPAT_MATHLIB of
        // the 5.3 and 5.4 builds; `atan2` is registered as `math_atan`, the
        // very same function value
        if ver < V::Lua55 {
            let atan = e[3].1;
            e.push(("atan2", atan));
            f(vm, &mut e, "cosh", m_cosh)?;
            f(vm, &mut e, "sinh", m_sinh)?;
            f(vm, &mut e, "tanh", m_tanh)?;
            f(vm, &mut e, "pow", m_pow)?;
            f(vm, &mut e, "frexp", m_frexp)?;
            f(vm, &mut e, "ldexp", m_ldexp)?;
            f(vm, &mut e, "log10", m_log10)?;
        }
        // 5.4 on, the generator's functions come last, as `setrandfunc`
        // makes them
        if ver >= V::Lua54 {
            rand(vm, &mut e)?;
        }
    }
    e.push(("pi", Value::Float(std::f64::consts::PI)));
    e.push(("huge", Value::Float(f64::INFINITY)));
    match ver {
        // LUA_COMPAT_MOD copies the `fmod` field to `mod`
        V::Lua51 => {
            let fmod = e[11].1;
            e.push(("mod", fmod));
        }
        V::Lua52 => {}
        _ => {
            e.push(("maxinteger", Value::Int(i64::MAX)));
            e.push(("mininteger", Value::Int(i64::MIN)));
        }
    }
    vm.lib_put_all(t, "math", e)?;
    vm.lib_set_global("math", Value::Table(t))?;
    vm.barrier_back_table(t);
    Ok(())
}

/// `random` and `randomseed`: from 5.4 on C closures over the generator's
/// state, which an opening that counts makes as stand-ins for PUC's
/// collectable closures (not light functions).
fn rand(vm: &mut Vm, e: &mut Vec<(&str, Value)>) -> Result<(), LuaError> {
    let ver = vm.version();
    let (r, s) = if ver >= V::Lua54 && vm.heap.puc.silent == 0 {
        let sz = vm.heap.sizes;
        use crate::runtime::heap::PucKind;
        let (ud, cl) = (
            (PucKind::Userdata, sz.userdata(0, 32)),
            (PucKind::CClosure, sz.c_closure(1)),
        );
        let fix = std::mem::replace(&mut vm.heap.fix_natives, false);
        let r = vm.lib_native_stand_in(m_random, &[], &[ud, cl], 0);
        let s = r.and_then(|r| Ok((r, vm.lib_native_stand_in(m_randomseed, &[], &[cl], 0)?)));
        vm.heap.fix_natives = fix;
        s?
    } else {
        let b = |n: &[u8]| inlinable_builtin(n).unwrap_or(Builtin::None);
        let r = vm.builtin(m_random, &[], b(b"random"))?;
        (r, vm.builtin(m_randomseed, &[], b(b"randomseed"))?)
    };
    if ver >= V::Lua54 {
        for f in [r, s] {
            if let Value::Native(n) = f {
                vm.heap.set_native_puc_nup(n, 1);
            }
        }
    }
    e.push(("random", r));
    e.push(("randomseed", s));
    Ok(())
}
