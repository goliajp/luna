//! `searchpath` (5.1's `findfile`) as PUC runs it, for what it makes: the
//! strings of `luaL_gsub`, the templates, the file names and the pieces of
//! the error message, in PUC's order, each through the `luaL_Buffer` model.

use super::*;
use crate::runtime::LuaStr;
use crate::runtime::puc::fmt::FmtArg;
use crate::vm::puc_buffer::PucBuffer;

/// What a search found: the file name's bytes, where 5.4 on has them in
/// its buffer's frame (see `found_string`), and the string ≤5.3 made of
/// them.
pub(super) struct Found {
    pub(super) file: Vec<u8>,
    pub(super) at: Option<u32>,
    pub(super) made: Option<Gc<LuaStr>>,
}

/// The first readable file among `path`'s templates with `name` (its
/// `sep`s turned into `dirsep`) in place of each '?', or the message
/// listing every file tried.
pub(super) fn search(
    vm: &mut Vm,
    name: &[u8],
    path: &[u8],
    sep: &[u8],
    dirsep: &[u8],
) -> Result<Result<Found, Gc<LuaStr>>, LuaError> {
    let base = vm.lib_roots.mark();
    let r = if vm.version() >= LuaVersion::Lua54 {
        search_54(vm, name, path, sep, dirsep)
    } else {
        search_51(vm, name, path, sep, dirsep)
    };
    vm.lib_roots.release(base);
    r
}

/// 5.4 on: the names in one buffer, the message made only when none is
/// readable.
fn search_54(
    vm: &mut Vm,
    name: &[u8],
    path: &[u8],
    sep: &[u8],
    dirsep: &[u8],
) -> Result<Result<Found, Gc<LuaStr>>, LuaError> {
    let name = match sep.first() {
        Some(&s) if name.contains(&s) => gsub(vm, name, sep, dirsep)?.0,
        _ => name.to_vec(),
    };
    let mut b = PucBuffer::new(vm);
    let mut out = Vec::new();
    addgsub(vm, &mut b, &mut out, path, b"?", &name)?;
    out.push(0);
    b.add_chars(vm, &out, 1)?;
    let expanded = &out[..out.len() - 1];
    // the names are cut from the buffer in `searchpath`'s frame, or in a
    // box when they do not fit there
    let in_frame = out.len() <= Vm::lual_buffersize(vm.version());
    let mut off = 0;
    for file in expanded.split(|&c| c == b';') {
        if readable(file) {
            let at = in_frame.then_some(off as u32);
            return Ok(Ok(Found {
                file: file.to_vec(),
                at,
                made: None,
            }));
        }
        off += file.len() + 1;
    }
    let pushed = b.finish(vm, &out)?;
    vm.lib_roots.push(Value::Str(pushed));
    // `pusherrornotfound(L, lua_tostring(L, -1))`
    let mut b = PucBuffer::new(vm);
    let mut msg = b"no file '".to_vec();
    b.add(vm, &msg, msg.len())?;
    addgsub(vm, &mut b, &mut msg, expanded, b";", b"'\n\tno file '")?;
    msg.push(b'\'');
    b.add(vm, &msg, 1)?;
    Ok(Err(b.finish(vm, &msg)?))
}

