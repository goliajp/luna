//! The size of a thread's Lua stack as PUC keeps it (`L->stacksize`, from
//! 5.4 `stack_last - stack`): grown when a check finds too little room
//! above the top (`luaD_growstack`), shrunk when a protected call catches
//! an error and when the collector visits the thread (`luaD_shrinkstack`,
//! 5.1 `checkstacksizes`). luna's own stack grows as it needs; what the
//! size decides is visible to scripts: where the stack overflows, how much
//! room a message handler has after it did, and what `lua_checkstack`
//! grants.
//!
//! Slots are counted as PUC counts them, from the bottom of its stack,
//! whose slot 0 holds no function: luna's slot `s` is PUC's `s + 1`.

use crate::version::LuaVersion;

/// PUC `LUAI_MAXSTACK`.
pub(crate) const MAX_STACK: u32 = 1_000_000;
/// PUC `ERRORSTACKSIZE`: the size an overflow leaves, the extra room for
/// the message handler.
pub(crate) const ERROR_STACK: u32 = MAX_STACK + 200;
/// PUC `EXTRA_STACK`.
const EXTRA: u32 = 5;
/// PUC `BASIC_STACK_SIZE` (`2 * LUA_MINSTACK`).
const BASIC: u32 = 40;
/// PUC `LUA_MINSTACK`.
pub(crate) const MIN_STACK: u32 = 20;
/// PUC 5.1 `LUAI_MAXCSTACK`: what `lua_checkstack` grants a C function.
pub(crate) const MAX_C_STACK_51: u32 = 8000;

/// What a check found (PUC `luaD_growstack`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Grown {
    /// the stack has (or now has) the room, at this size
    Fits(u32),
    /// past the limit: "stack overflow", the stack now this size (the
    /// handler's room)
    Overflow(u32),
    /// past the room an overflow left: "error in error handling"
    ErrErr,
}

/// The size a new thread's stack has (`stack_init`).
pub(crate) fn initial(v: LuaVersion) -> u32 {
    match v {
        LuaVersion::Lua51 => BASIC + EXTRA,
        _ => BASIC,
    }
}

/// `stack_last - stack` of a stack of size `s`: a check of `n` slots at
/// top `t` grows the stack when `t + n >= last(s)`.
#[inline]
pub(crate) fn last(v: LuaVersion, s: u32) -> u32 {
    match v {
        LuaVersion::Lua51 => s - EXTRA - 1,
        LuaVersion::Lua52 | LuaVersion::Lua53 => s - EXTRA,
        _ => s,
    }
}

/// PUC `luaD_growstack`: a stack of size `s`, top `t`, asked for `n` more
/// slots that it does not have.
pub(crate) fn grow(v: LuaVersion, s: u32, t: u32, n: u32) -> Grown {
    match v {
        LuaVersion::Lua51 => {
            // `luaD_reallocstack(L, newsize)` makes it `newsize + 1 + EXTRA`
            let newsize = if n <= s { 2 * s } else { s + n };
            Grown::Fits(newsize + 1 + EXTRA)
        }
        LuaVersion::Lua52 | LuaVersion::Lua53 => {
            if s > MAX_STACK {
                return Grown::ErrErr;
            }
            let needed = t + n + EXTRA;
            let newsize = (2 * s).min(MAX_STACK).max(needed);
            if newsize > MAX_STACK {
                Grown::Overflow(ERROR_STACK)
            } else {
                Grown::Fits(newsize)
            }
        }
        _ => {
            if s > MAX_STACK {
                return Grown::ErrErr;
            }
            if n < MAX_STACK {
                let grown = if v == LuaVersion::Lua54 {
                    2 * s
                } else {
                    s + s / 2
                };
                let newsize = grown.min(MAX_STACK).max(t + n);
                if newsize <= MAX_STACK {
                    return Grown::Fits(newsize);
                }
            }
            Grown::Overflow(ERROR_STACK)
        }
    }
}

/// PUC `luaD_checkstack`: `n` slots above top `t` of a stack of size `s`.
#[inline]
pub(crate) fn check(v: LuaVersion, s: u32, t: u32, n: u32) -> Grown {
    if t + n < last(v, s) {
        return Grown::Fits(s);
    }
    grow(v, s, t, n)
}

/// PUC `lua_checkstack(L, n)` with the top at `t` (5.1: `t - base` slots
/// above the C function's base): whether it grants the room, and the size
/// the stack has after.
pub(crate) fn api_check(v: LuaVersion, s: u32, t: u32, base: u32, n: u32) -> (bool, u32) {
    match v {
        LuaVersion::Lua51 => {
            if n > MAX_C_STACK_51 || t - base + n > MAX_C_STACK_51 {
                return (false, s);
            }
            match check(v, s, t, n) {
                Grown::Fits(s) => (true, s),
                _ => unreachable!("5.1 stacks grow without a limit"),
            }
        }
        LuaVersion::Lua52 | LuaVersion::Lua53 => {
            if last(v, s) > t + n {
                return (true, s);
            }
            // a request the limit refuses fails without growing
            if t + EXTRA > MAX_STACK.saturating_sub(n) {
                return (false, s);
            }
            match grow(v, s, t, n) {
                Grown::Fits(s) => (true, s),
                _ => unreachable!("the room was checked"),
            }
        }
        _ => {
            if last(v, s) > t + n {
                return (true, s);
            }
            match grow(v, s, t, n) {
                Grown::Fits(s) => (true, s),
                // refused, the stack left at the handler's size
                Grown::Overflow(s) => (false, s),
                Grown::ErrErr => (false, s),
            }
        }
    }
}

