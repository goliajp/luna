//! Reading lines and whole files, in the chunk sizes each dialect uses.

use super::*;

/// `BUFSIZ`, the size of the chunks ≤5.2 read a line in (`LUAL_BUFFERSIZE`).
#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "dragonfly"
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
    target_os = "dragonfly"
)))]
pub(crate) const BUFSIZ: usize = 8192;

/// `read_line`: up to and excluding (`keep_nl`: including) the newline; nil
/// when nothing at all was read.
pub(crate) fn read_line(vm: &mut Vm, u: Gc<Userdata>, keep_nl: bool) -> std::io::Result<Value> {
    if vm.version() <= LuaVersion::Lua52 {
        return read_line_fgets(vm, u, keep_nl);
    }
    let mut buf = Vec::new();
    let mut got_nl = false;
    while let Some(c) = getc(u)? {
        if c == b'\n' {
            got_nl = true;
            if keep_nl {
                buf.push(c);
            }
            break;
        }
        buf.push(c);
    }
    if got_nl || !buf.is_empty() {
        read_str(vm, &buf)
    } else {
        Ok(Value::Nil)
    }
}

/// ≤5.2's `read_line` reads with `fgets` and measures each chunk with
/// `strlen`: a NUL cuts the chunk short there, and a newline after it is
/// lost, so the line runs on into the next.
pub(crate) fn read_line_fgets(
    vm: &mut Vm,
    u: Gc<Userdata>,
    keep_nl: bool,
) -> std::io::Result<Value> {
    let mut out = Vec::new();
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
            return if out.is_empty() {
                Ok(Value::Nil)
            } else {
                read_str(vm, &out)
            };
        }
        // strlen: up to the first NUL, the whole chunk when there is none
        let len = chunk.iter().position(|&b| b == 0).unwrap_or(chunk.len());
        if len == 0 || chunk[len - 1] != b'\n' {
            out.extend_from_slice(&chunk[..len]);
        } else {
            let end = if keep_nl { len } else { len - 1 };
            out.extend_from_slice(&chunk[..end]);
            return read_str(vm, &out);
        }
    }
}

/// A string read from a file. One longer than a string can hold is an
/// allocation failure, which PUC's buffer would meet first.
pub(crate) fn read_str(vm: &mut Vm, bytes: &[u8]) -> std::io::Result<Value> {
    if bytes.len() > crate::runtime::string::MAX_LEN {
        return Err(posix_error(ENOMEM));
    }
    Ok(Value::Str(vm.heap.intern(bytes)))
}

/// `read_all`: never fails (an empty string at end of file).
pub(crate) fn read_all(vm: &mut Vm, u: Gc<Userdata>) -> std::io::Result<Value> {
    if u.crt.is_some() {
        let buf = read_all_crt(vm.version(), u);
        return read_str(vm, &buf);
    }
    let mut buf = Vec::new();
    loop {
        buf.extend_from_slice(&u.read_buf[u.read_pos..]);
        // SAFETY: `u` is a file handle the caller holds; the right-hand side is read before the borrow starts, and the borrow covers one field store
        unsafe { u.as_mut() }.read_pos = u.read_buf.len();
        if !fill(u)? {
            break;
        }
    }
    read_str(vm, &buf)
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
