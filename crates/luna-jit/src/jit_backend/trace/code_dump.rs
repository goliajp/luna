//! `LUNA_TRACE_CODE_DUMP=<dir>` writes the machine code of each trace
//! Cranelift compiles (`clif-N.bin`, for `objdump -D -b binary`) into
//! `<dir>`, and a line per trace to `<dir>/index.txt`: the number, how it
//! was compiled, its head pc, its address, its length, and the address's
//! offset in its page and in a 64-byte line. Off unless the variable is
//! set. The baseline tier has its own dump (`LUNA_BASELINE_DUMP`).

use std::cell::Cell;
use std::io::Write as _;
use std::sync::atomic::{AtomicU32, Ordering};

thread_local! {
    /// The size of the function the last `define_function` on this thread
    /// produced.
    static LAST_SIZE: Cell<usize> = const { Cell::new(0) };
}

fn dir() -> Option<std::path::PathBuf> {
    static DIR: std::sync::OnceLock<Option<std::path::PathBuf>> = std::sync::OnceLock::new();
    DIR.get_or_init(|| std::env::var_os("LUNA_TRACE_CODE_DUMP").map(Into::into))
        .clone()
}

/// Remember the size of the function just defined, when dumping.
pub(super) fn note_len(n: usize) {
    if dir().is_some() {
        LAST_SIZE.with(|s| s.set(n));
    }
}

/// Write the function at `ptr`, the one [`note_size`] last measured.
pub(super) fn dump(kind: &str, head_pc: u32, ptr: *const u8) {
    if dir().is_none() {
        return;
    }
    let len = LAST_SIZE.with(|s| s.replace(0));
    dump_len(kind, head_pc, ptr, len);
}

/// Write the `len` bytes of code at `ptr`.
pub(super) fn dump_len(kind: &str, head_pc: u32, ptr: *const u8, len: usize) {
    let Some(dir) = dir() else { return };
    if len == 0 {
        return;
    }
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    // SAFETY: `ptr` is code just finalized or placed, `len` bytes of
    // readable memory, and nothing writes them while they are copied
    let code = unsafe { std::slice::from_raw_parts(ptr, len) };
    let _ = std::fs::write(dir.join(format!("clif-{n}.bin")), code);
    let a = ptr as usize;
    let line = format!(
        "{n} {kind} head_pc={head_pc} addr={a:#x} len={len} page_off={} line_off={}\n",
        a % 4096,
        a % 64
    );
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("index.txt"))
    {
        let _ = f.write_all(line.as_bytes());
    }
}
