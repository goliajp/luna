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
    /// `R38_WHERE`: each line also names the luna functions that made it;
    /// `R38_WHERE=<n>` only the allocation events within 30 of the n-th
    /// (counted from 1, as `where.sh` counts PUC's)
    pub(crate) where_: bool,
    /// `R38_KEYS`: each key a table's hash part gets for the first time
    /// (`K <table> <key>`, as `libshape_patch.py` logs PUC's)
    pub(crate) keys: bool,
    /// `R38_PROP`: each object an incremental step traverses, with its
    /// type and the work counted for it (`p <tag> <work>`)
    pub(crate) prop: bool,
    window: Option<(u64, u64)>,
    events: u64,
    /// lines held back (see [`Trace::hold`]), innermost last
    held: Vec<Vec<String>>,
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
            prop: std::env::var_os("R38_PROP").is_some(),
            keys: std::env::var_os("R38_KEYS").is_some(),
            window: std::env::var("R38_WHERE")
                .ok()
                .and_then(|w| w.parse::<u64>().ok())
                .map(|n| (n.saturating_sub(30), n + 30)),
            events: 0,
            held: Vec::new(),
        }
    }
}

impl Trace {
    pub(crate) fn flush(&mut self) {
        if let Some(o) = &mut self.out {
            let _ = o.flush();
        }
    }

    /// Hold back the lines from now on until [`Trace::release`].
    pub(crate) fn hold(&mut self) {
        self.held.push(Vec::new());
    }

    /// The lines held back since the matching [`Trace::hold`].
    pub(crate) fn release(&mut self) -> Vec<String> {
        self.held.pop().unwrap_or_default()
    }

    /// Write lines held back before, where they belong now.
    pub(crate) fn put(&mut self, lines: Vec<String>) {
        for l in lines {
            self.line(format_args!("{l}"));
        }
    }

    pub(crate) fn line(&mut self, args: std::fmt::Arguments<'_>) {
        if self.out.is_none() {
            return;
        }
        if let Some(h) = self.held.last_mut() {
            h.push(std::fmt::format(args));
            return;
        }
        let Some(o) = &mut self.out else {
            return;
        };
        if !self.where_ {
            let _ = o.write_fmt(args);
            let _ = o.write_all(b"\n");
            return;
        }
        let text = std::fmt::format(args);
        let shown = match self.window {
            Some((lo, hi)) => {
                if text.starts_with(['a', 'r', 'd']) {
                    self.events += 1;
                }
                (lo..=hi).contains(&self.events)
            }
            None => true,
        };
        let _ = o.write_all(text.as_bytes());
        if shown {
            let _ = write!(o, "  {}", callers());
        }
        let _ = o.write_all(b"\n");
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
