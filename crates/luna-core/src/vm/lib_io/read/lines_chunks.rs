//! Reading lines and whole files, in the chunk sizes each dialect uses.

use super::*;
use crate::runtime::mem::LVec;

/// The C library's `BUFSIZ`: the size of the chunks ≤5.2 read a line in
/// (`LUAL_BUFFERSIZE`). glibc's is 8192; the BSDs', and musl's (which
/// emscripten builds with), 1024; Microsoft's 512.
#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "dragonfly",
    target_env = "musl",
    target_family = "wasm"
))]
pub(crate) const BUFSIZ: usize = 1024;
#[cfg(windows)]
pub(crate) const BUFSIZ: usize = 512;
#[cfg(not(any(
    windows,
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "dragonfly",
    target_env = "musl",
    target_family = "wasm"
)))]
pub(crate) const BUFSIZ: usize = 8192;

/// `read_line`: up to and excluding (`keep_nl`: including) the newline; nil
/// when nothing at all was read.
pub(crate) fn read_line(vm: &mut Vm, u: Gc<Userdata>, keep_nl: bool) -> Rd {
    if vm.version() <= LuaVersion::Lua52 {
        return read_line_fgets(vm, u, keep_nl);
    }
    let mut buf = LVec::new(vm.heap.mem());
    let mut got_nl = false;
    while let Some(c) = getc(u)? {
        if c == b'\n' {
            got_nl = true;
            if keep_nl {
                buf.push(c)?;
            }
            break;
        }
        buf.push(c)?;
    }
    // read a character at a time into chunks of the buffer's size
    let nl = keep_nl && got_nl;
    let len = buf.len() - usize::from(nl);
    let chunks = even_chunks(chunk_size(vm), len);
    let s = buffered(vm, &buf, &chunks, nl)?;
    if got_nl || len > 0 {
        Ok(s)
    } else {
        Ok(Value::Nil)
    }
}

/// ≤5.2's `read_line` reads with `fgets` and measures each chunk with
/// `strlen`: a NUL cuts the chunk short there, and a newline after it is
/// lost, so the line runs on into the next.
pub(crate) fn read_line_fgets(vm: &mut Vm, u: Gc<Userdata>, keep_nl: bool) -> Rd {
    let mut out = LVec::new(vm.heap.mem());
    let mut chunks = Vec::new();
    let room = chunk_size(vm);
    loop {
        let mut chunk = Vec::new();
        while chunk.len() < BUFSIZ - 1 {
            match getc(u)? {
                Some(c) => {
                    chunk.push(c);
                    if c == b'\n' {
                        break;
                    }
                }
                None => break,
            }
        }
        if chunk.is_empty() {
            // `fgets` found nothing more, after the room was made
            chunks.push((room, 0));
            let s = buffered(vm, &out, &chunks, false)?;
            return Ok(if out.is_empty() { Value::Nil } else { s });
        }
        // strlen: up to the first NUL, the whole chunk when there is none
        let len = chunk.iter().position(|&b| b == 0).unwrap_or(chunk.len());
        if len == 0 || chunk[len - 1] != b'\n' {
            out.extend_from_slice(&chunk[..len])?;
            chunks.push((room, len));
        } else {
            let end = if keep_nl { len } else { len - 1 };
            out.extend_from_slice(&chunk[..end])?;
            chunks.push((room, end));
            return buffered(vm, &out, &chunks, false);
        }
    }
}

/// `read_all`: never fails (an empty string at end of file).
pub(crate) fn read_all(vm: &mut Vm, u: Gc<Userdata>) -> Rd {
    if u.crt.is_some() {
        let buf = read_all_crt(vm.version(), u);
        let chunks = all_chunks(vm, buf.len());
        return buffered(vm, &buf, &chunks, false);
    }
    let mut buf = LVec::new(vm.heap.mem());
    loop {
        buf.extend_from_slice(&u.read_buf[u.read_pos..])?;
        // SAFETY: `u` is a file handle the caller holds; the right-hand side is read before the borrow starts, and the borrow covers one field store
        unsafe { u.as_mut() }.read_pos = u.read_buf.len();
        if !fill(u)? {
            break;
        }
    }
    let chunks = all_chunks(vm, buf.len());
    buffered(vm, &buf, &chunks, false)
}

/// `LUAL_BUFFERSIZE` of PUC built for 64-bit Windows.
pub(crate) fn lual_buffersize(v: LuaVersion) -> usize {
    match v {
        LuaVersion::Lua51 | LuaVersion::Lua52 => 512,
        LuaVersion::Lua53 => 8192,
        _ => 1024,
    }
}

/// Each dialect's `read_all` over the C library's `FILE`, whose `fread`
/// calls decide what stays in the stream buffer (and so what `seek`
/// reports).
pub(crate) fn read_all_crt(v: LuaVersion, u: Gc<Userdata>) -> Vec<u8> {
    if v == LuaVersion::Lua51 {
        return read_chars_51(u, usize::MAX);
    }
    let mut rlen = lual_buffersize(v);
    let mut out = Vec::new();
    loop {
        let got = crt::fread(u, rlen);
        let short = got.len() < rlen;
        out.extend_from_slice(&got);
        if short {
            return out;
        }
        // 5.2 doubles its buffer on every round
        if v == LuaVersion::Lua52 {
            rlen *= 2;
        }
    }
}

/// 5.1's `read_chars`: `fread` in `LUAL_BUFFERSIZE` chunks.
pub(crate) fn read_chars_51(u: Gc<Userdata>, mut n: usize) -> Vec<u8> {
    let mut rlen = lual_buffersize(LuaVersion::Lua51);
    let mut out = Vec::new();
    loop {
        rlen = rlen.min(n);
        let got = crt::fread(u, rlen);
        n -= got.len();
        let full = got.len() == rlen;
        out.extend_from_slice(&got);
        if n == 0 || !full {
            return out;
        }
    }
}
