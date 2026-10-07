//! Hooks: C hook functions per thread (`lua_sethook`), their events, and
//! how they share a thread's one hook with `debug.sethook`.
//!
//! A thread's hook is luna's `HookState`: a Lua hook keeps its function
//! there, a C hook its `lua_Hook` as a light userdata, which the VM hands
//! to [`run_c_hook`]. As in PUC, `lua_gethook` reports a Lua hook as the
//! debug library's C hook (`luna_c_hookf`), and setting that one back
//! restores the Lua hook, which a C hook set in between keeps in the
//! registry's hook table.

use super::ccall::{LuaHook, take_error, with_c};
use super::debug::{ArBuf, DebugPtr};
use super::*;
use luna_core::vm::exec::HookState;

const MASKCALL: c_int = 1;
const MASKRET: c_int = 2;
const MASKLINE: c_int = 4;
const MASKCOUNT: c_int = 8;

/// The C hook running on a thread.
pub(super) struct HookRun {
    /// the level reference of the function it interrupted
    pub(super) level: usize,
    /// C calls running on the thread when it started: a C function it
    /// calls runs above them
    pub(super) depth: usize,
    /// it interrupted a Lua function, so it may yield
    pub(super) lua: bool,
    /// it called `lua_yield`
    pub(super) yielded: bool,
}

// SAFETY: the declarations match the definitions in `csrc/shim_core.c` and
// `csrc/shim_debug.c`. `luna_c_protect_hook` runs a hook under a fresh error
// boundary and returns normally whatever it does; `luna_c_hookf` is only
// handed out as a hook. C sees `lua_State` as opaque and reads only its
// leading fields, which `csrc/shim.h` declares
#[allow(improper_ctypes)]
unsafe extern "C" {
    fn luna_c_protect_hook(L: *mut LuaState, h: LuaHook, ar: *mut c_void) -> c_int;
    fn luna_c_hookf(L: *mut LuaState, ar: *mut c_void);
}

/// The debug library's hook (PUC `hookf`), as `lua_gethook` reports a Lua
/// hook: the value first handed out, which `lua_sethook` knows again by
/// that value, never by taking the function's address anew.
fn hookf(api: &mut Api) -> LuaHook {
    type Raw = unsafe extern "C" fn(*mut LuaState, *mut c_void);
    // SAFETY: `luna_c_hookf` is a C function of the `lua_Hook` shape; it is
    // only handed out
    *(api.g().hookf).get_or_insert(unsafe { std::mem::transmute::<Raw, LuaHook>(luna_c_hookf) })
}

/// PUC's event codes: `LUA_HOOKTAILCALL` (5.2+) and 5.1's
/// `LUA_HOOKTAILRET` are both 4.
fn event_code(event: &[u8]) -> c_int {
    match event {
        b"call" => 0,
        b"return" => 1,
        b"line" => 2,
        b"count" => 3,
        _ => 4,
    }
}

/// Run the C hook `cf` of the running thread for `event` (PUC
/// `luaD_hook`): the hook gets a `lua_Debug` with the event, the line of a
/// line event and the level it interrupted, and a frame of its own on the
/// thread's C stack. An error it raises goes on from the interrupted
/// function; a `lua_yield` in a line or count hook suspends the coroutine.
fn run_c_hook(vm: &mut Vm, cf: *const (), event: &[u8], line: Option<i64>) -> Result<(), LuaError> {
    // SAFETY: the VM hands back the light userdata `lua_sethook` stored,
    // which came from a `lua_Hook`
    let h: LuaHook = unsafe { std::mem::transmute::<*const (), LuaHook>(cf) };
    let co = vm.host_thread();
    let l = state_of(vm, co);
    let code = event_code(event);
    let mut buf = ArBuf::zeroed();
    // SAFETY: `buf` has room for a `lua_Debug` of any dialect
    let ar = unsafe { DebugPtr::new(vm.version(), buf.as_mut_ptr()) };
    ar.set_event(code);
    let line = if code == 2 { line.unwrap_or(-1) } else { -1 };
    ar.set_currentline(line as c_int);
    let level = vm.host_level_count(co);
    // 5.1 tells nothing of the function a tail return leaves (`i_ci` 0)
    let raw = if event == b"tail return" {
        0
    } else {
        let mut api = Api { vm: &mut *vm, l };
        super::debug::encode_ref(&mut api, l, level)
    };
    ar.set_level_ref(raw);
    let lua = matches!(
        vm.host_level(co, 0),
        Some(luna_core::vm::exec::host_c::HostLevel::Lua)
    );
    let top = co.host_stack.len();
    // SAFETY: `l` is the running thread's live state; the borrow ends here
    let s = unsafe { &mut *l };
    let outer_base = std::mem::replace(&mut s.base, top);
    let outer = s.hook.running.replace(HookRun {
        level,
        depth: s.calls.len(),
        lua,
        yielded: false,
    });
    // SAFETY: `l` is a live thread and `h` the hook the host gave
    // `lua_sethook`; the boundary returns whatever the hook does
    let status = with_c(vm, l, || unsafe {
        luna_c_protect_hook(l, h, ar_ptr(&mut buf))
    });
    let r = if status == LUA_OK {
        Ok(())
    } else {
        Err(LuaError(take_error(l)))
    };
    // SAFETY: as above
    let s = unsafe { &mut *l };
    let run = std::mem::replace(&mut s.hook.running, outer);
    s.base = outer_base;
    // SAFETY: the thread is live while its state is; nothing else borrows
    // its C stack now
    unsafe { &mut (*co.as_ptr()).host_stack }.truncate(top);
    if r.is_ok() && run.is_some_and(|h| h.yielded) && (code == 2 || code == 3) {
        vm.host_hook_yield();
    }
    r
}

