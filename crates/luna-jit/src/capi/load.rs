//! Loading and dumping chunks (`lua_load`, `lua_dump`), and the text
//! `lua_ident` of each version.

use super::ccall::{take_error, with_c};
use super::*;
use luna_core::vm::exec::host_c::HostChunkProgress;

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
    // SAFETY: PUC's contract: a chunk name and a mode are null or
    // NUL-terminated strings
    let name = unsafe { c_bytes(chunkname) }.unwrap_or(b"?").to_vec();
    // SAFETY: as above
    let mode = unsafe { c_bytes(mode) }
        .map(<[u8]>::to_vec)
        .or_else(|| (v >= LuaVersion::Lua55).then(|| b"bt".to_vec()));
    let at = api.top();
    if v >= LuaVersion::Lua52 {
        api.vm.host_nny_enter();
    }
    // 5.5's parser anchors its work in a table on the stack, under what
    // the reader may push
    if v >= LuaVersion::Lua55 {
        let t = Value::Table(api.vm.heap.new_table());
        api.push(t);
    }
    let r =
        read_chunk(api, reader, data, mode.as_deref()).and_then(|src| compile(api, &src, &name));
    if v >= LuaVersion::Lua52 {
        api.vm.host_nny_leave();
    }
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

/// Call `reader` until it signals the end, or until PUC's parser would
/// have stopped asking (see `Vm::host_chunk_progress`), checking the kind
/// of chunk against `mode` once its first byte is known.
fn read_chunk(
    api: &mut Api,
    reader: LuaReader,
    data: *mut c_void,
    mode: Option<&[u8]>,
) -> Result<Vec<u8>, Failure> {
    let l = api.l;
    let mut src = Vec::new();
    // 5.1 peeks at the first character and then reads it again, so a
    // reader that signals the end at once is asked a second time
    let mut peeked = api.version() != LuaVersion::Lua51;
    loop {
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
            if src.is_empty() && !peeked {
                peeked = true;
                continue;
            }
            break;
        }
        let first = src.is_empty();
        // SAFETY: a reader returns a block of `size` bytes that stays valid
        // until it is called again
        src.extend_from_slice(unsafe { std::slice::from_raw_parts(p.cast::<u8>(), size) });
        if first {
            check_mode(api, mode, src[0] == 0x1b)?;
        }
        if api.vm.host_chunk_progress(&src) == HostChunkProgress::Done {
            return Ok(src);
        }
    }
    if src.is_empty() {
        check_mode(api, mode, false)?;
    }
    Ok(src)
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
    Err((LUA_ERRSYNTAX, api.str(&msg)))
}

/// Compile `src` and give its first upvalue the globals as PUC's
/// `lua_load` does: 5.1 closes the chunk over the thread's globals, 5.2
/// sets the only upvalue of a function with exactly one, 5.3 on the first
/// of any, to the registry's `LUA_RIDX_GLOBALS`.
fn compile(api: &mut Api, src: &[u8], name: &[u8]) -> Result<Value, Failure> {
    let (cl, n) = api
        .vm
        .host_load_chunk(src, name)
        .map_err(|e| (LUA_ERRSYNTAX, e))?;
    let globals = match api.version() {
        LuaVersion::Lua51 => (n <= 1).then(|| Value::Table(api.thread_globals())),
        LuaVersion::Lua52 => (n == 1).then(|| api.vm.host_registry().get_int(2)),
        _ => (n >= 1).then(|| api.vm.host_registry().get_int(2)),
    };
    if let Some(g) = globals {
        api.vm.host_set_first_upvalue(cl, g);
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

/// PUC `lua_dump`: hand the binary chunk of the Lua function on top of the
/// stack to `writer`, and return the first nonzero status it returns, or
/// 1 when the value is not a Lua function. 5.5 also signals the end with a
/// call of no bytes. An error the writer raises leaves `lua_dump`.
fn dump(api: &mut Api, writer: LuaWriter, data: *mut c_void, strip: bool) -> c_int {
    let f = api.get_or_nil(-1);
    let Some(bytes) = api.vm.host_dump(f, strip) else {
        return 1;
    };
    let v55 = api.version() >= LuaVersion::Lua55;
    let at = api.top();
    if v55 {
        // 5.5's dump keeps a table of the strings it wrote on the stack
        let t = Value::Table(api.vm.heap.new_table());
        api.push(t);
    }
    let mut status = write(api, writer, bytes.as_ptr().cast(), bytes.len(), data);
    if v55 && status == 0 && !api.raised() {
        status = write(api, writer, std::ptr::null(), 0, data);
    }
    if !api.raised() {
        api.truncate(at);
    }
    status
}

/// One call of the writer; an error it raises is passed on to the C
/// wrapper, which throws it.
fn write(api: &mut Api, w: LuaWriter, p: *const c_void, sz: usize, ud: *mut c_void) -> c_int {
    let l = api.l;
    let mut status = LUA_OK;
    // SAFETY: `l` is the live thread the dump runs on, `w` the host's
    // writer and `p` `sz` readable bytes (or null with 0)
    let r = with_c(api.vm, l, || unsafe {
        luna_c_protect_writer(l, w, p, sz, ud, &mut status)
    });
    if status != LUA_OK {
        // the error object is where the raise that threw it left it
        api.g().raised = status;
        return 0;
    }
    r
}

/// PUC 5.3+ `lua_dump`.
///
/// # Safety
/// `L` is a live thread of an open state, the innermost API call on it;
/// called by the C wrapper, which throws what this raises.
// SAFETY: no other item in the link is named `luna_capi_lua_dump`; the C
// wrapper `luna_c_lua_dump` is its only caller
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_capi_lua_dump(
    L: *mut LuaState,
    writer: LuaWriter,
    data: *mut c_void,
    strip: c_int,
) -> c_int {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    dump(&mut api, writer, data, strip != 0)
}

/// PUC 5.1/5.2 `lua_dump`, which has no strip flag.
///
/// # Safety
/// As [`luna_capi_lua_dump`].
// SAFETY: no other item in the link is named `luna_capi_luna_dump_51`; the
// C wrapper is its only caller
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_capi_luna_dump_51(
    L: *mut LuaState,
    writer: LuaWriter,
    data: *mut c_void,
) -> c_int {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    dump(&mut api, writer, data, false)
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
