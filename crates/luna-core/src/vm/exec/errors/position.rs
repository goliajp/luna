//! Where an error is raised: the position `luaG_addinfo` puts before it.

use super::*;

impl Vm {
    /// Where `luaG_addinfo` positions an error raised now: `None` when the
    /// running function is not a Lua one.
    pub(crate) fn error_position(&self) -> Option<ErrPos> {
        let f = match self.frames.last()? {
            CallFrame::Lua(f) => f,
            // a native metamethod runs above the Meta continuation of the
            // instruction that triggered it: that Lua function is its caller
            CallFrame::Cont(NativeCont {
                kind: ContKind::Meta(_),
                ..
            }) => self.frames.iter().rev().nth(1)?.lua()?,
            CallFrame::Cont(_) => return None,
        };
        Some(self.pos_at(f.closure.proto, f.pc as usize))
    }

    /// The position of the instruction before `pc` in `proto`.
    pub(in crate::vm::exec) fn pos_at(
        &self,
        proto: Gc<crate::runtime::function::Proto>,
        pc: usize,
    ) -> ErrPos {
        // a stripped chunk: no source in luna's own format, no line info in
        // PUC's (whose loader names the missing source "=?")
        if proto.source.as_bytes().is_empty() || proto.lines.is_empty() {
            return ErrPos::NoSource;
        }
        let line = proto.lines[pc.saturating_sub(1).min(proto.lines.len() - 1)];
        let raw = proto.source.as_bytes();
        ErrPos::At(
            crate::vm::lib_debug::chunk_id(self.version, raw),
            i64::from(line),
        )
    }

    /// `"short_src:line: "` of the instruction before `pc` in `proto`.
    pub(in crate::vm::exec) fn prefix_at(
        &self,
        proto: Gc<crate::runtime::function::Proto>,
        pc: usize,
    ) -> String {
        // a stripped chunk: no source in luna's own format, no line info in
        // PUC's (whose loader names the missing source "=?")
        if proto.source.as_bytes().is_empty() || proto.lines.is_empty() {
            return self.stripped_prefix();
        }
        let line = proto.lines[pc.saturating_sub(1).min(proto.lines.len() - 1)];
        let raw = proto.source.as_bytes();
        let display = crate::vm::lib_debug::chunk_id(self.version, raw);
        let src = String::from_utf8_lossy(&display).into_owned();
        format!("{src}:{line}: ")
    }

    /// PUC `luaG_addinfo` prefix for a stripped chunk. 5.5 substitutes "=?"
    /// for the source and renders the line as "?" (so the prefix reads
    /// `?:?: `). 5.4 and below leave the source NULL ("?") and use the raw
    /// `getfuncline = -1`, so the prefix reads `?:-1: ` (5.4 errors.lua :282
    /// matches `^%?:%-1:`).
    pub(in crate::vm::exec) fn stripped_prefix(&self) -> String {
        if self.version >= crate::version::LuaVersion::Lua55 {
            "?:?: ".to_string()
        } else {
            "?:-1: ".to_string()
        }
    }

    /// The `short_src` and current line `luaL_where(L, level)` reports.
    pub(crate) fn position_at_level(&self, level: i64) -> Option<(Vec<u8>, i64)> {
        let ts = self.thread_stack(None);
        let i = usize::try_from(level).ok()?;
        if i >= ts.levels.len() {
            return None;
        }
        let line = ts.currentline(i);
        if line <= 0 {
            return None;
        }
        let DbgKind::Lua(fi) = ts.levels[i] else {
            return None;
        };
        let ar = self.closure_ar(ts.lua(fi).closure);
        Some((ar.short_src, line))
    }
}
