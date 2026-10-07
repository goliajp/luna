//! Host-controlled switches: warnings, the instruction budget, the JIT
//! toggles, bytecode loading and the memory cap.

use super::*;

impl Vm {
    /// PUC 5.4+ warnf: emit one piece of a warning message, `to_cont` when
    /// more pieces follow. The C API's warning function gets it when one
    /// is installed (an error it raises comes back); otherwise the default
    /// one of `lauxlib.c` handles it, a state machine of `warnfoff`,
    /// `warnfon` and `warnfcont`:
    ///   * a piece that starts a message, is the whole message and reads
    ///     `@<word>` is a control message: `@on` / `@off` switch, any other
    ///     is ignored;
    ///   * while off, every other piece is dropped;
    ///   * while on, a message's pieces go to stderr as they come, after
    ///     `Lua warning: ` and followed by a newline at its last piece; the
    ///     whole message also goes to `warn_log`.
    pub(crate) fn emit_warn(&mut self, msg: &[u8], to_cont: bool) -> Result<(), LuaError> {
        if let Some(r) = self.host_warn_piece(msg, to_cont) {
            return r;
        }
        if !self.warn_cont && !to_cont && msg.first() == Some(&b'@') {
            match &msg[1..] {
                b"on" => self.warn_state = WarnState::On,
                b"off" => self.warn_state = WarnState::Off,
                _ => {}
            }
            return Ok(());
        }
        if self.warn_state == WarnState::Off {
            return Ok(());
        }
        let err = crate::stdio::write_stderr;
        if !self.warn_cont {
            let _ = err(b"Lua warning: ");
        }
        let _ = err(msg);
        self.warn_buf.extend_from_slice_or_abort(msg);
        self.warn_cont = to_cont;
        if !to_cont {
            let _ = err(b"\n");
            let line = self.warn_buf.take();
            self.warn_log.push_or_abort(line);
        }
        Ok(())
    }

    /// PUC `luaE_warnerror`: warn `error in <place> (<message>)` in five
    /// pieces, `error object is not a string` standing for an error object
    /// that is not a string. The collector warns this way from a finalizer
    /// loop that has no caller to hand an error of the warning function to,
    /// so such an error ends the warning and is dropped.
    pub(crate) fn warn_error(&mut self, place: &str, err: Value) {
        let msg = match err {
            Value::Str(s) => s.as_bytes().to_vec(),
            _ => b"error object is not a string".to_vec(),
        };
        let pieces: [(&[u8], bool); 5] = [
            (b"error in ", true),
            (place.as_bytes(), true),
            (b" (", true),
            (&msg, true),
            (b")", false),
        ];
        for (piece, to_cont) in pieces {
            if self.emit_warn(piece, to_cont).is_err() {
                return;
            }
        }
    }

    /// Drain the in-process warning log (one entry per emitted message, sans
    /// `"Lua warning: "` prefix and newline). For test harnesses that want to
    /// assert on warn output without scraping stderr.
    pub fn warn_log_take(&mut self) -> Vec<Vec<u8>> {
        let log = self.warn_log.take();
        log.iter().map(|l| l.to_vec()).collect()
    }

    /// Arm the cooperative instruction budget. The run loop
    /// decrements this once per dispatch turn; on zero it raises the
    /// `"instruction budget exceeded"` error ([`LuaErrorKind::InstrBudget`])
    /// and the budget stays exhausted: until the next call of this method
    /// every further instruction raises the same error again, whoever runs
    /// it — the script after a `pcall` that caught it, an `xpcall` handler,
    /// a `__close` or `__gc` handler, a metamethod, a library callback such
    /// as a sort comparator, a coroutine — so the script cannot catch the
    /// error and carry on. `None` removes the budget. Pass `Some(n)` before
    /// each `eval` / `call_value` for the embedder's short-script
    /// semantics. While a budget is armed no compiled code runs: the loops
    /// the JIT would compile stay in the interpreter, where they are
    /// counted.
    ///
    /// [`LuaErrorKind::InstrBudget`]: crate::vm::error::LuaErrorKind::InstrBudget
    pub fn set_instr_budget(&mut self, budget: Option<i64>) {
        self.instr_budget = budget;
        self.sync_limited();
        self.trap = true;
    }

