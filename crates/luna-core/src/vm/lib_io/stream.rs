//! The stream: buffered bytes over the OS handle.

use super::*;

/// Refill the input buffer; `false` at end of file.
pub(super) fn fill(u: Gc<Userdata>) -> std::io::Result<bool> {
    // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
    let m = unsafe { u.as_mut() };
    let mut chunk = vec![0u8; READ_CHUNK];
    let n = match m.file_mut() {
        FileHandle::File(f) => f.read(&mut chunk)?,
        FileHandle::Stdin => std::io::stdin().read(&mut chunk)?,
        // stdout/stderr are write-only streams
        FileHandle::Stdout | FileHandle::Stderr => {
            return Err(posix_error(EBADF));
        }
        FileHandle::Closed => unreachable!("reads check the stream is open"),
    };
    chunk.truncate(n);
    m.read_buf = chunk;
    m.read_pos = 0;
    Ok(n > 0)
}

/// `getc`.
pub(super) fn getc(u: Gc<Userdata>) -> std::io::Result<Option<u8>> {
    if u.read_pos >= u.read_buf.len() && !fill(u)? {
        return Ok(None);
    }
    // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
    let m = unsafe { u.as_mut() };
    let b = m.read_buf[m.read_pos];
    m.read_pos += 1;
    Ok(Some(b))
}

/// `ungetc` of any number of bytes: the next reads return `bytes` first.
pub(super) fn unget(u: Gc<Userdata>, bytes: &[u8]) {
    // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
    let m = unsafe { u.as_mut() };
    if m.read_pos >= bytes.len() && m.read_buf[m.read_pos - bytes.len()..m.read_pos] == *bytes {
        m.read_pos -= bytes.len();
        return;
    }
    let mut buf = bytes.to_vec();
    buf.extend_from_slice(&m.read_buf[m.read_pos..]);
    m.read_buf = buf;
    m.read_pos = 0;
}

/// Bytes buffered ahead of the logical position.
pub(super) fn read_ahead(u: Gc<Userdata>) -> i64 {
    (u.read_buf.len() - u.read_pos) as i64
}

/// Give back read-ahead before the position is used for something else
/// (a write, a seek): the OS position is that far past the logical one.
fn unread_ahead(u: Gc<Userdata>) -> std::io::Result<()> {
    let ahead = read_ahead(u);
    // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
    let m = unsafe { u.as_mut() };
    if ahead > 0
        && let FileHandle::File(f) = m.file_mut()
    {
        f.seek(SeekFrom::Current(-ahead))?;
    }
    m.read_buf = Vec::new();
    m.read_pos = 0;
    Ok(())
}

/// Write `bytes` straight to the OS handle.
fn write_to(u: Gc<Userdata>, bytes: &[u8]) -> std::io::Result<()> {
    // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
    match unsafe { u.as_mut() }.file_mut() {
        FileHandle::File(f) => f.write_all(bytes),
        FileHandle::Stdout => std::io::stdout().write_all(bytes),
        FileHandle::Stderr => std::io::stderr().write_all(bytes),
        FileHandle::Stdin => Err(posix_error(EBADF)),
        FileHandle::Closed => unreachable!("writes check the stream is open"),
    }
}

/// Drain the output buffer to the OS. The buffer is emptied either way: C
/// stdio drops what it failed to write and reports the error.
pub(super) fn drain_write_buf(u: Gc<Userdata>) -> std::io::Result<()> {
    // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
    let buf = std::mem::take(&mut unsafe { u.as_mut() }.write_buf);
    if buf.is_empty() {
        return Ok(());
    }
    write_to(u, &buf)
}

/// Put `bytes` on the stream through its buffering mode.
pub(super) fn put_bytes(u: Gc<Userdata>, bytes: &[u8]) -> std::io::Result<()> {
    if !matches!(u.file(), FileHandle::File(_)) || !u.writable {
        // standard streams are buffered by std; a read-only file fails here
        return write_to(u, bytes);
    }
    unread_ahead(u)?;
    // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
    let m = unsafe { u.as_mut() };
    match m.buf_mode {
        BUF_NO => write_to(u, bytes),
        BUF_LINE => {
            m.write_buf.extend_from_slice(bytes);
            match m.write_buf.iter().rposition(|&b| b == b'\n') {
                Some(nl) => {
                    let out: Vec<u8> = m.write_buf.drain(..=nl).collect();
                    write_to(u, &out)
                }
                None => Ok(()),
            }
        }
        _ => {
            m.write_buf.extend_from_slice(bytes);
            Ok(())
        }
    }
}

/// `setvbuf` modes as kept in `Userdata::buf_mode`.
pub(super) const BUF_FULL: u8 = 0;
pub(super) const BUF_LINE: u8 = 1;
pub(super) const BUF_NO: u8 = 2;

pub(super) fn flush_stream(u: Gc<Userdata>) -> std::io::Result<()> {
    drain_write_buf(u)?;
    // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
    match unsafe { u.as_mut() }.file_mut() {
        FileHandle::File(f) => f.flush(),
        FileHandle::Stdout => std::io::stdout().flush(),
        FileHandle::Stderr => std::io::stderr().flush(),
        FileHandle::Stdin | FileHandle::Closed => Ok(()),
    }
}
