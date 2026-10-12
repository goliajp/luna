//! Packing and unpacking integers and floats as bytes.

use super::*;

/// Pack `n` into `size` bytes, sign-extending past eight bytes when `neg`.
pub(crate) fn pack_int(
    vm: &mut Vm,
    out: &mut LVec<u8>,
    n: u64,
    islittle: bool,
    size: usize,
    neg: bool,
) -> Result<(), LuaError> {
    let at = out.len();
    vm.buf_reserve(out, size)?;
    for i in 0..size {
        let b = if i < SZINT as usize {
            (n >> (8 * i)) as u8
        } else if neg {
            0xff
        } else {
            0
        };
        vm.buf_push(out, &[b])?;
    }
    if !islittle {
        out[at..].reverse();
    }
    Ok(())
}

/// Unpack a `size`-byte integer from `bytes[..size]`, sign-extending or
/// checking the bytes past eight as PUC does.
pub(crate) fn unpack_int(
    vm: &mut Vm,
    bytes: &[u8],
    islittle: bool,
    size: usize,
    issigned: bool,
) -> Result<i64, LuaError> {
    let at = |i: usize| bytes[if islittle { i } else { size - 1 - i }];
    let limit = size.min(SZINT as usize);
    let mut res: u64 = 0;
    for i in (0..limit).rev() {
        res = (res << 8) | u64::from(at(i));
    }
    if size < SZINT as usize {
        if issigned {
            let mask = 1u64 << (size * 8 - 1);
            res = (res ^ mask).wrapping_sub(mask);
        }
    } else if size > SZINT as usize {
        let fill = if !issigned || (res as i64) >= 0 {
            0
        } else {
            0xff
        };
        if (limit..size).any(|i| at(i) != fill) {
            return Err(raise_str(
                vm,
                &format!("{size}-byte integer does not fit into Lua Integer"),
            ));
        }
    }
    Ok(res as i64)
}

pub(crate) fn float_bytes(
    vm: &mut Vm,
    out: &mut LVec<u8>,
    b: &mut [u8],
    islittle: bool,
) -> Result<(), LuaError> {
    if !islittle {
        b.reverse();
    }
    vm.buf_push(out, b)
}

/// Append `n` zero bytes.
pub(crate) fn zeros(vm: &mut Vm, out: &mut LVec<u8>, n: usize) -> Result<(), LuaError> {
    vm.buf_reserve(out, n)?;
    out.resize(out.len() + n, 0)?;
    Ok(())
}
