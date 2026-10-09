//! io library: `FILE*` handles over OS files, the default input/output
//! streams, and `io.popen`. Shaped per dialect after liolib.c 5.1–5.5 — the
//! file metatable's layout, what the operations return, how read formats
//! are spelled and how numbers are written all changed between versions.
//!
//! A handle's payload plays the part of C stdio's `FILE`: `write_buf` is its
//! output buffer, `read_buf[read_pos..]` its input buffer (which also holds
//! pushed-back bytes).

use std::io::{Read, Seek, SeekFrom, Write};

use crate::numeric::{self, FloatFmt, Num};
use crate::runtime::value::f2i_exact;
use crate::runtime::{FileHandle, Gc, Table, Userdata, UserdataPayload, Value};
use crate::version::LuaVersion;
use crate::vm::argcheck::{self, Args};
use crate::vm::builtins::{arg_error, raise_str};
use crate::vm::error::LuaError;
use crate::vm::exec::Vm;

mod crt;
pub(crate) mod fopen;
mod handle;
mod lines;
pub(crate) mod msvc;
mod numeral;
mod open;
mod read;
mod results;
mod stream;
#[cfg(windows)]
pub(crate) mod winfs;
mod write;
pub(crate) use handle::flush_all;
use handle::*;
use lines::*;
pub(crate) use msvc::translate_all;
use numeral::*;
pub(crate) use open::open_file;
#[cfg(any(unix, windows))]
pub(crate) use open::shell_command;
use open::*;
use read::*;
pub(crate) use results::c_str;
#[cfg(any(unix, windows))]
pub(crate) use results::exec_result;
pub(crate) use results::file_fail;
pub(crate) use results::note_failure;
pub use results::os_bytes;
pub(crate) use results::os_path;
pub(crate) use results::reset_errno;
pub(crate) use results::strerror;
use results::*;
use stream::*;
use write::*;

/// `EINVAL` / `EBADF` / `ESPIPE`: the errno values stdio reports for the
/// failures luna detects itself rather than receiving from the OS.
const EINVAL: i32 = 22;
const ENOMEM: i32 = 12;
const EBADF: i32 = 9;
#[cfg(not(unix))]
const ESPIPE: i32 = 29;

/// `LUAL_BUFFERSIZE` for `setvbuf`'s default size; luna's buffers do not
/// take a size, but the argument is still checked.
const LUAL_BUFFERSIZE: i64 = 1024;

/// Bytes pulled from the OS per refill of a handle's input buffer.
const READ_CHUNK: usize = 4096;

pub(crate) fn open_io(vm: &mut Vm) {
    let v = vm.version();
    let io = vm.heap.new_table();
    for (name, f) in [
        ("close", io_close as crate::runtime::value::NativeFn),
        ("flush", io_flush),
        ("input", io_input),
        ("lines", io_lines),
        ("open", io_open),
        ("output", io_output),
        ("popen", io_popen),
        ("read", io_read),
        ("tmpfile", io_tmpfile),
        ("type", io_type),
        ("write", io_write),
    ] {
        put_native(vm, io, name, f);
    }
    // The method `close`: 5.1 and 5.2 register `io_close` itself (5.1 with an
    // environment that has no default output, so a missing argument checks
    // nil); 5.3 split it into `f_close`. 5.2's is the very function value of
    // `io.close` (a light C function), so an error names it `io.close`.
    let close: crate::runtime::value::NativeFn = match v {
        LuaVersion::Lua51 => f_close_51,
        LuaVersion::Lua52 => io_close,
        _ => f_close,
    };
    let methods: [(&str, crate::runtime::value::NativeFn); 7] = [
        ("close", close),
        ("flush", f_flush),
        ("lines", f_lines),
        ("read", f_read),
        ("seek", f_seek),
        ("setvbuf", f_setvbuf),
        ("write", f_write),
    ];
    // ≤5.3 keep the methods in the metatable itself (`__index = mt`); 5.4
    // moved them to a table of their own and added `__close`. `__name` comes
    // from `luaL_newmetatable`, which only sets it from 5.3 on.
    let mt = vm.heap.new_table();
    let index = if v >= LuaVersion::Lua54 {
        vm.heap.new_table()
    } else {
        mt
    };
    for (name, f) in methods {
        put_native(vm, index, name, f);
    }
    if v == LuaVersion::Lua52 {
        let io_close = io.get(Value::Str(vm.heap.intern(b"close")));
        put(vm, index, "close", io_close);
    }
    put(vm, mt, "__index", Value::Table(index));
    put_native(vm, mt, "__gc", f_gc);
    put_native(vm, mt, "__tostring", f_tostring);
    if v >= LuaVersion::Lua53 {
        let n = Value::Str(vm.heap.intern(b"FILE*"));
        put(vm, mt, "__name", n);
    }
    if v >= LuaVersion::Lua54 {
        // a to-be-closed file already shut by the user must not re-error,
        // hence the __gc body rather than f_close
        put_native(vm, mt, "__close", f_gc);
    }
    vm.barrier_back_table(index);
    vm.barrier_back_table(mt);
    vm.file_mt = Some(mt);

    for (name, fh) in [
        ("stdin", FileHandle::Stdin),
        ("stdout", FileHandle::Stdout),
        ("stderr", FileHandle::Stderr),
    ] {
        let writable = !matches!(fh, FileHandle::Stdin);
        let stdin = !writable;
        let h = new_file(vm, fh, writable);
        if stdin && crate::stdio::text_mode() {
            use std::io::IsTerminal;
            let tty = std::io::stdin().is_terminal();
            crt::attach(h, msvc::CrtFile::standard(msvc::Std::No, tty));
        }
        put(vm, io, name, Value::Userdata(h));
        match name {
            "stdin" => {
                vm.io_input = Some(h);
                vm.io_stdin = Some(h);
            }
            "stdout" => vm.io_output = Some(h),
            _ => {}
        }
    }
    vm.set_global("io", Value::Table(io))
        .expect("stdlib registration");
    vm.barrier_back_table(io);
}

