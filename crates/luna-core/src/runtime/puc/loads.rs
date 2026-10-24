//! What a load in progress keeps in the model: the parser's frames a
//! collection step marks, the message of a load that failed, whether the
//! load may collect and whether its check was made already.

use super::*;
use crate::runtime::puc::records::parse::ParseFrame;

/// How many parser frames were open at some point, to return to with
/// [`Heap::restore_parse_frames`].
#[must_use]
pub(in crate::runtime) struct ParseMark(usize);

/// A parser frame open for a collection step (see [`Heap::open_parse_frame`]).
#[must_use = "the frame stays open until the token goes back to `Heap::close_parse_frame`"]
pub(crate) struct ParseOpen(());

/// A load that must not collect, running (see [`Heap::hold_loads`]).
#[must_use = "loads stay held until the token goes back to `Heap::release_loads`"]
pub(crate) struct LoadsHeld(bool);

impl Heap {
    /// Mark the parser objects of the loads a collection step runs in the
    /// middle of: above their C functions' slots on PUC's stack.
    pub(crate) fn mark_load_slots(&self, m: &mut Marker) {
        let frames = std::mem::take(&mut m.lent.parse);
        for f in &frames {
            for &h in &f.slots {
                // SAFETY: a parser object is allocated while its load runs
                unsafe { m.header(h) };
            }
        }
        m.lent.parse = frames;
    }

    /// How many stack slots the loads' parser objects take (see
    /// [`Heap::mark_load_slots`]).
    pub(crate) fn load_slots(&self) -> u32 {
        self.puc.parse.iter().map(|f| f.slots.len() as u32).sum()
    }

    /// `frame` is the innermost load's view while a collection step runs
    /// in it, until the token goes back to [`Heap::close_parse_frame`].
    #[inline]
    pub(crate) fn open_parse_frame(&mut self, frame: ParseFrame) -> ParseOpen {
        self.puc.parse.push(frame);
        ParseOpen(())
    }

    #[inline]
    pub(crate) fn close_parse_frame(&mut self, _: ParseOpen) {
        self.puc.parse.pop();
    }

    /// Where the parser frames stand now.
    #[inline]
    pub(in crate::runtime::puc) fn parse_mark(&self) -> ParseMark {
        ParseMark(self.puc.parse.len())
    }

    /// Back to the frames at `m` (a load unwound by running out of memory).
    #[inline]
    pub(in crate::runtime::puc) fn restore_parse_frames(&mut self, m: ParseMark) {
        self.puc.parse.truncate(m.0);
    }

    /// The message of the load that just failed, made as PUC makes it.
    #[inline]
    pub(crate) fn set_load_msg(&mut self, m: Gc<LuaStr>) {
        self.puc.load_msg = Some(m);
    }

    /// The message [`Heap::set_load_msg`] left, if any, taken.
    #[inline]
    pub(crate) fn take_load_msg(&mut self) -> Option<Gc<LuaStr>> {
        self.puc.load_msg.take()
    }

    /// The message [`Heap::set_load_msg`] left, if any.
    #[inline]
    pub(crate) fn load_msg(&self) -> Option<Gc<LuaStr>> {
        self.puc.load_msg
    }

    /// The load about to run was checked for a collection already (or no
    /// more, `false`).
    #[inline]
    pub(crate) fn set_load_checked(&mut self, on: bool) {
        self.puc.load_checked = on;
    }

    /// Whether the load about to run was checked already; the mark taken.
    #[inline]
    pub(crate) fn take_load_checked(&mut self) -> bool {
        std::mem::take(&mut self.puc.load_checked)
    }

    /// A load that must not collect is running.
    #[inline]
    pub(crate) fn loads_held(&self) -> bool {
        self.puc.load_holds
    }

    /// A load that must not collect starts, until the token is handed to
    /// [`Heap::release_loads`].
    #[inline]
    pub(crate) fn hold_loads(&mut self) -> LoadsHeld {
        LoadsHeld(std::mem::replace(&mut self.puc.load_holds, true))
    }

    /// The load [`Heap::hold_loads`] started is over.
    #[inline]
    pub(crate) fn release_loads(&mut self, held: LoadsHeld) {
        self.puc.load_holds = held.0;
    }

    /// 5.1's io library opening: its environments' records are on the
    /// stack (see `vm/opening/fenv51.rs`).
    #[inline]
    pub(crate) fn phantoms(&self) -> bool {
        self.puc.phantoms
    }

    #[inline]
    pub(crate) fn set_phantoms(&mut self, on: bool) {
        self.puc.phantoms = on;
    }

    /// `s`, a string a library of PUC's keeps and luna's may not, kept
    /// alive (once, however often pinned).
    #[inline]
    pub(crate) fn pin(&mut self, s: Gc<LuaStr>) {
        if !self.puc.pins.iter().any(|p| p.as_ptr() == s.as_ptr()) {
            self.puc.pins.push(s);
        }
    }

    /// Mark the strings [`Heap::pin`] keeps.
    #[inline]
    pub(crate) fn mark_pins(&self, m: &mut Marker) {
        m.mark_strings(self.puc.pins.iter());
    }

    /// Mark the message [`Heap::set_load_msg`] left.
    #[inline]
    pub(crate) fn mark_load_msg(&self, m: &mut Marker) {
        m.mark_strings(self.puc.load_msg.iter());
    }

    /// The longest string PUC keeps in its string table (5.1: all).
    #[inline]
    pub(crate) fn short_max(&self) -> usize {
        self.puc.short_max
    }
}