    /// Remaining instruction budget: `None` when unbounded, `Some(0)` once
    /// it ran out.
    pub fn instr_budget_remaining(&self) -> Option<i64> {
        self.instr_budget
    }

    /// Toggle the method JIT. Off on a Vm without a JIT backend, on once
    /// one is installed ([`Self::install_jit_backend`]); a value set here
    /// is kept across a later install. An armed instruction budget or
    /// memory cap keeps compiled code out on its own (see
    /// [`Self::set_instr_budget`]).
    pub fn set_jit_enabled(&mut self, enabled: bool) {
        self.set_jit_flag(enabled);
        self.jit.enabled_chosen = true;
    }

    /// Current JIT enable state.
    pub fn jit_enabled(&self) -> bool {
        self.jit.enabled
    }

    /// Toggle the trace JIT. Same default and install rule as
    /// [`Self::set_jit_enabled`]. When enabled, hot
    /// back-edges are counted on `Proto.trace_hot_count`; once the
    /// counter passes `TRACE_HOT_THRESHOLD`, the dispatch loop enters
    /// recording mode at the back-edge target.
    pub fn set_trace_jit_enabled(&mut self, enabled: bool) {
        self.jit.trace_enabled = enabled;
        self.jit.trace_enabled_chosen = true;
    }

    /// Which code generator compiles this Vm's traces from now on.
    pub fn set_trace_tier(&mut self, tier: crate::jit::trace::TraceTier) {
        self.jit.trace_tier = tier;
    }

    /// See [`Self::set_trace_tier`].
    pub fn trace_tier(&self) -> crate::jit::trace::TraceTier {
        self.jit.trace_tier
    }

    /// With [`crate::jit::trace::TraceTier::Auto`]: the loop iterations
    /// plus entries after which a trace compiled from now on moves to the
    /// optimizing tier (`0`: never); a quarter of that once the function the
    /// trace starts in has been called again since it was compiled.
    pub fn set_trace_tier_up_at(&mut self, n: u32) {
        self.jit.tier_up_at = n;
    }

    /// Opt-in flag for the self-link cycle catch. See field
    /// docs for the correctness blocker. Default `false`.
    pub fn set_self_link_enabled(&mut self, enabled: bool) {
        self.jit.self_link_enabled = enabled;
    }

    /// Current state of the self-link cycle catch.
    pub fn self_link_enabled(&self) -> bool {
        self.jit.self_link_enabled
    }

    /// Toggle the table-field inline cache for traces this Vm records
    /// from now on. Defaults to the `LUNA_JIT_FIELD_IC` environment
    /// variable (`1` or `true` turns it on).
    pub fn set_field_ic_enabled(&mut self, enabled: bool) {
        self.jit.field_ic_enabled = enabled;
    }

    /// Current state of the table-field inline cache.
    pub fn field_ic_enabled(&self) -> bool {
        self.jit.field_ic_enabled
    }

    /// Before recording a trace, ask the trace compiler for one other Vms
    /// compiled for code of the same content, and remember the chunks
    /// loaded from now on so such traces find the functions they inlined.
    /// The JIT crate's shared engine turns this on for the Vms it builds.
    #[doc(hidden)]
    pub fn enable_trace_sharing(&mut self) {
        self.jit.share_traces = true;
        self.heap.track_chunk_roots = true;
    }

    #[doc(hidden)]
    #[deprecated(since = "3.2.0", note = "renamed to `set_self_link_enabled`")]
    pub fn set_p16_self_link_enabled(&mut self, enabled: bool) {
        self.set_self_link_enabled(enabled);
    }

    #[doc(hidden)]
    #[deprecated(since = "3.2.0", note = "renamed to `self_link_enabled`")]
    pub fn p16_self_link_enabled(&self) -> bool {
        self.self_link_enabled()
    }

    /// Current trace-JIT enable state.
    pub fn trace_jit_enabled(&self) -> bool {
        self.jit.trace_enabled
    }