fn ar_ptr(buf: &mut ArBuf) -> *mut c_void {
    buf.as_mut_ptr()
}

/// `lua_yield` inside a C hook that interrupted a Lua function (PUC
/// `lua_yieldk`'s `isLua(ci)` case): the hook goes on and the coroutine
/// suspends once it returns. `false` when no such hook is running, or a
/// C function it called is the one yielding.
pub(super) fn hook_yield(api: &mut Api) -> bool {
    let s = api.st();
    let depth = s.calls.len();
    match s.hook.running.as_mut() {
        Some(h) if h.depth == depth && h.lua => {
            h.yielded = true;
            true
        }
        _ => false,
    }
}

/// Whether the running C code is a hook that interrupted a Lua function.
pub(super) fn in_lua_hook(api: &mut Api) -> bool {
    let s = api.st();
    let depth = s.calls.len();
    s.hook
        .running
        .as_ref()
        .is_some_and(|h| h.depth == depth && h.lua)
}

/// Whether the C function calling into Lua now is a C hook, whose callee
/// PUC names "hook" (5.3+).
pub(super) fn calling_from_hook(api: &mut Api) -> bool {
    let s = api.st();
    let depth = s.calls.len();
    s.hook.running.as_ref().is_some_and(|h| h.depth == depth)
}

/// The registry's table of the Lua hooks C hooks displaced, by thread
/// (PUC's `HOOKKEY` table).
fn hook_table(vm: &mut Vm) -> Gc<luna_core::runtime::Table> {
    let reg = vm.host_registry();
    let key = Value::Str(vm.heap.intern(b"_HOOKKEY"));
    if let Value::Table(t) = reg.get(key) {
        return t;
    }
    let t = vm.heap.new_table();
    // SAFETY: `reg` is the registry, a root; the borrow covers one store of
    // a string key, which does not collect
    let r = unsafe { reg.as_mut() }.set(&mut vm.heap, key, Value::Table(t));
    debug_assert!(r.is_ok(), "a string key");
    vm.heap.barrier_back(reg);
    t
}

fn is_lua_hook(f: Option<Value>) -> bool {
    matches!(f, Some(Value::Closure(_) | Value::Native(_)))
}

/// PUC `lua_sethook`: `func` becomes the thread's hook for the events in
/// `mask`, the count event every `count` instructions; NULL or an empty
/// mask turns hooks off. 5.1 to 5.3 return 1.
///
/// # Safety
/// `L` is a live thread of an open state, and no other API call on it is
/// running other than a C function it is calling into.
// SAFETY: no other item in the link is named `lua_sethook`: the host does
// not link PUC's liblua next to this crate, which defines each `lua_*`
// symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_sethook(
    L: *mut LuaState,
    func: Option<LuaHook>,
    mask: c_int,
    count: c_int,
) -> c_int {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    let (func, mask) = match func {
        Some(f) if mask != 0 => (Some(f), mask),
        _ => (None, 0),
    };
    api.vm.set_host_hook(run_c_hook);
    let co = api.thread();
    let old = api.vm.host_hook_state(co);
    let thread = Value::Coro(co);
    let hook = match func {
        // a Lua hook stays recorded (PUC's hook table keeps it)
        None => old.func.filter(|_| is_lua_hook(old.func)),
        Some(f) if api.g().hookf.is_some_and(|h| h as usize == f as usize) => {
            if is_lua_hook(old.func) {
                old.func
            } else {
                let t = hook_table(api.vm);
                match t.get(thread) {
                    v @ (Value::Closure(_) | Value::Native(_)) => Some(v),
                    _ => Some(Value::LightUserdata(f as *const ())),
                }
            }
        }
        Some(f) => {
            if let Some(lua) = old.func.filter(|_| is_lua_hook(old.func)) {
                let t = hook_table(api.vm);
                // SAFETY: the hook table is reachable from the registry, a
                // root; the borrow covers one store, which does not collect
                let r = unsafe { t.as_mut() }.set(&mut api.vm.heap, thread, lua);
                debug_assert!(r.is_ok(), "a thread key");
                api.vm.heap.barrier_back(t);
            }
            Some(Value::LightUserdata(f as *const ()))
        }
    };
    let state = HookState {
        func: hook,
        rust_func: old.rust_func,
        call: mask & MASKCALL != 0,
        ret: mask & MASKRET != 0,
        line: mask & MASKLINE != 0,
        count: mask & MASKCOUNT != 0 && count > 0,
        count_base: i64::from(count),
        count_left: i64::from(count),
    };
    api.vm.host_set_hook_state(co, state);
    let s = api.st();
    s.hook.func = func;
    s.hook.mask = mask;
    s.hook.count = count;
    1
}