/// What `luaL_loadfile(L, NULL)` reads from the MSVC C library's `stdin`
/// (`getF`: `fread` of `LUAL_BUFFERSIZE` until end of file); `None` when
/// standard input has no `FILE`.
pub(crate) fn read_stdin_chunk(vm: &Vm) -> Option<Vec<u8>> {
    let u = vm.io_stdin?;
    u.crt.as_ref()?;
    let size = read::lual_buffersize(vm.version());
    let mut src = Vec::new();
    while !crt::with(u, |f, _| f.feof()) {
        src.extend_from_slice(&crt::fread(u, size));
        if crt::ferror(u) {
            break;
        }
    }
    Some(src)
}

impl Vm {
    /// C `fgets(buf, size, stdin)`, for a host that reads lines as lua.c's
    /// REPL does: the bytes up to and including the next newline, at most
    /// `size - 1` of them; `None` at the end of input. The bytes come
    /// through the io library's `io.stdin` buffer, so Lua code reading
    /// stdin in between sees what follows, as it shares C's `stdin`.
    pub fn read_stdin_line(&mut self, size: usize) -> std::io::Result<Option<Vec<u8>>> {
        let mut line = Vec::new();
        if let Some(u) = self.io_stdin {
            while line.len() + 1 < size {
                let Some(b) = getc(u)? else { break };
                line.push(b);
                if b == b'\n' {
                    break;
                }
            }
        } else {
            // no io library: nothing else buffers stdin
            use std::io::BufRead;
            let mut input = std::io::stdin().lock();
            while line.len() + 1 < size && line.last() != Some(&b'\n') {
                let buf = input.fill_buf()?;
                if buf.is_empty() {
                    break;
                }
                let room = &buf[..buf.len().min(size - 1 - line.len())];
                let n = room
                    .iter()
                    .position(|&b| b == b'\n')
                    .map_or(room.len(), |i| i + 1);
                line.extend_from_slice(&room[..n]);
                input.consume(n);
            }
        }
        Ok((!line.is_empty()).then_some(line))
    }
}

fn put(vm: &mut Vm, t: Gc<Table>, k: &str, v: Value) {
    let k = Value::Str(vm.heap.intern(k.as_bytes()));
    // SAFETY: `t` is a table the caller allocated or holds in a local, so it is alive; no reference into it is held across this call, and `set` does not collect
    unsafe { t.as_mut() }
        .set(&mut vm.heap, k, v)
        .expect("valid key");
}

fn put_native(vm: &mut Vm, t: Gc<Table>, k: &str, f: crate::runtime::value::NativeFn) {
    let fv = vm.native(f);
    put(vm, t, k, fv);
}
