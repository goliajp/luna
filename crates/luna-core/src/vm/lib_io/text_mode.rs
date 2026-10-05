//! The MSVC C library's text mode, which PUC built with MSVC reads and
//! writes files through (see [`Vm::set_crt_text_mode`]).
//!
//! The stream buffer (`read_buf`) holds translated bytes and is refilled as
//! that library's `_filbuf` and `_read` refill it, so that `seek` reports
//! the position its `ftell` computes from the buffer. For a file whose lines
//! end in a bare `\n` that position is not the true one, and can be
//! negative, which makes `seek` fail; PUC on Windows behaves the same.

use super::*;
use crate::runtime::TextState;

/// `_INTERNAL_BUFSIZ`, the size of a stream buffer.
const INTERNAL_BUFSIZ: usize = 4096;
/// `_SMALL_BUFSIZ`, what the first refill after a seek on a read-only
/// stream asks for.
const SMALL_BUFSIZ: usize = 512;
const CR: u8 = b'\r';
const LF: u8 = b'\n';
const CTRL_Z: u8 = 0x1a;

fn state(u: Gc<Userdata>) -> TextState {
    u.text.expect("a text mode stream")
}

fn set_state(u: Gc<Userdata>, st: TextState) {
    // SAFETY: `u` is a file handle the caller holds; the borrow covers one field store
    unsafe { u.as_mut() }.text = Some(st);
}

/// Mark a newly made file handle as being in text mode.
pub(super) fn set_text(u: Gc<Userdata>) {
    set_state(u, TextState::default());
}

/// `\n` as the library writes it in text mode, `\r\n`.
pub(crate) fn to_crlf(bytes: &[u8]) -> std::borrow::Cow<'_, [u8]> {
    if !bytes.contains(&LF) {
        return std::borrow::Cow::Borrowed(bytes);
    }
    let mut out = Vec::with_capacity(bytes.len() + bytes.len() / 8);
    for &b in bytes {
        if b == LF {
            out.push(CR);
        }
        out.push(b);
    }
    std::borrow::Cow::Owned(out)
}

/// A whole source file as reading it in text mode gives it: `\r\n` becomes
/// `\n` and a Ctrl+Z ends it.
pub(crate) fn translate_all(raw: &[u8]) -> Vec<u8> {
    let end = raw.iter().position(|&b| b == CTRL_Z).unwrap_or(raw.len());
    let raw = &raw[..end];
    let mut out = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == CR && raw.get(i + 1) == Some(&LF) {
            i += 1;
        }
        out.push(raw[i]);
        i += 1;
    }
    out
}

/// Whether the stream's handle cannot seek (a pipe, or standard input,
/// which luna treats as one): a byte read past a final `\r` is kept for the
/// next read instead of being given back by seeking.
fn is_pipe(u: Gc<Userdata>) -> bool {
    u.popen_child.is_some() || matches!(u.file(), FileHandle::Stdin)
}

/// One `ReadFile` of up to `buf.len()` bytes.
fn os_read(u: Gc<Userdata>, buf: &mut [u8]) -> std::io::Result<usize> {
    // SAFETY: `u` is held by the caller; the borrow lives for the one read, which runs no Lua code
    match unsafe { u.as_mut() }.file_mut() {
        FileHandle::File(f) => f.read(buf),
        FileHandle::Stdin => {
            crate::stdio::before_stdin_read();
            std::io::stdin().read(buf)
        }
        FileHandle::Stdout | FileHandle::Stderr => Err(posix_error(EBADF)),
        FileHandle::Closed => unreachable!("reads check the stream is open"),
    }
}

fn os_seek(u: Gc<Userdata>, from: SeekFrom) -> std::io::Result<u64> {
    // SAFETY: `u` is held by the caller; the borrow lives for the one seek
    match unsafe { u.as_mut() }.file_mut() {
        FileHandle::File(f) => f.seek(from),
        _ => Err(posix_error(ESPIPE)),
    }
}

