//! Userdata objects. In luna the only userdata are io file handles (there is no
//! C API exposing arbitrary host objects), so a userdata wraps a file/stream
//! handle plus an optional metatable — the shared `FILE*` metatable attached by
//! the io library. Full io handle methods (read/write/seek/…) land with the io
//! file model; this is the GC-level object + identity.

use crate::runtime::heap::{Gc, GcHeader, Marker};
use crate::runtime::table::Table;

#[repr(C)]
pub struct Userdata {
    pub(crate) hdr: GcHeader,
    /// per-object metatable (the io library installs the shared `FILE*` one)
    metatable: Option<Gc<Table>>,
    /// host-side payload
    pub(crate) payload: UserdataPayload,
    /// one-byte read pushback (ungetc) for `file:read("n")`, which must peek one
    /// past the numeral and return it to the stream
    pub(crate) peeked: Option<u8>,
    /// User-space write buffer for `FileHandle::File` (PUC's stdio FILE*).
    /// A `:write` only appends here; the buffer is drained to the OS by
    /// `:flush` / `:seek` / `:close` (and before a `:read` on the same handle).
    /// Without this, writes to `/dev/full` would fail at the `write` call
    /// instead of at `flush`, breaking files.lua :475's expectation.
    pub(crate) write_buf: Vec<u8>,
    /// Whether `:write` should buffer (true for files opened in a write-
    /// capable mode and for `stdout`/`stderr`). A read-only file's write
    /// still goes through `write_to` so the OS surfaces the EBADF — files.lua
    /// :302 asserts `io.input():write(...)` returns `(nil, msg, errno)`.
    pub(crate) writable: bool,
    /// PUC `setvbuf` mode: 0 = `"full"` (default), 1 = `"line"`, 2 = `"no"`.
    /// `"line"` flushes after every newline written; `"no"` flushes after
    /// every write; `"full"` only flushes on close/seek/explicit flush.
    /// files.lua 5.1 :245 baselines on the `"line"` mode behaviour.
    pub(crate) buf_mode: u8,
    /// Child process for an `io.popen` handle. The pipe end (stdout for `"r"`,
    /// stdin for `"w"`) is re-wrapped as a `std::fs::File` and lives in the
    /// `FileHandle::File` slot so all read/write/seek/flush paths stay
    /// untouched; this field keeps the `Child` alive so `:close` can wait on
    /// it and return PUC's `(success, "exit"|"signal", code)` triple. Cleared
    /// on close. Unaffected by `__gc` paths that just drop the pipe — the
    /// process will be reaped by the kernel.
    pub(crate) popen_child: Option<std::process::Child>,
}

/// A userdata's host-side payload. Beyond io file handles luna exposes one
/// more shape — an `Empty` proxy used by PUC 5.1 `newproxy()`, which only
/// carries identity + an optional metatable for `__index` / `__newindex` /
/// `__gc` (5.1 module tests treat it like a metatable hook).
pub enum UserdataPayload {
    /// an io stream/file handle
    File(FileHandle),
    /// a PUC 5.1 `newproxy` userdata — no host payload, only identity
    Empty,
}

/// The OS resource behind a file userdata. Standard streams cannot be closed;
/// an opened file carries its handle and becomes `Closed` after `:close()`.
pub enum FileHandle {
    Stdin,
    Stdout,
    Stderr,
    File(std::fs::File),
    Closed,
}

impl FileHandle {
    /// PUC io.type: an open file vs. a closed one vs. (caller handles non-file).
    pub fn is_closed(&self) -> bool {
        matches!(self, FileHandle::Closed)
    }

    /// Standard streams cannot be closed (io.close(io.stdin) fails in PUC).
    pub fn is_std(&self) -> bool {
        matches!(self, FileHandle::Stdin | FileHandle::Stdout | FileHandle::Stderr)
    }
}

impl Userdata {
    pub(crate) fn new(hdr: GcHeader, payload: UserdataPayload, writable: bool) -> Userdata {
        Userdata {
            hdr,
            metatable: None,
            payload,
            peeked: None,
            write_buf: Vec::new(),
            writable,
            buf_mode: 0,
            popen_child: None,
        }
    }

    pub fn metatable(&self) -> Option<Gc<Table>> {
        self.metatable
    }

    pub fn set_metatable(&mut self, mt: Option<Gc<Table>>) {
        self.metatable = mt;
    }

    pub(crate) fn trace(&self, m: &mut Marker) {
        if let Some(mt) = self.metatable {
            m.header(mt.as_ptr() as *mut GcHeader);
        }
    }

    /// The file handle behind this userdata (all io userdata are files; the
    /// `Empty` proxy variant is only constructed by `newproxy` and surfaces
    /// via [`Self::is_proxy`], so callers reaching `.file()` must already
    /// know they hold a file handle — luna's io builtins all guard with
    /// `is_proxy()` or `Userdata` matching before unpacking).
    pub fn file(&self) -> &FileHandle {
        match &self.payload {
            UserdataPayload::File(fh) => fh,
            UserdataPayload::Empty => panic!("file() on a newproxy userdata"),
        }
    }

    pub fn file_mut(&mut self) -> &mut FileHandle {
        match &mut self.payload {
            UserdataPayload::File(fh) => fh,
            UserdataPayload::Empty => panic!("file_mut() on a newproxy userdata"),
        }
    }

    /// True for `newproxy` userdata — they have no host payload, only a
    /// metatable and identity. io builtins reject these with the PUC
    /// "bad argument" error rather than panicking on `file()`.
    pub fn is_proxy(&self) -> bool {
        matches!(self.payload, UserdataPayload::Empty)
    }
}
