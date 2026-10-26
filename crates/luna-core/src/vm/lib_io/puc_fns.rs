//! The io library's C functions as PUC's opening names them.

use super::*;
use crate::runtime::Builtin;
use crate::vm::opening::{Ctor, Natives, Opener, run_open};

use Ctor::{C, F};

pub(crate) const PUC_FNS: Natives = &[
    ("f_close", F(f_close, Builtin::None)),
    ("f_flush", F(f_flush, Builtin::None)),
    ("f_gc", F(f_gc, Builtin::None)),
    ("f_lines", F(f_lines, Builtin::None)),
    ("f_read", F(f_read, Builtin::None)),
    ("f_seek", F(f_seek, Builtin::None)),
    ("f_setvbuf", F(f_setvbuf, Builtin::None)),
    ("f_tostring", F(f_tostring, Builtin::None)),
    ("f_write", F(f_write, Builtin::None)),
    ("io_close", C(io_close_fn)),
    ("io_fclose", C(fclose_51)),
    ("io_flush", F(io_flush, Builtin::None)),
    ("io_gc", F(f_gc, Builtin::None)),
    ("io_input", F(io_input, Builtin::None)),
    ("io_lines", F(io_lines, Builtin::None)),
    ("io_noclose", C(noclose_51)),
    ("io_open", F(io_open, Builtin::None)),
    ("io_output", F(io_output, Builtin::None)),
    ("io_pclose", C(pclose_51)),
    ("io_popen", F(io_popen, Builtin::None)),
    ("io_read", F(io_read, Builtin::None)),
    ("io_tmpfile", F(io_tmpfile, Builtin::None)),
    ("io_tostring", F(f_tostring, Builtin::None)),
    ("io_type", F(io_type, Builtin::None)),
    ("io_write", F(io_write, Builtin::None)),
    ("luaopen_io", Ctor::Open(luaopen_io)),
];

fn luaopen_io(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let n = run_open(vm, fs, nargs, "luaopen_io", 1)?;
    vm.heap.set_phantoms(false);
    if vm.version() == LuaVersion::Lua51
        && let Value::Table(io) = vm.stack[fs as usize]
    {
        // where 5.1's environment records are on PUC's list of objects:
        // after the library's first function, before its last
        let h = |vm: &Vm, k: &[u8]| match vm.heap.find_interned(k).map(|k| io.get(Value::Str(k))) {
            Some(Value::Native(n)) => n.as_ptr() as *mut _,
            _ => std::ptr::null_mut(),
        };
        let (first, last) = (h(vm, b"close"), h(vm, b"write"));
        vm.heap.env51_anchors(first, last);
    }
    Ok(n)
}

/// 5.1's closures of the io library's environments, which luna has no
/// objects for (see `vm/opening/fenv51.rs`).
fn fclose_51(vm: &mut Vm, op: &mut Opener, _id: u32, _nup: u32) -> Result<Value, LuaError> {
    Ok(op.phantom_closure(vm, 0))
}

fn noclose_51(vm: &mut Vm, op: &mut Opener, _id: u32, _nup: u32) -> Result<Value, LuaError> {
    Ok(op.phantom_closure(vm, 1))
}

fn pclose_51(vm: &mut Vm, op: &mut Opener, _id: u32, _nup: u32) -> Result<Value, LuaError> {
    Ok(op.phantom_closure(vm, 2))
}

/// PUC's `createstdfile`: the standard file `fname` made, its handle
/// stored in the io table (below it) and, as the default input or output
/// (`k`), in 5.1's environment array (an index) or the registry (a
/// name).
pub(crate) fn puc_createstdfile(
    vm: &mut Vm,
    op: &mut Opener,
    k: Result<Option<&[u8]>, i64>,
    fname: &[u8],
) -> Result<(), LuaError> {
    let fh = match fname {
        b"stdin" => FileHandle::Stdin,
        b"stdout" => FileHandle::Stdout,
        _ => FileHandle::Stderr,
    };
    let stdin = matches!(fh, FileHandle::Stdin);
    let h = handle::new_file(vm, fh, !stdin, |vm, u| {
        op.push(vm, Value::Userdata(u))?;
        op.after(vm)
    })?;
    if stdin {
        vm.heap.set_weak_userdata(h);
        if crate::stdio::text_mode() {
            use std::io::IsTerminal;
            let tty = std::io::stdin().is_terminal();
            crt::attach(h, msvc::CrtFile::standard(msvc::Std::No, tty));
        }
    }
    match k {
        Err(k) => {
            if k > 0 {
                op.pushvalue(vm, -1)?;
                op.rawseti(vm, crate::vm::opening::Ix::Env, k)?;
            }
            op.pushvalue(vm, -2)?;
            let ix = op.ix(vm, -2);
            op.setfenv(vm, ix)?;
            let ix = op.ix(vm, -3);
            op.setfield(vm, ix, fname)
        }
        Ok(k) => {
            if let Some(k) = k {
                op.pushvalue(vm, -1)?;
                op.setfield(vm, crate::vm::opening::Ix::Registry, k)?;
            }
            let ix = op.ix(vm, -2);
            op.setfield(vm, ix, fname)
        }
    }
}

/// `io.close`, and 5.1's file method `close`, which luna keeps apart (its
/// environment has no default output, so a missing argument checks nil).
fn io_close_fn(vm: &mut Vm, op: &mut Opener, id: u32, nup: u32) -> Result<Value, LuaError> {
    let f = if vm.version() == LuaVersion::Lua51 && op.registering_methods() {
        f_close_51
    } else {
        io_close
    };
    op.func(vm, id, f, Builtin::None, &[], nup)
}
