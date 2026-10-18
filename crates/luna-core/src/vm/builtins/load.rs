//! `load` and 5.1 `loadstring`.

use super::*;
use crate::runtime::{Gc, LuaStr};

mod reader;
use reader::{Phase, ReaderSlot, next_piece};

/// `load`. 5.1 only loads from a reader function (`loadstring` takes the
/// strings); 5.2+ takes a string — or a number, which `lua_tolstring`
/// renders — or a reader, then an optional chunk name, mode and env.
pub(crate) fn nat_load(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    if vm.version() == LuaVersion::Lua51 {
        let name = argcheck::opt_string(vm, a, 1)?;
        let reader = argcheck::check_function(vm, a, 0)?;
        let name = name.map_or_else(|| b"=(load)".to_vec(), |n| n.as_bytes().to_vec());
        return load_reader(vm, a, reader, &name, None);
    }
    let first = a.get(vm, 0);
    let mode = load_mode(vm, a, 2)?;
    // 5.2-5.4 default to "bt"; 5.5 has no default and allows both
    let mode = match &mode {
        Some(m) => Some(m.as_bytes()),
        None if vm.version() < LuaVersion::Lua55 => Some(&b"bt"[..]),
        None => None,
    };
    let name = argcheck::opt_string(vm, a, 1)?;
    // a string chunk is read in place: the argument keeps it alive
    if let Value::Str(src) = first {
        let name = name.unwrap_or(src);
        return load_chunk(vm, a, src.as_bytes(), ChunkName::Str(&name), mode);
    }
    if let Some(src) = argcheck::to_str_bytes(vm, first) {
        let name = name.map_or_else(|| src.clone(), |n| n.as_bytes().to_vec());
        return load_chunk(vm, a, &src, ChunkName::Bytes(&name), mode);
    }
    let name = name.map_or_else(|| b"=(load)".to_vec(), |n| n.as_bytes().to_vec());
    let reader = argcheck::check_function(vm, a, 0)?;
    load_reader(vm, a, reader, &name, mode)
}

/// 5.1 `loadstring(s [, chunkname])`: the source is a string (or a number,
/// rendered), and the chunk name defaults to the source itself.
pub(super) fn nat_loadstring(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let src = argcheck::check_string(vm, a, 0)?;
    let name = argcheck::opt_string(vm, a, 1)?.unwrap_or(src);
    load_chunk(vm, a, src.as_bytes(), ChunkName::Str(&name), None)
}

/// The `mode` argument of 5.2+ `load`: `luaL_optstring` with the dialect's
/// default ("bt" up to 5.4, none on 5.5, where an absent mode allows both).
/// 5.5 refuses 'B' (a fixed-buffer chunk, which Lua code cannot supply).
fn load_mode(vm: &mut Vm, a: Args, i: u32) -> Result<Option<Gc<LuaStr>>, LuaError> {
    let mode = argcheck::opt_string(vm, a, i)?;
    if vm.version() >= LuaVersion::Lua55 && mode.is_some_and(|m| m.as_bytes().contains(&b'B')) {
        return Err(arg_error(vm, i + 1, "invalid mode"));
    }
    Ok(mode)
}

/// `load` with a reader function. A text chunk is parsed as the reader
/// hands it over, and the reader is called only when the parser moves past
/// the end of what it has, as PUC's parser does: a syntax error stops the
/// reading. A binary chunk is read to the end first.
fn load_reader(
    vm: &mut Vm,
    a: Args,
    reader: Value,
    name: &[u8],
    mode: Option<&[u8]>,
) -> Result<u32, LuaError> {
    let r = read_and_load(vm, a, reader, name, mode);
    vm.host_load_end();
    r
}

fn read_and_load(
    vm: &mut Vm,
    a: Args,
    reader: Value,
    name: &[u8],
    mode: Option<&[u8]>,
) -> Result<u32, LuaError> {
    let fail = |vm: &mut Vm, msg: Value| vm.nat_ret(a.fs, &[Value::Nil, msg]);
    let mut rs = ReaderSlot::new(vm, a)?;
    if let Err(e) = vm.host_load_begin() {
        return fail(vm, e.0);
    }
    // what each call of the reader counts is set aside, to be counted
    // where PUC's parser calls it: at the byte the piece starts at
    let counted = vm.heap.puc.silent == 0;
    let mut splices = Vec::new();
    let mut fed = 0u32;
    let mut spliced = |vm: &mut Vm, rs: &mut ReaderSlot, at: u32, phase: Phase| {
        let mark = counted.then(|| vm.heap.defer_begin());
        let r = next_piece(vm, reader, rs, phase, name);
        if let Some(m) = mark {
            splices.push((at, vm.heap.defer_end(m)));
        }
        r
    };
    let mut first = match spliced(vm, &mut rs, 0, Phase::Start) {
        Ok(p) => p,
        Err(msg) => return fail(vm, msg),
    };
    // 5.1 peeks at the first character and then reads it again, so a
    // reader that signals the end at once is asked a second time
    // (from the scanner, the main function open)
    if first.is_none() && vm.version() == LuaVersion::Lua51 {
        first = match spliced(vm, &mut rs, 0, Phase::Text(1, b"")) {
            Ok(p) => p,
            Err(msg) => return fail(vm, msg),
        };
    }
    let first = first.unwrap_or_default();
    let text = vm
        .text_load()
        .filter(|_| !crate::vm::dump::is_binary_chunk(&first));
    let Some(text) = text else {
        // the undumper (and MacroLua's macro pass) take the whole chunk
        let budget = vm.loader_input_budget();
        let mut buf = crate::runtime::mem::LVec::from_slice(vm.heap.mem(), &first)?;
        // how far PUC's undumper gets, as the bytes come
        let mut undumping = crate::vm::dump::Undumping::new(false);
        // what the debug library was shown of the undumper's objects
        let mut shown = crate::vm::exec::reader_layers::UndumpShown::default();
        loop {
            undumping.feed(&buf);
            // PUC reads a binary chunk no further than its end
            if undumping.done {
                break;
            }
            match next_piece(
                vm,
                reader,
                &mut rs,
                Phase::Binary(&buf, &undumping, &mut shown),
                name,
            ) {
                Ok(Some(p)) if p.len() > budget.saturating_sub(buf.len()) => {
                    let m = vm.mem_err().0;
                    return fail(vm, m);
                }
                Ok(Some(p)) => {
                    if let Err(o) = buf.extend_from_slice(&p) {
                        let m = LuaError::from(o).0;
                        return fail(vm, m);
                    }
                }
                Ok(None) => break,
                Err(msg) => return fail(vm, msg),
            }
        }
        return load_chunk(vm, a, &buf, ChunkName::Bytes(name), mode);
    };
    if let Some(m) = mode_refusal(vm, mode, false)? {
        return fail(vm, m);
    }
    let mut failed = None;
    fed += first.len() as u32;
    let parsed = text.parse_open(first, &mut |buf, open, src| match spliced(
        vm,
        &mut rs,
        fed,
        Phase::Text(open, src),
    ) {
        Ok(Some(p)) => {
            fed += p.len() as u32;
            buf.extend_from_slice(&p);
            true
        }
        Ok(None) => false,
        Err(msg) => {
            failed = Some(msg);
            false
        }
    });
    if let Some(msg) = failed {
        return fail(vm, msg);
    }
    let r = vm.load_parsed_spliced(parsed, name, None, splices);
    loaded(vm, a, r, name)
}