/// The library's `_read` on a text mode handle: one read of up to `count`
/// bytes from the OS, `\r\n` turned into `\n`, stopping at a Ctrl+Z (after
/// which every read gives nothing until a seek).
fn lowio_read(u: Gc<Userdata>, count: usize) -> std::io::Result<Vec<u8>> {
    let mut st = state(u);
    if count == 0 || st.eof_flag {
        return Ok(Vec::new());
    }
    let mut raw = vec![0u8; count];
    let mut have = 0;
    if let Some(b) = st.lookahead.take() {
        raw[0] = b;
        have = 1;
    }
    have += os_read(u, &mut raw[have..])?;
    raw.truncate(have);
    if raw.is_empty() {
        set_state(u, st);
        return Ok(raw);
    }
    st.crlf = raw[0] == LF;
    let pipe = is_pipe(u);
    let mut out = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        let c = raw[i];
        if c == CTRL_Z {
            st.eof_flag = true;
            break;
        }
        if c != CR {
            out.push(c);
            i += 1;
            continue;
        }
        if i + 1 < raw.len() {
            if raw[i + 1] == LF {
                out.push(LF);
                i += 2;
            } else {
                out.push(CR);
                i += 1;
            }
            continue;
        }
        // a `\r` that ends the read: look at the next byte
        i += 1;
        let mut peek = [0u8; 1];
        if os_read(u, &mut peek)? == 0 {
            out.push(CR);
            continue;
        }
        if pipe {
            if peek[0] == LF {
                out.push(LF);
            } else {
                out.push(CR);
                st.lookahead = Some(peek[0]);
            }
        } else if peek[0] == LF && out.is_empty() {
            out.push(LF);
        } else {
            // the `\n` (or whatever follows) is left for the next read
            os_seek(u, SeekFrom::Current(-1))?;
            if peek[0] != LF {
                out.push(CR);
            }
        }
    }
    set_state(u, st);
    Ok(out)
}

/// `_filbuf`: refill the buffer; `false` at end of file.
pub(super) fn refill(u: Gc<Userdata>) -> std::io::Result<bool> {
    let mut st = state(u);
    st.has_buffer = true;
    let size = if st.small {
        SMALL_BUFSIZ
    } else {
        INTERNAL_BUFSIZ
    };
    set_state(u, st);
    let data = lowio_read(u, size)?;
    // SAFETY: `u` is held by the caller; `lowio_read`'s borrows have ended, and `m` is the only reference into it until return
    let m = unsafe { u.as_mut() };
    m.read_pos = 0;
    if data.is_empty() {
        m.read_buf = data;
        return Ok(false);
    }
    m.read_buf = data;
    let mut st = state(u);
    if !u.writable && st.eof_flag {
        st.ctrl_z = true;
    }
    st.small = false;
    set_state(u, st);
    Ok(true)
}

/// `fread` of `n` bytes; fewer at end of file.
pub(super) fn fread(u: Gc<Userdata>, n: usize) -> std::io::Result<Vec<u8>> {
    let st = state(u);
    let bufsiz = if st.has_buffer && st.small {
        SMALL_BUFSIZ
    } else {
        INTERNAL_BUFSIZ
    };
    let mut out = Vec::new();
    while out.len() < n {
        let remaining = n - out.len();
        let cnt = u.read_buf.len() - u.read_pos;
        if cnt > 0 {
            let take = remaining.min(cnt);
            out.extend_from_slice(&u.read_buf[u.read_pos..u.read_pos + take]);
            // SAFETY: `u` is held by the caller; the borrow covers one field store
            unsafe { u.as_mut() }.read_pos += take;
        } else if remaining >= bufsiz {
            // straight from the handle, past the buffer, which is emptied
            // SAFETY: `u` is held by the caller; the borrow ends before `lowio_read` takes its own
            let m = unsafe { u.as_mut() };
            m.read_buf = Vec::new();
            m.read_pos = 0;
            let got = lowio_read(u, remaining - remaining % bufsiz)?;
            if got.is_empty() {
                break;
            }
            out.extend_from_slice(&got);
        } else {
            if !refill(u)? {
                break;
            }
            out.push(u.read_buf[0]);
            // SAFETY: `u` is held by the caller; the borrow covers one field store
            unsafe { u.as_mut() }.read_pos = 1;
        }
    }
    Ok(out)
}

