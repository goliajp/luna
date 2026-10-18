//! `generic_reader`: calling a `load` reader where PUC's C function does.

use super::*;
use crate::vm::dump::Undumping;
use crate::vm::exec::reader_layers::UndumpShown;

/// The stack of `load` while it reads from a reader function: the slots
/// `luaB_load` keeps (5.1 three, 5.2 to 5.4 five, 5.5 its arguments), the
/// one `generic_reader` keeps the last piece in (5.1 the third, 5.2 to
/// 5.4 the fifth; 5.5 leaves it on top and pops it before the next call),
/// then what the parser holds there: per open function a constants table
/// and a prototype (5.1), the main closure and per open function a
/// constants table (5.2), the main closure and the scanner table (5.3,
/// 5.4), the anchor table (5.5).
pub(super) struct ReaderSlot {
    fs: u32,
    /// the slots below the parser's
    base: u32,
    /// 5.1 leaves a nil a reader returned on the stack
    nils: u32,
}

impl ReaderSlot {
    /// `luaB_load`'s `lua_settop` before it loads from a reader.
    pub(super) fn new(vm: &mut Vm, a: Args) -> Result<Self, LuaError> {
        let base = match vm.version() {
            LuaVersion::Lua51 => 3,
            LuaVersion::Lua55 => a.n,
            _ => 5,
        };
        vm.native_settop_nil(base)?;
        Ok(ReaderSlot {
            fs: a.fs,
            base,
            nils: 0,
        })
    }

    /// The slot the last piece is kept in.
    fn reserved(vm: &Vm) -> Option<u32> {
        match vm.version() {
            LuaVersion::Lua51 => Some(3),
            LuaVersion::Lua55 => None,
            _ => Some(5),
        }
    }

    /// What the parser (or the undumper) holds on the stack at `phase`.
    fn parser_slots(vm: &Vm, phase: Phase) -> u32 {
        match (vm.version(), phase) {
            // the anchor table goes on before the first read
            (LuaVersion::Lua55, _) => 1,
            (_, Phase::Start) => 0,
            (LuaVersion::Lua51, Phase::Text(n, _)) => 2 * n,
            (LuaVersion::Lua52, Phase::Text(n, _)) => 1 + n,
            (_, Phase::Text(..)) => 2,
            (v, Phase::Binary(_, u, _)) => binary_slots(v, u),
        }
    }
}

/// Where a load reading from a reader is: before the chunk is known to be
/// text or binary, parsing text with this many functions open (the source
/// read so far), or undumping a binary chunk (the bytes read so far, and
/// how far the undumper has got with them).
#[derive(Clone, Copy)]
pub(super) enum Phase<'a> {
    Start,
    Text(u32, &'a [u8]),
    Binary(&'a [u8], &'a Undumping, *mut UndumpShown),
}

/// What PUC's undumper holds on the stack at progress `p`: 5.1 the
/// prototype of each function it is reading, 5.2 on the main closure once
/// the header is read, and 5.3 and 5.4 a long string while its bytes are.
pub(super) fn binary_slots(v: LuaVersion, u: &Undumping) -> u32 {
    match v {
        LuaVersion::Lua51 => u.open.len() as u32,
        _ => u32::from(u.closure.is_some()) + u32::from(u.long.is_some()),
    }
}

/// The next piece of a `load` reader: `None` when it returns nil, no
/// value or the empty string; a string or number piece is its bytes. Any
/// other value, or an error the reader raises, fails the load softly with
/// the message `load` returns after its nil. The reader runs with the
/// thread non-yieldable, as under PUC's `lua_load`, from where
/// `generic_reader` calls it, and what it returns is kept where that
/// function keeps it.
pub(super) fn next_piece(
    vm: &mut Vm,
    reader: Value,
    rs: &mut ReaderSlot,
    phase: Phase,
    name: &[u8],
) -> Result<Option<Vec<u8>>, Value> {
    let reserved = ReaderSlot::reserved(vm);
    let held = rs.base + rs.nils;
    let parser = ReaderSlot::parser_slots(vm, phase);
    // 5.5's last result is popped here
    vm.native_settop_nil(held + parser).map_err(|e| e.0)?;
    let top = vm.native_top().unwrap_or(vm.top);
    argcheck::check_stack_at(vm, top, 2, "too many nested functions").map_err(|e| e.0)?;
    // where the debug library finds the parser's objects, made if it looks
    let (src, open, undumping, shown) = match phase {
        Phase::Start => (&[][..], None, std::ptr::null(), std::ptr::null_mut()),
        Phase::Text(n, src) => (src, Some(n), std::ptr::null(), std::ptr::null_mut()),
        Phase::Binary(src, u, shown) => (src, None, u as *const Undumping, shown),
    };
    if parser > 0 {
        vm.reader_layers
            .push(crate::vm::exec::reader_layers::ReaderLayer {
                thread: vm.current,
                at: rs.fs + 1 + held,
                n: parser,
                src: (src.as_ptr(), src.len()),
                name: (name.as_ptr(), name.len()),
                open,
                undumping,
                shown,
                filled: false,
            });
    }
    let r = vm.call_value(reader, &[]);
    if parser > 0 {
        vm.drop_reader_layer();
    }
    let r = r.map_err(|e| e.0)?;
    let v = r.first().copied().unwrap_or(Value::Nil);
    let bytes = match v {
        Value::Nil => None,
        v => match argcheck::to_str_bytes(vm, v) {
            Some(b) => Some(b),
            None => {
                return Err(vm
                    .str_value(b"reader function must return a string")
                    .unwrap_or_else(|e| e.0));
            }
        },
    };
    // `lua_tolstring` turns a number into a string where it is kept
    let kept = match v {
        Value::Int(_) | Value::Float(_) => Value::Str(vm.number_string(v).map_err(|e| e.0)?),
        v => v,
    };
    let held = match (reserved, v) {
        // 5.2 to 5.4 pop a nil, 5.1 leaves it
        (Some(_), Value::Nil) if vm.version() >= LuaVersion::Lua52 => Ok(()),
        (Some(_), Value::Nil) => {
            if matches!(phase, Phase::Start) {
                rs.nils += 1;
            }
            vm.native_push_vals(&[Value::Nil])
        }
        (Some(slot), _) => {
            vm.stack[(rs.fs + slot) as usize] = kept;
            Ok(())
        }
        (None, _) => vm.native_push_vals(&[kept]),
    };
    held.map_err(|e| e.0)?;
    Ok(bytes.filter(|b| !b.is_empty()))
}