    /// Toggle precompiled-chunk loading. Default `true`. Sandbox embedders
    /// should set to `false` so `load`/`loadstring` reject bytecode input
    /// (which bypasses parser limits and could exploit verifier gaps).
    pub fn set_bytecode_loading(&mut self, enabled: bool) {
        self.bytecode_loading = enabled;
    }

    /// Current bytecode-loading gate state.
    pub fn bytecode_loading(&self) -> bool {
        self.bytecode_loading
    }

    /// Toggle PUC `.luac` bytecode loading. Default `false` — PUC
    /// bytecode is a strictly larger trust surface than luna's own dump
    /// format (third-party toolchain bugs, malformed chunks, unknown
    /// opcode shapes). Enable only for trusted PUC chunks. Per-dialect
    /// translators live in `crate::vm::dump::puc`.
    pub fn set_puc_bytecode_loading(&mut self, enabled: bool) {
        self.puc_bytecode_loading = enabled;
    }

    /// Current PUC bytecode-loading gate state.
    pub fn puc_bytecode_loading(&self) -> bool {
        self.puc_bytecode_loading
    }

    /// Open files as PUC built with MSVC does: a file opened without `b`
    /// (by `io.open`, `io.lines`, `io.input`, `io.output`, `io.popen`, and
    /// the source `loadfile`, `dofile` and `require` read) is in the C
    /// library's text mode, so `\r\n` reads as `\n`, a Ctrl+Z ends the
    /// input, `\n` is written as `\r\n`, and `seek` reports the positions
    /// that library's `ftell` gives. Default `false`, on every platform; the
    /// `luna` command sets it on Windows, as `lua.exe` behaves there. Files
    /// already open keep their mode.
    pub fn set_crt_text_mode(&mut self, on: bool) {
        self.crt_text = on;
    }

    /// Default loader input budget — 256 MiB.
    ///
    /// `Vm::load` and the Lua-level `load(reader, ...)` both refuse
    /// sources whose byte length crosses this cap, returning the
    /// PUC-shaped `not enough memory` error rather than letting the
    /// host allocator try (and crash) to hold the next chunk.
    pub const DEFAULT_LOADER_INPUT_BUDGET: usize = 256 * 1024 * 1024;

    /// Set the loader input byte budget (see
    /// [`Vm::DEFAULT_LOADER_INPUT_BUDGET`]). Pass `usize::MAX` to
    /// effectively disable. Smaller caps are honored verbatim — a 0
    /// cap rejects every non-empty source.
    pub fn set_loader_input_budget(&mut self, bytes: usize) {
        self.loader_input_budget = bytes;
    }

    /// Current loader input byte budget.
    pub fn loader_input_budget(&self) -> usize {
        self.loader_input_budget
    }

    /// Arm the soft memory cap. The run loop checks the
    /// heap's tracked byte usage between dispatch turns; on overshoot it
    /// first runs a full collect, and if `bytes` still exceeds the cap it
    /// raises the `"memory cap exceeded"` Lua error
    /// ([`LuaErrorKind::MemoryCap`]). Like the instruction budget the cap
    /// then stays exceeded: every further instruction raises the error
    /// again until the next call of this method (see
    /// [`Self::set_instr_budget`] for who that stops). `None` removes the
    /// cap; re-arm before the next `call_value` when reusing the Vm across
    /// requests. The accounting is approximate — internal Vec/Box capacity
    /// overhead is not tracked, so embedders should size the cap with ~2×
    /// margin over the desired hard limit and additionally bound the Vm's
    /// lifetime (drop after each request). While a cap is armed no
    /// compiled code runs.
    ///
    /// [`LuaErrorKind::MemoryCap`]: crate::vm::error::LuaErrorKind::MemoryCap
    pub fn set_memory_cap(&mut self, cap: Option<usize>) {
        self.heap.mem_cap = cap;
        self.sync_limited();
        self.trap = true;
    }

    /// Approximate bytes the heap is currently holding. Object shells plus
    /// every table's internal array/hash boxes (tracked via
    /// `Heap::apply_bytes_delta` in `set`/`rehash`/`ensure_*`). Proto
    /// bytecode and closure upvalue slices still go uncounted — this is a
    /// lower bound, not a precise `malloc_stats`-style total.
    pub fn memory_used(&self) -> usize {
        self.heap.bytes()
    }
}
