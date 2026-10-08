//! String arithmetic metamethods (5.4+, lstrlib `stringmetamethods`).

use crate::numeric::{self, Num};
use crate::runtime::Value;
use crate::vm::argcheck::Args;
use crate::vm::builtins::raise_str;
use crate::vm::error::LuaError;
use crate::vm::exec::{ArithOp, Mm, Vm, arith_num};

/// lstrlib `tonum`: a number, or a string that converts as a whole.
fn tonum(v: Value) -> Option<Num> {
    match v {
        Value::Int(i) => Some(Num::Int(i)),
        Value::Float(f) => Some(Num::Float(f)),
        Value::Str(s) => numeric::str2num(s.as_bytes(), true, true),
        _ => None,
    }
}

/// lstrlib `arith`: both operands convertible → plain arithmetic, else the
/// second operand's metamethod (`trymt`) or an error naming both types.
fn string_arith(
    vm: &mut Vm,
    fs: u32,
    nargs: u32,
    op: Option<ArithOp>,
    event: Mm,
    verb: &str,
) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let x = a.get(vm, 0);
    // `tonum` pushes the converted first operand, so with a single argument
    // that copy is what the second position then holds
    let y = match (a.is_none(1), tonum(x)) {
        (true, Some(Num::Int(i))) => Value::Int(i),
        (true, Some(Num::Float(f))) => Value::Float(f),
        _ => a.get(vm, 1),
    };
    if let (Some(nx), Some(ny)) = (tonum(x), tonum(y)) {
        // `tonum` pushed each converted operand
        vm.native_push(2);
        let r = match op {
            Some(op) => arith_num(vm.version(), op, nx, ny).map_err(|msg| vm.plain_err(msg))?,
            // unary minus works on the second (duplicated) operand
            None => match ny {
                Num::Int(i) => Value::Int(i.wrapping_neg()),
                Num::Float(f) => Value::Float(-f),
            },
        };
        return Ok(vm.nat_return(fs, &[r]));
    }
    let mm = if matches!(y, Value::Str(_)) {
        Value::Nil
    } else {
        vm.get_mm(y, event)
    };
    if mm.is_nil() {
        let msg = format!(
            "attempt to {verb} a '{}' with a '{}'",
            x.type_name(),
            y.type_name()
        );
        return Err(raise_str(vm, &msg));
    }
    // `trymt` moves the metamethod below the two operands, so the call
    // sits at the first argument; a lua_call from C, it cannot yield
    let r = vm
        .call_value_at(mm, &[x, y], fs + 1)?
        .first()
        .copied()
        .unwrap_or(Value::Nil);
    Ok(vm.nat_return(fs, &[r]))
}

pub(super) fn mm_add(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    string_arith(vm, fs, nargs, Some(ArithOp::Add), Mm::Add, "add")
}

pub(super) fn mm_sub(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    string_arith(vm, fs, nargs, Some(ArithOp::Sub), Mm::Sub, "sub")
}

pub(super) fn mm_mul(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    string_arith(vm, fs, nargs, Some(ArithOp::Mul), Mm::Mul, "mul")
}

pub(super) fn mm_mod(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    string_arith(vm, fs, nargs, Some(ArithOp::Mod), Mm::Mod, "mod")
}

pub(super) fn mm_pow(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    string_arith(vm, fs, nargs, Some(ArithOp::Pow), Mm::Pow, "pow")
}

pub(super) fn mm_div(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    string_arith(vm, fs, nargs, Some(ArithOp::Div), Mm::Div, "div")
}

pub(super) fn mm_idiv(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    string_arith(vm, fs, nargs, Some(ArithOp::IDiv), Mm::IDiv, "idiv")
}

pub(super) fn mm_unm(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    string_arith(vm, fs, nargs, None, Mm::Unm, "unm")
}
