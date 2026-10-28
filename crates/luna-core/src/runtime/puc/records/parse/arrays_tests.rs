//! The parser's arrays grow as PUC's `luaM_growaux_` grows them: doubled,
//! then straight to their limit once doubling would pass half of it. The
//! bytes are those of r38 PUC's allocation traces on Linux, 64-bit and
//! 32-bit (where the entries hold half-size pointers), where a function's
//! array first grows past half its limit.

use super::{GrowVec, ParserArray};
use crate::version::LuaVersion;
use crate::vm::Vm;

/// The 64-bit bytes, or the 32-bit ones.
fn by_width(w64: (usize, usize), w32: (usize, usize)) -> (usize, usize) {
    if cfg!(target_pointer_width = "64") {
        w64
    } else {
        w32
    }
}

/// The last growth of array `a` as `n` elements go in: its bytes before
/// and after.
fn last_growth(v: LuaVersion, a: ParserArray, n: usize) -> (usize, usize) {
    let mut vm = Vm::new_minimal(v);
    let heap = &mut vm.heap;
    let mut g = GrowVec::default();
    let mut last = (0, 0);
    for _ in 0..n {
        let before = g.cap;
        heap.parser_push(&mut g, a);
        if g.cap != before {
            last = (heap.parser_bytes(a, before), heap.parser_bytes(a, g.cap));
        }
    }
    heap.parser_free(g, a);
    last
}

#[test]
fn upvalues_grow_to_their_limit_of_255() {
    for v in [
        LuaVersion::Lua52,
        LuaVersion::Lua53,
        LuaVersion::Lua54,
        LuaVersion::Lua55,
    ] {
        assert_eq!(
            last_growth(v, ParserArray::Upvals, 150),
            by_width((2048, 4080), (1024, 2040)),
            "{v:?}"
        );
    }
    // 5.1 counts its upvalues against a cap of its own: the array doubles
    assert_eq!(
        last_growth(LuaVersion::Lua51, ParserArray::Upvals, 60),
        by_width((256, 512), (128, 256))
    );
}

#[test]
fn local_variable_records_grow_to_their_limit_of_32767() {
    for v in [
        LuaVersion::Lua52,
        LuaVersion::Lua53,
        LuaVersion::Lua54,
        LuaVersion::Lua55,
    ] {
        assert_eq!(
            last_growth(v, ParserArray::LocVars, 16400),
            by_width((262144, 524272), (196608, 393204)),
            "{v:?}"
        );
    }
}

#[test]
fn prototypes_grow_to_their_limit_of_131071_from_5_4() {
    for v in [LuaVersion::Lua54, LuaVersion::Lua55] {
        assert_eq!(
            last_growth(v, ParserArray::Protos, 65600),
            by_width((524288, 1048568), (262144, 524284)),
            "{v:?}"
        );
    }
    // MAXARG_Bx is 2^18 - 1 before: 65600 prototypes only double
    assert_eq!(
        last_growth(LuaVersion::Lua53, ParserArray::Protos, 65600),
        by_width((524288, 1048576), (262144, 524288))
    );
}