fn count_lf(bytes: &[u8]) -> i64 {
    bytes.iter().filter(|&&b| b == LF).count() as i64
}

/// `ftell` of a text mode stream whose output is written out.
pub(super) fn ftell(u: Gc<Userdata>) -> std::io::Result<i64> {
    let st = state(u);
    let lowio = os_seek(u, SeekFrom::Current(0))? as i64;
    let total = u.read_buf.len();
    let pos = u.read_pos;
    let cnt = (total - pos) as i64;
    if !st.has_buffer {
        return Ok(lowio - cnt);
    }
    let offset = pos as i64 + count_lf(&u.read_buf[..pos]);
    if lowio == 0 {
        return Ok(offset);
    }
    if total == 0 {
        // not reading: what is buffered for output is already written
        return Ok(lowio + offset);
    }
    if cnt == 0 {
        return Ok(lowio);
    }
    let mut bytes_read = total as i64;
    if os_seek(u, SeekFrom::End(0))? as i64 == lowio {
        bytes_read += count_lf(&u.read_buf) + i64::from(st.ctrl_z);
    } else {
        os_seek(u, SeekFrom::Start(lowio as u64))?;
        bytes_read = if total <= SMALL_BUFSIZ {
            SMALL_BUFSIZ as i64
        } else {
            INTERNAL_BUFSIZ as i64
        };
        if st.crlf {
            bytes_read += 1;
        }
    }
    Ok(lowio - bytes_read + offset)
}

/// `fseek` (`op`: 0 set, 1 cur, 2 end), then `ftell`, as `file:seek` does.
pub(super) fn fseek(u: Gc<Userdata>, op: usize, offset: i64) -> std::io::Result<u64> {
    // what is buffered for output counts in `ftell` as written
    drain_write_buf(u)?;
    let (mut target, mut op) = (offset, op);
    if op == 1 {
        target = target.saturating_add(ftell(u)?);
        op = 0;
    }
    // SAFETY: `u` is held by the caller; `drain_write_buf`'s borrow has ended, and `m` is the only reference into it until `state` is read
    let m = unsafe { u.as_mut() };
    m.read_buf = Vec::new();
    m.read_pos = 0;
    let mut st = state(u);
    if !u.writable && st.has_buffer {
        st.small = true;
    }
    let from = match op {
        0 if target < 0 => {
            set_state(u, st);
            return Err(posix_error(EINVAL));
        }
        0 => SeekFrom::Start(target as u64),
        _ => SeekFrom::End(target),
    };
    st.eof_flag = false;
    set_state(u, st);
    os_seek(u, from)?;
    Ok(ftell(u)? as u64)
}

/// Opening a text mode file for reading and writing drops a Ctrl+Z that
/// ends it, so that appending works.
pub(super) fn drop_final_ctrl_z(f: &mut std::fs::File) -> std::io::Result<()> {
    let len = f.seek(SeekFrom::End(0))?;
    if len > 0 {
        f.seek(SeekFrom::Start(len - 1))?;
        let mut last = [0u8; 1];
        if f.read(&mut last)? == 1 && last[0] == CTRL_Z {
            f.set_len(len - 1)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_and_whole_reads() {
        assert_eq!(&*to_crlf(b"a\nb\r\nc\r"), b"a\r\nb\r\r\nc\r");
        assert_eq!(&*to_crlf(b"plain"), b"plain");
        assert_eq!(translate_all(b"a\r\nb\r\r\nc\rd\x1ae\r\n"), b"a\nb\r\nc\rd");
    }
}
