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
use crate::runtime::{FileHandle, Gc, Userdata, UserdataPayload, Value};
use crate::version::LuaVersion;
use crate::vm::argcheck::{self, Args};
use crate::vm::builtins::{arg_error, raise_str};
use crate::vm::error::LuaError;
use crate::vm::exec::Vm;

mod crt;
pub(crate) mod fopen;
mod handle;
mod refs;
use refs::{default_file, set_default_file};
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
use handle::*;
pub(crate) use handle::{flush_all, flush_all_closing};
use lines::*;
pub(crate) use msvc::translate_all;
use numeral::*;
pub(crate) use open::open_file;
#[cfg(any(unix, windows))]
pub(crate) use open::shell_command;
use open::*;
pub(crate) use read::BUFSIZ;
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
mod puc_fns;
pub(crate) use puc_fns::{PUC_FNS, puc_createstdfile};

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

/// What `luaL_loadfile(L, NULL)` reads from the MSVC C library's `stdin`
/// (`getF`: `fread` of `LUAL_BUFFERSIZE` until end of file); `None` when
/// standard input has no `FILE`.
pub(crate) fn read_stdin_chunk(vm: &Vm) -> Option<Vec<u8>> {
    let u = vm.io_stdin()?;
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
        if let Some(u) = self.io_stdin() {
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
