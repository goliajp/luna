//! Dropping a Vm, as a host that makes many of them does: every block
//! goes back, and the streams it left open write out their buffers first,
//! as PUC's `lua_close` and C's `exit` leave them.

use std::cell::Cell;
use std::ffi::c_void;

use luna_core::runtime::mem::MemOwner;
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

const ALL: [LuaVersion; 5] = [
    LuaVersion::Lua51,
    LuaVersion::Lua52,
    LuaVersion::Lua53,
    LuaVersion::Lua54,
    LuaVersion::Lua55,
];

unsafe extern "C" fn counting(
    ud: *mut c_void,
    p: *mut c_void,
    os: usize,
    ns: usize,
) -> *mut c_void {
    // SAFETY: the test passes a live `Cell<usize>` as `ud`
    let live = unsafe { &*(ud as *const Cell<usize>) };
    let layout = |n| std::alloc::Layout::from_size_align(n, 16).unwrap();
    let old = if p.is_null() { 0 } else { os };
    live.set(live.get() - old);
    if ns == 0 {
        if !p.is_null() {
            // SAFETY: `p` is a live block of `os` bytes this function made
            unsafe { std::alloc::dealloc(p.cast(), layout(os)) };
        }
        return std::ptr::null_mut();
    }
    live.set(live.get() + ns);
    if p.is_null() {
        // SAFETY: the size is not 0
        unsafe { std::alloc::alloc(layout(ns)).cast() }
    } else {
        // SAFETY: `p` is a live block of `os` bytes this function made
        unsafe { std::alloc::realloc(p.cast(), layout(os), ns).cast() }
    }
}

#[test]
fn many_vms_give_back_every_block() {
    for v in ALL {
        let live = Cell::new(0usize);
        for _ in 0..20 {
            {
                // SAFETY: `counting` follows the `lua_Alloc` contract, and
                // the Vm drops before `live`
                let mem = unsafe {
                    MemOwner::raw(counting, &live as *const Cell<usize> as *mut c_void, v)
                };
                let mut vm = Vm::new_with_mem(v, mem);
                vm.eval(
                    "local t = {} for i = 1, 100 do t[i] = {tostring(i), function() return i end} end \
                     local co = coroutine.wrap(function() coroutine.yield(1) end) co() \
                     for w in string.gmatch('a b c', '%a') do t[#t + 1] = w end",
                )
                .unwrap();
            }
            assert_eq!(live.get(), 0, "{v:?}: a block was left after the drop");
        }
    }
}

#[cfg(unix)]
#[test]
fn streams_left_open_write_out_when_the_vm_drops() {
    for v in ALL {
        for nogc in [false, true] {
            let dir =
                std::env::temp_dir().join(format!("luna-drop-{}-{v:?}-{nogc}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let p = |n: &str| dir.join(n).to_str().unwrap().to_string();
            let script = format!(
                "{} local f = io.open({:?}, 'w') f:write('file') \
                 local p = io.popen('cat > {}', 'w') p:write('pipe') \
                 io.output({:?}) io.write('out')",
                if nogc {
                    "getmetatable(io.stdout).__gc = nil"
                } else {
                    ""
                },
                p("file.txt"),
                p("pipe.txt"),
                p("out.txt"),
            );
            {
                let mut vm = Vm::new(v);
                vm.eval(&script).unwrap();
            }
            let read = |n: &str| {
                for _ in 0..200 {
                    if let Ok(s) = std::fs::read_to_string(dir.join(n))
                        && !s.is_empty()
                    {
                        return s;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                String::new()
            };
            assert_eq!(read("file.txt"), "file", "{v:?} {nogc}");
            assert_eq!(read("pipe.txt"), "pipe", "{v:?} {nogc}");
            assert_eq!(read("out.txt"), "out", "{v:?} {nogc}");
            std::fs::remove_dir_all(&dir).ok();
        }
    }
}
