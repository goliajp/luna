//! Loading and dumping chunks (`lua_load`, `lua_dump`), and the text
//! `lua_ident` of each version.

use super::ccall::{take_error, with_c};
use super::*;
use luna_core::runtime::LuaClosure;
use luna_core::vm::exec::host_c::HostLoadError;

mod dump;
#[cfg(test)]
pub(super) use dump::*;

/// PUC `lua_Reader`.
pub type LuaReader =
    unsafe extern "C" fn(L: *mut LuaState, ud: *mut c_void, size: *mut usize) -> *const c_char;
/// PUC `lua_Writer`.
pub type LuaWriter =
    unsafe extern "C" fn(L: *mut LuaState, p: *const c_void, sz: usize, ud: *mut c_void) -> c_int;

// SAFETY: the declarations match the definitions in `csrc/shim_load.c`;
// each calls the host function under a fresh error boundary and returns
// normally, whatever it does. C sees `lua_State` as opaque and reads only
// its leading fields, which `csrc/shim.h` declares
#[allow(improper_ctypes)]
unsafe extern "C" {
    fn luna_c_protect_reader(
        L: *mut LuaState,
        r: LuaReader,
        ud: *mut c_void,
        size: *mut usize,
        status: *mut c_int,
    ) -> *const c_char;
    fn luna_c_protect_writer(
        L: *mut LuaState,
        w: LuaWriter,
        p: *const c_void,
        sz: usize,
        ud: *mut c_void,
        status: *mut c_int,
    ) -> c_int;
}

c_exports! {
    lua_dump => luna_c_lua_dump,
    luna_dump_51 => luna_c_luna_dump_51,
}

/// A load that failed: its status and error object.
type Failure = (c_int, Value);

/// PUC `lua_load`: read a chunk from `reader` and push it compiled, or
/// push the error and return its status. `mode` (5.2+) limits the chunk
/// to text (`t`) and/or binary (`b`); 5.5 allows both when it is null.
fn load(
    api: &mut Api,
    reader: LuaReader,
    data: *mut c_void,
    chunkname: *const c_char,
    mode: *const c_char,
) -> c_int {
    let v = api.version();
    // a collection check raising there fails the load
    let begun = api.vm.host_load_begin();
    // SAFETY: PUC's contract: a chunk name and a mode are null or
    // NUL-terminated strings
    let name = unsafe { c_bytes(chunkname) }.unwrap_or(b"?").to_vec();
    // SAFETY: as above
    let mode = unsafe { c_bytes(mode) }
        .map(<[u8]>::to_vec)
        .or_else(|| (v >= LuaVersion::Lua55).then(|| b"bt".to_vec()));
    let at = api.top();
    // 5.5's parser anchors its work in a table on the stack, under what
    // the reader may push
    let r = if let Err(e) = begun {
        Err((LUA_ERRRUN, e.0))
    } else if v >= LuaVersion::Lua55 {
        match api.vm.host_load_anchor() {
            Ok(t) => {
                api.push(Value::Table(t));
                read_and_compile(api, reader, data, mode.as_deref(), &name)
            }
            Err(e) => Err((LUA_ERRMEM, e.0)),
        }
    } else {
        read_and_compile(api, reader, data, mode.as_deref(), &name)
    };
    api.vm.host_load_end();
    match r {
        Ok(f) => {
            if v >= LuaVersion::Lua55 {
                api.truncate(at);
            }
            api.push(f);
            LUA_OK
        }
        Err((status, e)) => {
            api.truncate(at);
            api.push(e);
            status
        }
    }
}

/// Read the chunk from `reader` and compile it. The first piece tells a
/// binary chunk from text and is checked against `mode`. A text chunk is
/// parsed as it is read: the reader is called only when the parser moves
/// past the end of what it has, as PUC's parser calls it, so a syntax error
/// stops the reading. A binary chunk is read until its bytes are complete,
/// where PUC's undumper stops.
fn read_and_compile(
    api: &mut Api,
    reader: LuaReader,
    data: *mut c_void,
    mode: Option<&[u8]>,
    name: &[u8],
) -> Result<Value, Failure> {
    let mut first = Vec::new();
    // 5.1 peeks at the first character and then reads it again, so a
    // reader that signals the end at once is asked a second time
    if !read_piece(api, reader, data, &mut first)? && api.version() == LuaVersion::Lua51 {
        read_piece(api, reader, data, &mut first)?;
    }
    let binary = first.first() == Some(&0x1b);
    check_mode(api, mode, binary)?;
    let text = api.vm.text_load().filter(|_| !binary);
    let Some(text) = text else {
        let mut src = first;
        while !(binary && api.vm.host_binary_chunk_complete(&src))
            && read_piece(api, reader, data, &mut src)?
        {}
        let r = api.vm.host_load_chunk(&src, name);
        return finish(api, r);
    };
    let mut failed = None;
    let parsed = text.parse(first, &mut |buf| match read_piece(api, reader, data, buf) {
        Ok(more) => more,
        Err(f) => {
            failed = Some(f);
            false
        }
    });
    if let Some(f) = failed {
        return Err(f);
    }
    let r = api.vm.host_load_parsed(parsed, name);
    finish(api, r)
}

