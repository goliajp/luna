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
    /// `R38_NOAUTO`: no automatic step runs, a full collection does
    pub(crate) no_auto: bool,
    /// `R38_WHERE`: each line also names the luna functions that made it
    pub(crate) where_: bool,
}

impl Default for Trace {
    fn default() -> Trace {
        let out = std::env::var_os("R38_TRACE")
            .and_then(|p| std::fs::File::create(p).ok())
            .map(|f| std::io::BufWriter::with_capacity(1 << 16, f));
        Trace {
            out,
            no_gc: std::env::var_os("R38_NOGC").is_some(),
            no_auto: std::env::var_os("R38_NOAUTO").is_some(),
            where_: std::env::var_os("R38_WHERE").is_some(),
        }
    }
}

impl Trace {
    pub(crate) fn flush(&mut self) {
        if let Some(o) = &mut self.out {
            let _ = o.flush();
        }
    }

    pub(crate) fn line(&mut self, args: std::fmt::Arguments<'_>) {
        if let Some(o) = &mut self.out {
            let _ = o.write_fmt(args);
            if self.where_ {
                let _ = write!(o, "  {}", callers());
            }
            let _ = o.write_all(b"\n");
        }
    }
}

/// The innermost luna functions on the stack, past the log's own.
fn callers() -> String {
    let bt = std::backtrace::Backtrace::force_capture().to_string();
    let mut out = Vec::new();
    for l in bt.lines() {
        let Some((_, name)) = l.trim().split_once(": ") else {
            continue;
        };
        if !name.contains("luna") || name.contains("gc_trace") || name.contains("acct_") {
            continue;
        }
        // the symbol's hash
        let name = match name.rsplit_once("::h") {
            Some((n, h)) if h.len() == 16 => n,
            _ => name,
        };
        let short = name.rsplit("::").take(2).collect::<Vec<_>>();
        out.push(format!("{}::{}", short.get(1).unwrap_or(&""), short[0]));
        if out.len() == 12 {
            break;
        }
    }
    out.join("<")
}