/// 5.1 to 5.3: each template pushed, its file name made with `luaL_gsub`,
/// and each failure's entry added to the message (a buffer from 5.2, a
/// concatenation in 5.1).
fn search_51(
    vm: &mut Vm,
    name: &[u8],
    path: &[u8],
    sep: &[u8],
    dirsep: &[u8],
) -> Result<Result<Found, Gc<LuaStr>>, LuaError> {
    let v51 = vm.version() == LuaVersion::Lua51;
    let mut msg = if v51 {
        None
    } else {
        Some((PucBuffer::new(vm), Vec::new()))
    };
    // 5.1 always replaces the name's dots; 5.2 and 5.3 when the separator
    // is not empty
    let name = if v51 || !sep.is_empty() {
        gsub(vm, name, sep, dirsep)?.0
    } else {
        name.to_vec()
    };
    // 5.1's accumulator, `lua_pushliteral(L, "")`
    let mut acc = if v51 {
        Some(vm.api_push_string(b"")?)
    } else {
        None
    };
    if let Some(a) = acc {
        vm.lib_roots.push(a);
    }
    let mut rest = path;
    loop {
        while rest.first() == Some(&b';') {
            rest = &rest[1..];
        }
        if rest.is_empty() {
            break;
        }
        let end = rest.iter().position(|&c| c == b';').unwrap_or(rest.len());
        let (tpl, next) = rest.split_at(end);
        rest = next;
        vm.api_pushing(|vm| vm.alloc_str(tpl))?;
        let (file, made) = gsub(vm, tpl, b"?", &name)?;
        // `lua_remove(L, -2)`: the template goes, the file name stays
        remove_below_top(vm);
        if readable(&file) {
            return Ok(Ok(Found {
                file,
                at: None,
                made: Some(made),
            }));
        }
        let entry = vm.api_pushfstring(1, b"\n\tno file '%s'", &[FmtArg::S(&file)])?;
        vm.lib_roots.push(Value::Str(entry));
        // and the file name, the entry above it
        remove_below_top(vm);
        match (&mut msg, &mut acc) {
            (Some((b, out)), _) => {
                out.extend_from_slice(entry.as_bytes());
                b.add_value(vm, out, entry)?;
                vm.lib_roots.pop();
            }
            (None, Some(a)) => {
                // `lua_concat(L, 2)` of the accumulator and the entry: both
                // replaced by the result
                vm.api_check_before()?;
                let joined = crate::vm::builtins::concat2(vm, *a, Value::Str(entry))?;
                vm.lib_roots.pop_n(2);
                vm.lib_roots.push(joined);
                *a = joined;
            }
            (None, None) => unreachable!("5.1 accumulates, 5.2 and 5.3 buffer"),
        }
    }
    match (msg, acc) {
        (Some((b, out)), _) => Ok(Err(b.finish(vm, &out)?)),
        (None, Some(Value::Str(s))) => Ok(Err(s)),
        _ => unreachable!("the message is a string"),
    }
}

/// `lua_remove(L, -2)` of the values `lib_roots` stands for.
fn remove_below_top(vm: &mut Vm) {
    let n = vm.lib_roots.len();
    vm.lib_roots.remove(n - 2);
}

/// `luaL_gsub(L, s, p, r)`: the result's bytes and the string pushed.
fn gsub(vm: &mut Vm, s: &[u8], p: &[u8], r: &[u8]) -> Result<(Vec<u8>, Gc<LuaStr>), LuaError> {
    let mut b = PucBuffer::new(vm);
    let mut out = Vec::new();
    addgsub(vm, &mut b, &mut out, s, p, r)?;
    let made = b.finish(vm, &out)?;
    vm.lib_roots.push(Value::Str(made));
    Ok((out, made))
}

/// `luaL_addgsub` (inside `luaL_gsub` before 5.4): each run before a `p`
/// added, then `r`, then what is left.
fn addgsub(
    vm: &mut Vm,
    b: &mut PucBuffer,
    out: &mut Vec<u8>,
    s: &[u8],
    p: &[u8],
    r: &[u8],
) -> Result<(), LuaError> {
    let mut rest = s;
    while let Some(i) = (!p.is_empty())
        .then(|| rest.windows(p.len()).position(|w| w == p))
        .flatten()
    {
        out.extend_from_slice(&rest[..i]);
        b.add(vm, out, i)?;
        out.extend_from_slice(r);
        b.add(vm, out, r.len())?;
        rest = &rest[i + p.len()..];
    }
    out.extend_from_slice(rest);
    b.add(vm, out, rest.len())
}
