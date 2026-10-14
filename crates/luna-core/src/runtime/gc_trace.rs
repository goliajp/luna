//! A log of what the collector's accounting saw, for comparing luna with an
//! instrumented PUC build line by line (`gc-trace` feature). The file is
//! named by the `R38_TRACE` environment variable; the format is PUC's
//! instrumented allocation function's: `a <tag> <size>` for a new object or
//! block, `r <old> <new>` for a resize, `d <size>` for a free.

use std::io::Write;

/// The log of one heap: nothing when `R38_TRACE` is unset.
pub(crate) struct Trace {
    out: Option<std::io::BufWriter<std::fs::File>>,
    /// `R38_NOGC`: no collection runs, as in the instrumented PUC build
    /// with the same variable set
    pub(crate) no_gc: bool,
}

impl Default for Trace {
    fn default() -> Trace {
        let out = std::env::var_os("R38_TRACE")
            .and_then(|p| std::fs::File::create(p).ok())
            .map(|f| std::io::BufWriter::with_capacity(1 << 16, f));
        Trace {
            out,
            no_gc: std::env::var_os("R38_NOGC").is_some(),
        }
    }
}

impl Trace {
    pub(crate) fn line(&mut self, args: std::fmt::Arguments<'_>) {
        if let Some(o) = &mut self.out {
            let _ = o.write_fmt(args);
            let _ = o.write_all(b"\n");
        }
    }
}