/// The thread's hook if it is a C hook `lua_sethook` installed on it.
fn own_c_hook(api: &mut Api, state: &HookState) -> Option<LuaHook> {
    let Some(Value::LightUserdata(p)) = state.func else {
        return None;
    };
    api.st()
        .hook
        .func
        .filter(|&f| std::ptr::eq(f as *const (), p))
}

fn armed(state: &HookState) -> bool {
    state.call || state.ret || state.line || state.count
}

/// PUC `lua_gethook`: the thread's hook, or NULL.
///
/// # Safety
/// As [`lua_sethook`].
// SAFETY: no other item in the link is named `lua_gethook`: the host does
// not link PUC's liblua next to this crate, which defines each `lua_*`
// symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_gethook(L: *mut LuaState) -> Option<LuaHook> {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    let state = api.vm.host_hook_state(api.thread());
    if !armed(&state) {
        return own_c_hook(&mut api, &state);
    }
    match state.func {
        Some(Value::LightUserdata(p)) => {
            // SAFETY: a C hook's light userdata came from a `lua_Hook`
            Some(unsafe { std::mem::transmute::<*const (), LuaHook>(p) })
        }
        Some(_) => Some(hookf(&mut api)),
        None => None,
    }
}

/// PUC `lua_gethookmask`.
///
/// # Safety
/// As [`lua_sethook`].
// SAFETY: no other item in the link is named `lua_gethookmask`: the host
// does not link PUC's liblua next to this crate, which defines each `lua_*`
// symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_gethookmask(L: *mut LuaState) -> c_int {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    let state = api.vm.host_hook_state(api.thread());
    if own_c_hook(&mut api, &state).is_some() {
        return api.st().hook.mask;
    }
    if state.func.is_none() {
        return 0;
    }
    let bit = |on: bool, m: c_int| if on { m } else { 0 };
    bit(state.call, MASKCALL)
        | bit(state.ret, MASKRET)
        | bit(state.line, MASKLINE)
        | bit(state.count, MASKCOUNT)
}

/// PUC `lua_gethookcount`.
///
/// # Safety
/// As [`lua_sethook`].
// SAFETY: no other item in the link is named `lua_gethookcount`: the host
// does not link PUC's liblua next to this crate, which defines each `lua_*`
// symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_gethookcount(L: *mut LuaState) -> c_int {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    let state = api.vm.host_hook_state(api.thread());
    if own_c_hook(&mut api, &state).is_some() {
        return api.st().hook.count;
    }
    state.count_base as c_int
}

/// The Rust side of `luna_c_hookf`, the debug library's hook (PUC
/// `hookf`): call the thread's Lua hook with the event's name and line.
///
/// # Safety
/// `L` is a live thread of an open state, the innermost API call on it;
/// `ar` is the `lua_Debug` a hook was given. Called by `luna_c_hookf`,
/// which throws what this raises.
// SAFETY: no other item in the link is named `luna_capi_hookf`; the C
// function `luna_c_hookf` is its only caller
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_capi_hookf(L: *mut LuaState, ar: *mut c_void) {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    let v = api.version();
    // SAFETY: `ar` is a `lua_Debug` of the state's dialect (# Safety)
    let d = unsafe { DebugPtr::new(v, ar) };
    let co = api.thread();
    let state = api.vm.host_hook_state(co);
    let f = match state.func {
        f @ Some(Value::Closure(_) | Value::Native(_)) => f,
        _ => match hook_table(api.vm).get(Value::Coro(co)) {
            f @ (Value::Closure(_) | Value::Native(_)) => Some(f),
            _ => None,
        },
    };
    let Some(f) = f else { return };
    let names: [&[u8]; 5] = [
        b"call",
        b"return",
        b"line",
        b"count",
        if v == LuaVersion::Lua51 {
            b"tail return"
        } else {
            b"tail call"
        },
    ];
    let event = usize::try_from(d.event()).ok().and_then(|e| names.get(e));
    let Some(event) = event else { return };
    let name = api.str(event);
    let line = d.currentline();
    let line = if line >= 0 {
        Value::Int(i64::from(line))
    } else {
        Value::Nil
    };
    let hooked = calling_from_hook(&mut api);
    api.vm.host_mark_hook_call(hooked);
    let r = api.vm.host_call(f, &[name, line], None);
    api.vm.host_mark_hook_call(false);
    if let Err(e) = r {
        api.raise(e);
    }
}
