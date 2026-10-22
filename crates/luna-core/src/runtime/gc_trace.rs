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
    /// `R38_SWEEP`: each object of PUC's a step's sweep goes over (`w <tag>
    /// <dead>`; a record of PUC's objects `w g<n> <dead>`)
    pub(crate) sweep: bool,
    window: Option<(u64, u64)>,
    events: u64,
    /// lines held back (see [`Trace::hold`]), innermost last
    held: Vec<Vec<String>>,
    /// a C function of PUC's runs where luna has no level for it yet: a
    /// checkpoint shows no line
    pub(crate) in_c: bool,
    /// a call hook runs: PUC counts the function as past its first
    /// instruction then (`luaD_hookcall`)
    pub(crate) call_hook: bool,
    /// `R38_REFUSE=n`: the instrumented PUC's allocation function refuses
    /// the first growth of a block to n bytes or more (every one with
    /// `R38_REFUSE_EVERY`); here the growth is PUC's, as the trace has it,
    /// and what follows it is held until the VM has done what PUC does then
    refuse: Option<usize>,
    pub(crate) refuse_every: bool,
    /// a refused growth waits: the hold that begins with it
    refused: Option<usize>,
    /// the bytes counted since it, itself included
    refused_bytes: i64,
    /// the collector is in generational mode (the steps' lines say so)
    pub(crate) generational: bool,
    /// the line being written is a block PUC leaves out of its count
    pub(crate) raw: bool,
    /// the refused growth was a block PUC leaves out of its count
    pub(crate) refused_raw: bool,
    /// PUC's `L->top` for an emergency collection that answers a refusal
    /// where the VM cannot tell it (a stack growth)
    pub(crate) alloc_top: Option<u32>,
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
            sweep: std::env::var_os("R38_SWEEP").is_some(),
            keys: std::env::var_os("R38_KEYS").is_some(),
            window: std::env::var("R38_WHERE")
                .ok()
                .and_then(|w| w.parse::<u64>().ok())
                .map(|n| (n.saturating_sub(30), n + 30)),
            events: 0,
            held: Vec::new(),
            in_c: false,
            call_hook: false,
            refuse: std::env::var("R38_REFUSE")
                .ok()
                .and_then(|n| n.parse().ok()),
            refuse_every: std::env::var_os("R38_REFUSE_EVERY").is_some(),
            refused: None,
            refused_bytes: 0,
            generational: false,
            raw: false,
            refused_raw: false,
            alloc_top: None,
        }
    }
}

impl Trace {
    /// A block going from `old` to `new` bytes: `a 0 <new>`, `d <old>` or
    /// `r <old> <new>`.
    pub(crate) fn block(&mut self, old: usize, new: usize) {
        match (old, new) {
            (0, 0) => {}
            (0, n) => self.line(format_args!("a 0 {n}")),
            (o, 0) => self.line(format_args!("d {o}")),
            (o, n) => self.line(format_args!("r {o} {n}")),
        }
    }

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
        // a refused growth's hold stays: the one below it is released
        if let Some(at) = self.refused
            && at > 0
            && at + 1 == self.held.len()
        {
            self.refused = Some(at - 1);
            return self.held.remove(at - 1);
        }
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
        if self.refuse.is_some() || self.refused.is_some() {
            self.watch_refusal(&std::fmt::format(args));
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

impl Trace {
    /// `text`, about to be written: a growth the refusal takes, or one
    /// counted while it waits
    fn watch_refusal(&mut self, text: &str) {
        let nums: Vec<i64> = text
            .split(' ')
            .skip(1)
            .filter_map(|w| w.parse().ok())
            .collect();
        let (old, new) = match (text.as_bytes().first(), nums.as_slice()) {
            (Some(b'a'), [_, n]) => (0, *n),
            (Some(b'r'), [o, n]) => (*o, *n),
            (Some(b'd'), [o]) => (*o, 0),
            _ => return,
        };
        if self.refused.is_none()
            && let Some(limit) = self.refuse
            && new >= limit as i64
            && new > old
        {
            if !self.refuse_every {
                self.refuse = None;
            }
            self.held.push(Vec::new());
            self.refused = Some(self.held.len() - 1);
            self.refused_raw = self.raw;
        }
        if self.refused.is_some() && !self.raw {
            self.refused_bytes += new - old;
        }
    }

    /// A growth of PUC's from `old` to `new` bytes about to be made that
    /// `R38_REFUSE` refuses: the allocation fails there instead.
    pub(crate) fn refuse_now(&mut self, old: usize, new: usize) -> bool {
        if self.out.is_none() || self.refused.is_some() {
            return false;
        }
        match self.refuse {
            Some(n) if new >= n && new > old => {
                if !self.refuse_every {
                    self.refuse = None;
                }
                true
            }
            _ => false,
        }
    }

    /// Whether a refused growth waits.
    pub(crate) fn refused(&self) -> bool {
        self.refused.is_some()
    }

    /// The lines held since the refused growth and the bytes they count.
    pub(crate) fn take_refused(&mut self) -> (Vec<String>, i64) {
        let at = self.refused.take().expect("a refusal waits");
        (
            self.held.remove(at),
            std::mem::take(&mut self.refused_bytes),
        )
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

/// The tag PUC passes its allocation function for a new object of `kind`
/// (5.1 passes none).
pub(crate) fn puc_tag(
    kind: crate::runtime::puc::PucKind,
    d: crate::runtime::dialect::Dialect,
) -> usize {
    use crate::runtime::dialect::Dialect;
    use crate::runtime::puc::PucKind::*;
    match (d, kind) {
        (Dialect::L51, _) => 0,
        (_, ShortStr) => 4,
        // PUC passes the base type, not the variant
        (_, LongStr) => 4,
        (_, Table) => 5,
        (_, LuaClosure) => 6,
        (_, CClosure) => 6,
        (_, Userdata) => 7,
        (_, Thread) => 8,
        // 5.3's upvalues are plain blocks
        (Dialect::L53, Upval) => 0,
        (Dialect::L52 | Dialect::L53, Proto) => 9,
        (Dialect::L52, Upval) => 10,
        (_, Upval) => 9,
        (_, Proto) => 10,
    }
}

/// The seed the instrumented PUC build takes from `R38_SEED` too.
pub(crate) fn fixed_seed() -> Option<u32> {
    std::env::var("R38_SEED").ok().and_then(|s| s.parse().ok())
}
