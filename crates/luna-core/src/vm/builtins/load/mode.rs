//! `lua_load` failing before its parser's work is replayed: a mode that
//! refuses the chunk, an error from the reader.

use super::*;
use crate::runtime::puc::fmt::FmtArg;

/// What `lua_load` counted before its parser met an error, when no replay
/// counts it: 5.5's anchor table and the reader's calls so far.
pub(super) fn before_error(
    vm: &mut Vm,
    splices: &mut Vec<(u32, Deferred)>,
) -> Result<(), LuaError> {
    vm.parser_anchor_55()?;
    for (_, d) in splices.drain(..) {
        vm.heap.replay_reader_call(d);
    }
    Ok(())
}

/// PUC `checkmode`: the message `load` returns when `mode` does not allow
/// a binary or text chunk, after what the load counted before it (see
/// [`before_error`]).
pub(super) fn mode_refusal(
    vm: &mut Vm,
    mode: Option<&[u8]>,
    binary: bool,
    splices: &mut Vec<(u32, Deferred)>,
) -> Result<Option<Value>, LuaError> {
    // `strchr`: the mode is a C string, so it ends at the first NUL.
    let Some(mode) = mode.map(|m| &m[..m.iter().position(|&c| c == 0).unwrap_or(m.len())]) else {
        return Ok(None);
    };
    let kind = if binary { b'b' } else { b't' };
    if mode.contains(&kind) {
        return Ok(None);
    }
    before_error(vm, splices)?;
    let what: &[u8] = if binary { b"binary" } else { b"text" };
    let msg = vm.pushfstring(
        b"attempt to load a %s chunk (mode is '%s')",
        &[FmtArg::S(what), FmtArg::S(mode)],
    )?;
    Ok(Some(Value::Str(msg)))
}