/// Call `reader` once and append the piece it returns to `buf`: false
/// when it signals the end (no piece, or one of no bytes).
fn read_piece(
    api: &mut Api,
    reader: LuaReader,
    data: *mut c_void,
    buf: &mut Vec<u8>,
) -> Result<bool, Failure> {
    let l = api.l;
    let mut size = 0usize;
    let mut status = LUA_OK;
    // SAFETY: `l` is the live thread the load runs on and `reader` the
    // host's reader; the boundary returns whatever it does
    let p = with_c(api.vm, l, || unsafe {
        luna_c_protect_reader(l, reader, data, &mut size, &mut status)
    });
    if status != LUA_OK {
        return Err((status, take_error(l)));
    }
    if p.is_null() || size == 0 {
        return Ok(false);
    }
    // SAFETY: a reader returns a block of `size` bytes that stays valid
    // until it is called again
    buf.extend_from_slice(unsafe { std::slice::from_raw_parts(p.cast::<u8>(), size) });
    Ok(true)
}

/// PUC `checkmode`.
fn check_mode(api: &mut Api, mode: Option<&[u8]>, binary: bool) -> Result<(), Failure> {
    let Some(mode) = mode else {
        return Ok(());
    };
    let (kind, c) = if binary {
        ("binary", b'b')
    } else {
        ("text", b't')
    };
    if mode.contains(&c) {
        return Ok(());
    }
    let mut msg = format!("attempt to load a {kind} chunk (mode is '").into_bytes();
    msg.extend_from_slice(mode);
    msg.extend_from_slice(b"')");
    Err(match api.str(&msg) {
        Ok(m) => (LUA_ERRSYNTAX, m),
        Err(e) => (LUA_ERRMEM, e.0),
    })
}

/// The function a load made, with its first upvalue set to the globals
/// as PUC's `lua_load` sets it: 5.1 closes the chunk over the thread's
/// globals, 5.2 sets the only upvalue of a function with exactly one, 5.3
/// on the first of any, to the registry's `LUA_RIDX_GLOBALS`.
fn finish(
    api: &mut Api,
    r: Result<(Gc<LuaClosure>, usize), HostLoadError>,
) -> Result<Value, Failure> {
    let (cl, n) = r.map_err(|e| match e {
        HostLoadError::Memory(v) => (LUA_ERRMEM, v),
        HostLoadError::Syntax(v) => (LUA_ERRSYNTAX, v),
    })?;
    let globals = match api.version() {
        LuaVersion::Lua51 => (n <= 1).then(|| Value::Table(api.thread_globals())),
        LuaVersion::Lua52 => (n == 1).then(|| api.vm.host_registry().get_int(2)),
        _ => (n >= 1).then(|| api.vm.host_registry().get_int(2)),
    };
    if let Some(g) = globals {
        api.vm
            .host_set_first_upvalue(cl, g)
            .map_err(|e| (LUA_ERRMEM, e.0))?;
    }
    Ok(Value::Closure(cl))
}

/// PUC 5.2+ `lua_load`.
///
/// # Safety
/// `L` is a live thread of an open state, and no other API call on it is
/// running other than a C function it is calling into; `reader` is a
/// `lua_Reader`, `chunkname` and `mode` are null or NUL-terminated.
// SAFETY: no other item in the link is named `lua_load`: the host does not
// link PUC's liblua next to this crate, which defines each `lua_*` symbol
// once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_load(
    L: *mut LuaState,
    reader: LuaReader,
    data: *mut c_void,
    chunkname: *const c_char,
    mode: *const c_char,
) -> c_int {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    load(&mut api, reader, data, chunkname, mode)
}

/// PUC 5.1 `lua_load`, which has no mode.
///
/// # Safety
/// As [`lua_load`].
// SAFETY: no other item in the link is named `luna_load_51`: PUC's liblua
// has no such symbol and this crate defines it once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_load_51(
    L: *mut LuaState,
    reader: LuaReader,
    data: *mut c_void,
    chunkname: *const c_char,
) -> c_int {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    load(&mut api, reader, data, chunkname, std::ptr::null())
}

/// `s` as a NUL-terminated array, for the `lua_ident` statics.
const fn cstr<const N: usize>(s: &str) -> [u8; N] {
    let b = s.as_bytes();
    assert!(b.len() + 1 == N);
    let mut a = [0u8; N];
    let mut i = 0;
    while i < b.len() {
        a[i] = b[i];
        i += 1;
    }
    a
}

macro_rules! ident {
    ($($name:ident = $text:expr;)*) => {
        $(
            /// PUC's `lua_ident` of one version.
            // SAFETY: no other item in the link has this name: PUC's liblua
            // is not linked next to this crate, and each name is defined
            // once
            #[unsafe(no_mangle)]
            #[allow(non_upper_case_globals)]
            pub static $name: [u8; $text.len() + 1] = cstr($text);
        )*
    };
}

ident! {
    luna_ident_51 = "$Lua: Lua 5.1.5 Copyright (C) 1994-2012 Lua.org, PUC-Rio $\n\
                     $Authors: R. Ierusalimschy, L. H. de Figueiredo & W. Celes $\n\
                     $URL: www.lua.org $\n";
    luna_ident_52 = "$LuaVersion: Lua 5.2.4  Copyright (C) 1994-2015 Lua.org, PUC-Rio $\
                     $LuaAuthors: R. Ierusalimschy, L. H. de Figueiredo, W. Celes $";
    luna_ident_53 = "$LuaVersion: Lua 5.3.6  Copyright (C) 1994-2020 Lua.org, PUC-Rio $\
                     $LuaAuthors: R. Ierusalimschy, L. H. de Figueiredo, W. Celes $";
    luna_ident_54 = "$LuaVersion: Lua 5.4.9  Copyright (C) 1994-2026 Lua.org, PUC-Rio $\
                     $LuaAuthors: R. Ierusalimschy, L. H. de Figueiredo, W. Celes $";
    lua_ident = "$LuaVersion: Lua 5.5.1  Copyright (C) 1994-2026 Lua.org, PUC-Rio $\
                 $LuaAuthors: R. Ierusalimschy, L. H. de Figueiredo, W. Celes $";
}
