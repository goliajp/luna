//! The `luaL_Buffer` PUC's readers fill: what it allocates for the bytes
//! read, given the chunks each reader asks room for and fills.

use super::*;
use crate::vm::puc_buffer::PucBuffer;

/// The string of `bytes`, read in `chunks`: each `(room, got)` a
/// `luaL_prepbuffsize(room)` (5.1's `luaL_prepbuffer`) and a
/// `luaL_addsize(got)`; `nl`: one more byte, a newline added
/// (`luaL_addchar`).
pub(crate) fn buffered(
    vm: &mut Vm,
    bytes: &[u8],
    chunks: &[(usize, usize)],
    nl: bool,
) -> Result<Value, RdErr> {
    if bytes.len() > crate::runtime::string::MAX_LEN {
        return Err(posix_error(ENOMEM).into());
    }
    // 5.1's buffer keeps its strings on the stack: an error lets them go
    vm.lib_scope(|vm| {
        let mut b = PucBuffer::new(vm);
        let mut at = 0;
        for &(room, got) in chunks {
            b.prep(vm, &bytes[..at], room)?;
            at += got;
            b.add_size(got);
        }
        if nl {
            at += 1;
            b.add_chars(vm, &bytes[..at], 1)?;
        }
        Ok(Value::Str(b.finish(vm, &bytes[..at])?))
    })
}

/// `LUAL_BUFFERSIZE`.
pub(crate) fn chunk_size(vm: &Vm) -> usize {
    Vm::lual_buffersize(vm.version())
}

/// Chunks of `size` bytes, the last one short (perhaps empty), as a reader
/// fills them until one comes back short: `total` bytes in all.
pub(crate) fn even_chunks(size: usize, total: usize) -> Vec<(usize, usize)> {
    let full = total / size;
    let mut c = vec![(size, size); full];
    c.push((size, total - full * size));
    c
}

/// `read_all` of `total` bytes: 5.2 doubles the room each round; the
/// others ask the same size each time.
pub(crate) fn all_chunks(vm: &Vm, total: usize) -> Vec<(usize, usize)> {
    let size = chunk_size(vm);
    if vm.version() != LuaVersion::Lua52 {
        return even_chunks(size, total);
    }
    let (mut c, mut room, mut left) = (Vec::new(), size, total);
    loop {
        let got = left.min(room);
        c.push((room, got));
        left -= got;
        if got < room {
            return c;
        }
        room *= 2;
    }
}

/// `read_chars` of `n` bytes that gave `got`: 5.1 in chunks of
/// `LUAL_BUFFERSIZE` (the last one asking for what is left), the others
/// room for all `n` at once.
pub(crate) fn count_chunks(vm: &Vm, n: usize, got: usize) -> Vec<(usize, usize)> {
    if vm.version() != LuaVersion::Lua51 {
        return vec![(n, got)];
    }
    let size = chunk_size(vm);
    let (mut c, mut want, mut left) = (Vec::new(), n, got);
    loop {
        let room = want.min(size);
        let g = left.min(room);
        c.push((room, g));
        want -= g;
        left -= g;
        if want == 0 || g < room {
            return c;
        }
    }
}