/// PUC `checkmode`: the message `load` returns when `mode` does not allow
/// a binary or text chunk.
fn mode_refusal(vm: &mut Vm, mode: Option<&[u8]>, binary: bool) -> Result<Option<Value>, LuaError> {
    // `strchr`: the mode is a C string, so it ends at the first NUL.
    let Some(mode) = mode.map(|m| &m[..m.iter().position(|&c| c == 0).unwrap_or(m.len())]) else {
        return Ok(None);
    };
    let kind = if binary { b'b' } else { b't' };
    if mode.contains(&kind) {
        return Ok(None);
    }
    let what: &[u8] = if binary { b"binary" } else { b"text" };
    let msg = vm.concat_str(&[
        b"attempt to load a ",
        what,
        b" chunk (mode is '",
        mode,
        b"')",
    ])?;
    Ok(Some(msg))
}

/// Compile `src` as chunk `name` and return `load`'s results: the function,
/// or nil and the message. `mode` restricts text/binary chunks
/// (`checkmode` in ldo.c); a 4th argument, when present, becomes the
/// function's first upvalue (5.2+ `load`).
fn load_chunk(
    vm: &mut Vm,
    a: Args,
    src: &[u8],
    name: ChunkName<'_>,
    mode: Option<&[u8]>,
) -> Result<u32, LuaError> {
    let (name, name_str) = match name {
        ChunkName::Bytes(b) => (b, None),
        ChunkName::Str(s) => (s.as_bytes(), Some(*s)),
    };
    let binary = crate::vm::dump::is_binary_chunk(src);
    if let Some(m) = mode_refusal(vm, mode, binary)? {
        return vm.nat_ret(a.fs, &[Value::Nil, m]);
    }
    let r = vm.load_named(src, name, name_str);
    loaded(vm, a, r, name)
}

/// `load`'s results for a load that made `r`: the function, or nil and the
/// message. A 4th argument, when present, becomes the function's first
/// upvalue (5.2+ `load`).
fn loaded(
    vm: &mut Vm,
    a: Args,
    r: Result<Gc<crate::runtime::LuaClosure>, crate::frontend::SyntaxError>,
    name: &[u8],
) -> Result<u32, LuaError> {
    match r {
        Ok(cl) => {
            // `lua_setupvalue(L, -2, 1)`: a function without upvalues
            // ignores the env.
            if vm.version() >= LuaVersion::Lua52 && !a.is_none(3) && !cl.upvals().is_empty() {
                let env = a.get(vm, 3);
                let uv = vm.lib_closed_upvalue(env)?;
                // SAFETY: `cl` is the closure the load just built, held only by this local, and `new_upvalue` does not collect; the borrow covers one slot store, inside the bounds checked by `upvals().is_empty()`
                unsafe { cl.as_mut() }.upvals_mut()[0] = uv;
            }
            vm.nat_ret(a.fs, &[Value::Closure(cl)])
        }
        Err(e) => {
            // PUC formats the syntax error's source prefix via `luaO_chunkid`
            // (see `syntax_chunk_id`), not as a bare `[string "<name>"]`. This handles
            // the `@file` / `=name` sigils and head/tail-truncation rules.
            // `e.msg` carries raw bytes (PUC's near-token may be a non-UTF-8
            // byte from the source) — splice it in as-is so 5.1 errors.lua
            // can pattern-match `near '\xff'` etc.
            let m = vm.load_error_value(&e, name);
            vm.nat_ret(a.fs, &[Value::Nil, m])
        }
    }
}

/// The name of a chunk `load` was given: bytes, or a string the chunk can
/// keep as its source name without making another.
enum ChunkName<'a> {
    Bytes(&'a [u8]),
    Str(&'a Gc<LuaStr>),
}
