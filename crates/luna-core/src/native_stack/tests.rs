//! The reserves cover the deepest stack the code uses between two checks.
//! The stack below the caller is painted first; a run that nests natives
//! and Lua calls, raises, and collects in its message handler then writes
//! over part of it, and the lowest byte it wrote is how deep it went. From
//! the deepest check the run made down to there has to fit in the smaller
//! reserve.

use super::*;
use crate::runtime::Value;
use crate::version::LuaVersion;
use crate::vm::exec::Vm;

thread_local! {
    /// the stack pointer of the deepest check this thread made
    static DEEPEST_CHECK: Cell<usize> = const { Cell::new(usize::MAX) };
}

/// A check of the stack left, at `sp`.
pub(super) fn checked_at(sp: usize) {
    DEEPEST_CHECK.with(|c| c.set(c.get().min(sp)));
}

const PAINT: u8 = 0x5A;
const PAINTED: usize = 2 << 20;

/// Library functions calling back into Lua (`xpcall`, `table.sort`'s
/// comparator, `string.gsub`'s replacement), an error with a table of
/// tables, and a message handler that collects them.
const NESTED: &str = r#"
local t = {}
for i = 1, 200 do t[i] = {i, {i}} end
local function h(m) collectgarbage() collectgarbage("step") return m end
return xpcall(function()
  return string.gsub("ab", "%w", function()
    table.sort({3, 2, 1}, function() error(t) end)
  end)
end, h)
"#;

/// The bytes below the caller's frame, set to [`PAINT`]; their range.
#[inline(never)]
fn paint() -> (usize, usize) {
    let mut buf = [PAINT; PAINTED];
    std::hint::black_box(&mut buf);
    let lo = buf.as_ptr() as usize;
    (lo, lo + PAINTED)
}

/// The lowest address in `lo..hi` no longer [`PAINT`].
#[inline(never)]
fn lowest_written(lo: usize, hi: usize) -> usize {
    (lo..hi)
        // SAFETY: the range is the painted part of this thread's stack, below
        // every live frame now and mapped while the thread runs; it is only read
        .find(|&a| unsafe { std::ptr::read_volatile(a as *const u8) } != PAINT)
        .unwrap_or(hi)
}

#[test]
#[cfg_attr(
    miri,
    ignore = "reads the real native stack, which Miri does not model"
)]
fn the_reserves_cover_the_deepest_nesting_between_two_checks() {
    let deepest = std::thread::Builder::new()
        .stack_size(16 << 20)
        .spawn(|| {
            let mut worst = 0;
            for v in [
                LuaVersion::Lua51,
                LuaVersion::Lua52,
                LuaVersion::Lua53,
                LuaVersion::Lua54,
                LuaVersion::Lua55,
            ] {
                let mut vm = Vm::new(v);
                let f = vm
                    .load(NESTED.as_bytes(), b"=nested")
                    .expect("the chunk loads");
                let (lo, hi) = paint();
                DEEPEST_CHECK.with(|c| c.set(usize::MAX));
                vm.call_value(Value::Closure(f), &[])
                    .expect("the chunk runs");
                let check = DEEPEST_CHECK.with(Cell::get);
                assert!(check < hi, "{v:?}: no check in the run");
                worst = worst.max(check - lowest_written(lo, hi));
            }
            worst
        })
        .expect("a thread")
        .join()
        .expect("the run");
    assert!(
        deepest < HANDLER_RESERVE,
        "{deepest} bytes deep, the reserve in a message handler is {HANDLER_RESERVE}"
    );
}
