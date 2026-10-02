//! Host-controlled switches: warnings, the instruction budget, the JIT
//! toggles, bytecode loading and the memory cap.

use super::*;

impl Vm {
    /// PUC 5.4+ default warnf: emit one piece of a warning message. `to_cont`
    /// = true indicates more pieces follow (concatenated until the first
    /// `to_cont = false` call flushes the whole line). Mirrors
    /// `lauxlib.c::warnfon` + `warnfcont` + `checkcontrol`:
    ///   * If the buffer is fresh, `to_cont` is false, and the message is
    ///     `@<word>`, treat as a control message — only `@on` / `@off` are
    ///     recognised; any other `@…` is silently ignored.
    ///   * Otherwise, while the state is `Off`, drop the piece; while `On`,
    ///     accumulate, and flush to stderr + `warn_log` on the
    ///     non-continuation call.
    pub(crate) fn emit_warn(&mut self, msg: &[u8], to_cont: bool) {
        if self.warn_buf.is_empty()
            && !to_cont
            && let Some(b'@') = msg.first().copied()
        {
            match &msg[1..] {
                b"on" => self.warn_state = WarnState::On,
                b"off" => self.warn_state = WarnState::Off,
                _ => {} // unknown control — silently ignored (PUC checkcontrol)
            }
            return;
        }
        if self.warn_state == WarnState::Off {
            // drop continuation pieces too — PUC `warnfoff` is the trampoline
            return;
        }
        self.warn_buf.extend_from_slice(msg);
        if !to_cont {
            let line = std::mem::take(&mut self.warn_buf);
            eprintln!("Lua warning: {}", String::from_utf8_lossy(&line));
            self.warn_log.push(line);
        }
    }

    /// Drain the in-process warning log (one entry per emitted message, sans
    /// `"Lua warning: "` prefix and newline). For test harnesses that want to
    /// assert on warn output without scraping stderr.
    pub fn warn_log_take(&mut self) -> Vec<Vec<u8>> {
        std::mem::take(&mut self.warn_log)
    }

    /// Arm the cooperative instruction budget. The run loop
    /// decrements this once per dispatch turn; on zero it raises a catchable
    /// `"instruction budget exceeded"` error and disarms itself so the host
    /// can resume with a fresh budget on the next call. `None` removes the
    /// cap. Pass `Some(n)` before `eval`/`call_value` for the embedder's
    /// short-script semantics.
    pub fn set_instr_budget(&mut self, budget: Option<i64>) {
        self.instr_budget = budget;
        self.trap = true;
    }

    /// Remaining instruction budget (None when unbounded).
    pub fn instr_budget_remaining(&self) -> Option<i64> {
        self.instr_budget
    }

    /// Toggle the method JIT. Off on a Vm without a JIT backend, on once
    /// one is installed ([`Self::install_jit_backend`]); a value set here
    /// is kept across a later install. Sandbox embedders
    /// **must** disable JIT when relying on `instr_budget` — see the
    /// `jit_enabled` field doc for the rationale.
    pub fn set_jit_enabled(&mut self, enabled: bool) {
        self.jit.enabled = enabled;
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
    /// optimizing tier (`0`: never).
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
    /// raises a catchable `"memory cap exceeded"` Lua error and disarms
    /// itself (fire-once: re-arm before the next `call_value` if reusing
    /// the Vm across requests). `None` removes the cap. The accounting is
    /// approximate — internal Vec/Box capacity overhead is not tracked,
    /// so embedders should size the cap with ~2× margin over the desired
    /// hard limit and additionally bound the Vm's lifetime (drop after
    /// each request).
    pub fn set_memory_cap(&mut self, cap: Option<usize>) {
        self.heap.mem_cap = cap;
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