/// PUC `luaD_shrinkstack` of a stack of size `s` whose frames and top
/// reach slot `inuse` (`stackinuse`, without the floor 5.4 puts on it).
/// 5.1 shrinks only in the collector ([`halve_51`]).
pub(crate) fn shrink(v: LuaVersion, s: u32, inuse: u32) -> u32 {
    match v {
        LuaVersion::Lua51 => s,
        LuaVersion::Lua52 => {
            let good = (inuse + inuse / 8 + 2 * EXTRA).min(MAX_STACK);
            if inuse > MAX_STACK || good >= s {
                s
            } else {
                good
            }
        }
        LuaVersion::Lua53 => {
            let good = (inuse + inuse / 8 + 2 * EXTRA).min(MAX_STACK);
            if inuse <= MAX_STACK - EXTRA && good < s {
                good
            } else {
                s
            }
        }
        _ => {
            let inuse = inuse.max(MIN_STACK);
            let max = if inuse > MAX_STACK / 3 {
                MAX_STACK
            } else {
                inuse * 3
            };
            if inuse <= MAX_STACK && s > max {
                if inuse > MAX_STACK / 2 {
                    MAX_STACK
                } else {
                    inuse * 2
                }
            } else {
                s
            }
        }
    }
}

/// PUC 5.1 `checkstacksizes` for the stack: halved when its frames and top
/// reach less than a quarter of it (`s_used` slots) and it is more than
/// twice the basic size.
pub(crate) fn halve_51(s: u32, s_used: u32) -> u32 {
    if 4 * s_used < s && 2 * (BASIC + EXTRA) < s {
        // `luaD_reallocstack(L, L->stacksize / 2)`
        s / 2 + 1 + EXTRA
    } else {
        s
    }
}

/// PUC 5.1 `checkstacksizes` for the `CallInfo` array of `size` entries,
/// `used` of them in use: halved when less than a quarter is used and it
/// is more than twice the basic size, and left alone while it is past the
/// call limit (an overflow is being handled).
pub(crate) fn halve_calls_51(size: u32, used: u32, max_calls: u32, basic: u32) -> u32 {
    if size > max_calls {
        return size;
    }
    if 4 * used < size && 2 * basic < size {
        size / 2
    } else {
        size
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use LuaVersion::*;

    #[test]
    fn a_stack_doubles_until_the_limit_then_overflows() {
        // 5.3: 40, 80, ... capped at the limit; past it the handler's size
        let mut s = initial(Lua53);
        let mut t = 1;
        while let Grown::Fits(n) = check(Lua53, s, t, 20) {
            s = n;
            t += 1000;
        }
        assert_eq!(s, MAX_STACK);
        assert_eq!(check(Lua53, s, MAX_STACK - 25, 20), Grown::Fits(MAX_STACK));
        assert_eq!(
            check(Lua53, s, MAX_STACK - 24, 20),
            Grown::Overflow(ERROR_STACK)
        );
        assert_eq!(
            check(Lua53, ERROR_STACK, MAX_STACK + 175, 20),
            Grown::ErrErr
        );
        assert_eq!(
            check(Lua53, ERROR_STACK, MAX_STACK + 174, 20),
            Grown::Fits(ERROR_STACK)
        );
    }

    #[test]
    fn five_five_grows_by_half() {
        assert_eq!(check(Lua55, 40, 30, 20), Grown::Fits(60));
        assert_eq!(check(Lua54, 40, 30, 20), Grown::Fits(80));
        // a request larger than the growth takes what it needs
        assert_eq!(check(Lua55, 40, 30, 100), Grown::Fits(130));
        assert_eq!(
            check(Lua55, MAX_STACK, MAX_STACK - 10, 10),
            Grown::Fits(MAX_STACK)
        );
        assert_eq!(
            check(Lua55, MAX_STACK, MAX_STACK - 10, 11),
            Grown::Overflow(ERROR_STACK)
        );
    }

    #[test]
    fn checkstack_refuses_and_five_four_grows_to_the_handler_size() {
        assert_eq!(
            api_check(Lua53, MAX_STACK, MAX_STACK - 10, 0, 6),
            (false, MAX_STACK)
        );
        assert_eq!(
            api_check(Lua54, MAX_STACK, MAX_STACK - 10, 0, 11),
            (false, ERROR_STACK)
        );
        // in the handler's room, what it has is granted
        assert_eq!(
            api_check(Lua53, ERROR_STACK, MAX_STACK + 100, 0, 94),
            (true, ERROR_STACK)
        );
        assert_eq!(
            api_check(Lua53, ERROR_STACK, MAX_STACK + 100, 0, 95),
            (false, ERROR_STACK)
        );
        assert_eq!(api_check(Lua51, 45, 10, 5, 7996), (false, 45));
    }

    #[test]
    fn shrinking_leaves_the_handler_room_once_the_frames_fit() {
        assert_eq!(shrink(Lua53, ERROR_STACK, MAX_STACK - 5), MAX_STACK);
        assert_eq!(shrink(Lua53, ERROR_STACK, MAX_STACK - 4), ERROR_STACK);
        assert_eq!(shrink(Lua54, ERROR_STACK, MAX_STACK), MAX_STACK);
        assert_eq!(shrink(Lua54, 4000, 100), 200);
        assert_eq!(shrink(Lua52, 4000, 100), 122);
        assert_eq!(halve_51(1000, 100), 506);
        assert_eq!(halve_calls_51(16384, 100, 20000, 8), 8192);
        assert_eq!(halve_calls_51(20000, 100, 20000, 8), 10000);
        assert_eq!(halve_calls_51(40000, 100, 20000, 8), 40000);
    }
}
