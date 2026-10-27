//! A load as the model sees it: its span, the objects built ahead of PUC,
//! its message, and the readers it calls.

use super::super::*;
use crate::runtime::heap::Heap;
use crate::runtime::puc::load_session::{Building, LoadSpan, LoadUnwind};
use crate::runtime::puc::loads::{LoadsHeld, ParseOpen};
use crate::runtime::puc::records::parse::ParseFrame;

/// A load as the model sees it: its span, the objects built ahead of PUC,
/// its message, and the readers it calls.
pub(crate) trait PucLoads {
    /// luna builds a load's objects now, before PUC's parser makes them:
    /// nothing is counted, and with `aside` (a counted load) the objects
    /// are set aside until the replay reaches where PUC makes each.
    fn build_load(&mut self, aside: bool) -> Building;
    fn close_parse_frame(&mut self, _a1: ParseOpen);
    /// A load that must not collect starts, until the token is handed to
    /// [`Heap::release_loads`].
    fn hold_loads(&mut self) -> LoadsHeld;
    /// A load begins.
    fn load_begins(&self) -> LoadSpan;
    /// The building `b` started is done: counting as it was.
    fn load_built(&mut self, b: Building);
    /// Whether the failed load's replay shrank the stack where
    /// `luaD_pcall` caught its error, now `on`.
    fn load_caught(&mut self, on: bool) -> bool;
    /// The load of `s` ends: what it set aside that PUC did not make (luna's
    /// own, or the garbage of a load that failed) goes on the list.
    fn load_ends(&mut self, s: LoadSpan);
    /// The message [`Heap::set_load_msg`] left, if any.
    fn load_msg(&self) -> Option<Gc<LuaStr>>;
    /// How many stack slots the loads' parser objects take (see
    /// [`Heap::mark_load_slots`]).
    fn load_slots(&self) -> u32;
    /// Where loads stand now, for [`Heap::unwind_load`].
    fn load_unwind_point(&self) -> LoadUnwind;
    /// A load that must not collect is running.
    fn loads_held(&self) -> bool;
    /// Mark the message [`Heap::set_load_msg`] left.
    fn mark_load_msg(&self, m: &mut Marker);
    /// Mark the parser objects of the loads a collection step runs in the
    /// middle of: above their C functions' slots on PUC's stack.
    fn mark_load_slots(&self, m: &mut Marker);
    /// Mark the strings [`Heap::pin`] keeps.
    fn mark_pins(&self, m: &mut Marker);
    /// `frame` is the innermost load's view while a collection step runs
    /// in it, until the token goes back to [`Heap::close_parse_frame`].
    fn open_parse_frame(&mut self, frame: ParseFrame) -> ParseOpen;
    /// 5.1's io library opening: its environments' records are on the
    /// stack (see `vm/opening/fenv51.rs`).
    fn phantoms(&self) -> bool;
    /// `s`, a string a library of PUC's keeps and luna's may not, kept
    /// alive (once, however often pinned).
    fn pin(&mut self, s: Gc<LuaStr>);
    /// The load [`Heap::hold_loads`] started is over.
    fn release_loads(&mut self, held: LoadsHeld);
    /// The load about to run was checked for a collection already (or no
    /// more, `false`).
    fn set_load_checked(&mut self, on: bool);
    /// The message of the load that just failed, made as PUC makes it.
    fn set_load_msg(&mut self, m: Gc<LuaStr>);
    fn set_phantoms(&mut self, on: bool);
    /// Whether the load about to run was checked already; the mark taken.
    fn take_load_checked(&mut self) -> bool;
    /// The message [`Heap::set_load_msg`] left, if any, taken.
    fn take_load_msg(&mut self) -> Option<Gc<LuaStr>>;
    /// A memory error unwound a load past its tokens: counting, the parser
    /// frames and the setting aside back to `u`, and what it set aside on
    /// the list.
    fn unwind_load(&mut self, u: LoadUnwind);
}

impl PucLoads for Heap {
    #[inline(always)]
    fn build_load(&mut self, aside: bool) -> Building {
        Heap::build_load(self, aside)
    }

    #[inline(always)]
    fn close_parse_frame(&mut self, _a1: ParseOpen) {
        Heap::close_parse_frame(self, _a1)
    }

    #[inline(always)]
    fn hold_loads(&mut self) -> LoadsHeld {
        Heap::hold_loads(self)
    }

    #[inline(always)]
    fn load_begins(&self) -> LoadSpan {
        Heap::load_begins(self)
    }

    #[inline(always)]
    fn load_built(&mut self, b: Building) {
        Heap::load_built(self, b)
    }

    #[inline(always)]
    fn load_caught(&mut self, on: bool) -> bool {
        Heap::load_caught(self, on)
    }

    #[inline(always)]
    fn load_ends(&mut self, s: LoadSpan) {
        Heap::load_ends(self, s)
    }

    #[inline(always)]
    fn load_msg(&self) -> Option<Gc<LuaStr>> {
        Heap::load_msg(self)
    }

    #[inline(always)]
    fn load_slots(&self) -> u32 {
        Heap::load_slots(self)
    }

    #[inline(always)]
    fn load_unwind_point(&self) -> LoadUnwind {
        Heap::load_unwind_point(self)
    }

    #[inline(always)]
    fn loads_held(&self) -> bool {
        Heap::loads_held(self)
    }

    #[inline(always)]
    fn mark_load_msg(&self, m: &mut Marker) {
        Heap::mark_load_msg(self, m)
    }

    #[inline(always)]
    fn mark_load_slots(&self, m: &mut Marker) {
        Heap::mark_load_slots(self, m)
    }

    #[inline(always)]
    fn mark_pins(&self, m: &mut Marker) {
        Heap::mark_pins(self, m)
    }

    #[inline(always)]
    fn open_parse_frame(&mut self, frame: ParseFrame) -> ParseOpen {
        Heap::open_parse_frame(self, frame)
    }

    #[inline(always)]
    fn phantoms(&self) -> bool {
        Heap::phantoms(self)
    }

    #[inline(always)]
    fn pin(&mut self, s: Gc<LuaStr>) {
        Heap::pin(self, s)
    }

    #[inline(always)]
    fn release_loads(&mut self, held: LoadsHeld) {
        Heap::release_loads(self, held)
    }

    #[inline(always)]
    fn set_load_checked(&mut self, on: bool) {
        Heap::set_load_checked(self, on)
    }

    #[inline(always)]
    fn set_load_msg(&mut self, m: Gc<LuaStr>) {
        Heap::set_load_msg(self, m)
    }

    #[inline(always)]
    fn set_phantoms(&mut self, on: bool) {
        Heap::set_phantoms(self, on)
    }

    #[inline(always)]
    fn take_load_checked(&mut self) -> bool {
        Heap::take_load_checked(self)
    }

    #[inline(always)]
    fn take_load_msg(&mut self) -> Option<Gc<LuaStr>> {
        Heap::take_load_msg(self)
    }

    #[inline(always)]
    fn unwind_load(&mut self, u: LoadUnwind) {
        Heap::unwind_load(self, u)
    }
}
