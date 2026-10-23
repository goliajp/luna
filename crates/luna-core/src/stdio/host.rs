//! What the C API hands over to write standard output through.

/// The C library's own `stdout`, as a C host linking luna's C API has it:
/// the C API routes standard output there, so it interleaves with what the
/// host writes with `printf` as PUC's output does.
pub struct HostStdout {
    /// `fwrite(bytes, 1, n, stdout)`; `false` on a short write
    pub write: fn(&[u8]) -> bool,
    /// `fflush(stdout)`; `false` on failure
    pub flush: fn() -> bool,
    /// `setvbuf(stdout, NULL, mode, BUFSIZ)`: `0` full, `1` line, `2` none
    pub setvbuf: fn(u8),
}
