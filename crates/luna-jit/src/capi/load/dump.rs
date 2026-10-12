//! `lua_dump`: a Lua function's binary chunk handed to a writer.

use super::*;

/// PUC `lua_dump`: hand the binary chunk of the Lua function on top of the
/// stack to `writer` one block at a time, as the dialect's `ldump.c` cuts
/// it, until the writer returns nonzero; return that status, or 1 when the
/// value is not a Lua function. 5.5 also signals the end with a call of no
/// bytes. An error the writer raises leaves `lua_dump`.
fn dump(api: &mut Api, writer: LuaWriter, data: *mut c_void, strip: bool) -> c_int {
    let f = api.get_or_nil(-1);
    let Some((bytes, blocks)) = api.vm.host_dump(f, strip) else {
        return 1;
    };
    let v55 = api.version() >= LuaVersion::Lua55;
    let at = api.top();
    if v55 {
        // 5.5's dump keeps a table of the strings it wrote on the stack
        let t = or_raise!(api, api.vm.host_new_table(0, 0), 0);
        api.push(Value::Table(t));
    }
    let mut status = 0;
    let mut at_byte = 0;
    for n in blocks {
        if status != 0 || api.raised() {
            break;
        }
        let p = bytes[at_byte..].as_ptr().cast();
        status = write(api, writer, p, n, data);
        at_byte += n;
    }
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
