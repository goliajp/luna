//! The accounting trace's lines about the heap itself (`gc-trace`).

use super::*;

impl Heap {
    /// `LUNA_GC_KEYS`: key `k` enters table `t`'s hash part.
    pub(crate) fn trace_new_key(&mut self, t: &crate::runtime::Table, k: crate::runtime::Value) {
        use crate::runtime::Value;
        if !self.hooks.trace.keys {
            return;
        }
        let key = match k {
            Value::Str(s) if s.is_empty() => String::from("-"),
            Value::Str(s) => s.as_bytes().iter().map(|b| format!("{b:02x}")).collect(),
            Value::Int(i) => format!("#{i}"),
            Value::Float(f) if f.fract() == 0.0 && f.abs() < 1e15 => format!("#{}", f as i64),
            Value::Float(f) => format!("#{f}"),
            other => format!("?{}", other.type_name()),
        };
        self.hooks.trace.line(format_args!(
            "K {:p} {key}",
            t as *const crate::runtime::Table
        ));
    }

    /// `LUNA_GC_TRACE`: a thread of `stack` slots shrunk with `inuse` in use.
    pub(crate) fn trace_shrink(&mut self, stack: u32, inuse: u32) {
        self.hooks.trace.line(format_args!("sh {stack} {inuse}"));
    }

    /// `LUNA_GC_WHERE`: a string `prefix`ed: `s` freed, `s+` made.
    pub(crate) fn trace_string(&mut self, prefix: &[u8], s: Gc<LuaStr>) {
        let s = s.as_ptr();
        if self.hooks.trace.where_ {
            // SAFETY: `s` comes from a handle, so the string is allocated
            let b = String::from_utf8_lossy(unsafe { string::bytes_of(s) }).into_owned();
            let p = std::str::from_utf8(prefix).unwrap_or("s");
            self.hooks.trace.line(format_args!("{p} {b:?}"));
        }
    }

    /// `LUNA_GC_SWEEP`: the string sweep frees string `h`.
    pub(crate) fn trace_string_swept(&mut self, h: Gc<GcHeader>) {
        let h = h.as_ptr();
        if self.hooks.trace.sweep {
            // SAFETY: `h` comes from a handle and the string sweep passes
            // only strings
            let b = unsafe { string::bytes_of(h as *const LuaStr) };
            let b = String::from_utf8_lossy(&b[..b.len().min(24)])
                .escape_debug()
                .to_string();
            self.hooks.trace.line(format_args!("w 4 1 {b}"));
        }
    }
}
