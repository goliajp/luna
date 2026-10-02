//! `load` and 5.1 `loadstring`.

use super::*;
use crate::runtime::{Gc, LuaStr};

/// `load`. 5.1 only loads from a reader function (`loadstring` takes the
/// strings); 5.2+ takes a string — or a number, which `lua_tolstring`
/// renders — or a reader, then an optional chunk name, mode and env.
pub(crate) fn nat_load(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    if vm.version() == LuaVersion::Lua51 {
        let name = argcheck::opt_string(vm, a, 1)?;
        let reader = argcheck::check_function(vm, a, 0)?;
        let name = name.map_or_else(|| b"=(load)".to_vec(), |n| n.as_bytes().to_vec());
        return match read_chunk(vm, reader)? {
            Ok(src) => load_chunk(vm, a, &src, ChunkName::Bytes(&name), None),
            Err(msg) => Ok(vm.nat_return(fs, &[Value::Nil, msg])),
        };
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
    match read_chunk(vm, reader)? {
        Ok(src) => load_chunk(vm, a, &src, ChunkName::Bytes(&name), mode),
        Err(msg) => Ok(vm.nat_return(fs, &[Value::Nil, msg])),
    }
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

/// Drain a `load` reader: it is called until it returns nil, no value or
/// the empty string. A string or number piece is appended; any other value,
/// or an error the reader raises, fails the load softly — `Ok(Err(msg))`
/// is the message `load` returns after its nil.
fn read_chunk(vm: &mut Vm, reader: Value) -> Result<Result<Vec<u8>, Value>, LuaError> {
    // PUC's parser reads from the reader incrementally — it lexes
    // one token at a time with a one-char lookahead, so even an
    // immediately-syntactically-invalid chunk like `"*a = 123"`
    // pulls exactly 2 reader calls before the parser bails. luna's
    // parser is whole-buffer, so we approximate by trying a parse
    // after the first 2 bytes arrive: a *definitive* syntax error
    // (not an "expected <eof>" / "near '<eof>'" wall that just
    // signals "needs more input") returns immediately, with the
    // reader having been called exactly twice. 5.1 calls.lua :250
    // pins `i == 2`.
    let mut buf = Vec::new();
    let mut try_early = true;
    // Snapshot the loader budget once — embedders are not
    // expected to widen the cap mid-`load`.
    let input_budget = vm.loader_input_budget();
    loop {
        // the reader runs in a protected context (PUC protectedparser):
        // an error it raises becomes a soft load failure
        // the parser runs with the thread non-yieldable (`nny` is raised
        // around `lua_load`), so the reader cannot yield
        let r = match vm.call_noyield(reader, &[]) {
            Ok(r) => r,
            Err(e) => return Ok(Err(e.0)),
        };
        let piece = match r.first() {
            None | Some(Value::Nil) => break,
            Some(&v) => match argcheck::to_str_bytes(vm, v) {
                Some(b) => b,
                None => {
                    let m = Value::Str(vm.heap.intern(b"reader function must return a string"));
                    return Ok(Err(m));
                }
            },
        };
        if piece.is_empty() {
            break;
        }
        // Gate the next chunk *before* we extend the
        // buffer: PUC's `loadrep` feeder returns a 1 MiB
        // string every iteration and runs forever; with
        // the default 256 MiB cap we error out after the
        // first quarter-gig instead of letting the host
        // allocator crawl past 7 GB then SIGSEGV.
        // Matches the PUC `not enough memory` failure
        // shape that `heavy.lua::loadrep` asserts on.
        if piece.len() > input_budget.saturating_sub(buf.len()) {
            return Ok(Err(Value::Str(vm.heap.intern(b"not enough memory"))));
        }
        buf.extend_from_slice(&piece);
        if try_early && buf.len() >= 2 && !crate::vm::dump::is_binary_chunk(&buf) {
            try_early = false;
            let ver = vm.version();
            if let Err(e) = crate::frontend::parse(&buf, ver) {
                let msg_str = String::from_utf8_lossy(&e.msg);
                let eof_related = msg_str.contains("<eof>") || msg_str.contains("near eof");
                if !eof_related {
                    // definitive error — leave the source as is; the
                    // post-loop parse at the same call site re-runs
                    // it and produces the user-facing failure.
                    break;
                }
            }
        }
    }
    Ok(Ok(buf))
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
    let kind: &[u8] = if binary { b"b" } else { b"t" };
    // `strchr`: the mode is a C string, so it ends at the first NUL.
    if let Some(mode) = mode.map(|m| &m[..m.iter().position(|&c| c == 0).unwrap_or(m.len())])
        && !mode.contains(&kind[0])
    {
        let msg = format!(
            "attempt to load a {} chunk (mode is '{}')",
            if binary { "binary" } else { "text" },
            String::from_utf8_lossy(mode)
        );
        let m = Value::Str(vm.heap.intern(msg.as_bytes()));
        return Ok(vm.nat_return(a.fs, &[Value::Nil, m]));
    }
    match vm.load_named(src, name, name_str) {
        Ok(cl) => {
            // `lua_setupvalue(L, -2, 1)`: a function without upvalues
            // ignores the env.
            if vm.version() >= LuaVersion::Lua52 && !a.is_none(3) && !cl.upvals().is_empty() {
                let env = a.get(vm, 3);
                let uv = vm.heap.new_upvalue(crate::runtime::UpvalState::Closed(env));
                // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
                unsafe { cl.as_mut() }.upvals_mut()[0] = uv;
            }
            Ok(vm.nat_return(a.fs, &[Value::Closure(cl)]))
        }
        Err(e) => {
            // PUC formats the syntax error's source prefix via `luaO_chunkid`
            // (see `syntax_chunk_id`), not as a bare `[string "<name>"]`. This handles
            // the `@file` / `=name` sigils and head/tail-truncation rules.
            // `e.msg` carries raw bytes (PUC's near-token may be a non-UTF-8
            // byte from the source) — splice it in as-is so 5.1 errors.lua
            // can pattern-match `near '\xff'` etc.
            let m = vm.load_error_value(&e, name);
            Ok(vm.nat_return(a.fs, &[Value::Nil, m]))
        }
    }
}

/// The name of a chunk `load` was given: bytes, or a string the chunk can
/// keep as its source name without making another.
enum ChunkName<'a> {
    Bytes(&'a [u8]),
    Str(&'a Gc<LuaStr>),
}
