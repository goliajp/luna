//! The loads under way as the model sees them: the parser frames a
//! collection step runs in the middle of, the message of the last one that
//! failed, and the flags a load's run leaves for the next.

use super::super::*;
use crate::runtime::puc::Lent;
use crate::runtime::puc::records::parse::ParseFrame;

/// The loads under way, as the model sees them.
pub(in crate::runtime::puc) struct Loads {
    /// the message of the last load that failed, made as PUC makes it,
    /// until the load's caller takes it
    load_msg: Option<Gc<crate::runtime::LuaStr>>,
    /// the load about to run was checked for a collection already (5.4+
    /// `lua_load`, before its reader read)
    load_checked: bool,
    /// the failed load's replay shrank the stack where `luaD_pcall` caught
    /// its error (`Heap::load_caught`)
    load_caught: bool,
    /// a load that must not collect is running (`Vm::load`)
    load_holds: bool,
    /// the loads a collection step runs in the middle of
    parse: Vec<ParseFrame>,
}

impl Loads {
    /// No load under way.
    pub(in crate::runtime::puc) fn new() -> Loads {
        Loads {
            load_msg: None,
            load_checked: false,
            load_caught: false,
            load_holds: false,
            parse: Vec::new(),
        }
    }
}

/// How many parser frames were open at some point, to return to with
/// [`Heap::restore_parse_frames`].
#[must_use]
pub(in crate::runtime::puc) struct ParseMark(usize);

/// A parser frame open for a collection step (see [`Heap::open_parse_frame`]).
#[must_use = "the frame stays open until the token goes back to `Heap::close_parse_frame`"]
pub(crate) struct ParseOpen(());

/// A load that must not collect, running (see [`Heap::hold_loads`]).
#[must_use = "loads stay held until the token goes back to `Heap::release_loads`"]
pub(crate) struct LoadsHeld(bool);

impl Heap {
    /// Where the parser frames stand now.
    #[inline]
    pub(in crate::runtime::puc) fn parse_mark(&self) -> ParseMark {
        ParseMark(self.puc.loads.parse.len())
    }

    /// Back to the frames at `m` (a load unwound by running out of memory).
    #[inline]
    pub(in crate::runtime::puc) fn restore_parse_frames(&mut self, m: ParseMark) {
        self.puc.loads.parse.truncate(m.0);
    }
}

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
        self.puc
            .loads
            .parse
            .iter()
            .map(|f| f.slots.len() as u32)
            .sum()
    }

    /// `frame` is the innermost load's view while a collection step runs
    /// in it, until the token goes back to [`Heap::close_parse_frame`].
    #[inline]
    pub(crate) fn open_parse_frame(&mut self, frame: ParseFrame) -> ParseOpen {
        self.puc.loads.parse.push(frame);
        ParseOpen(())
    }

    #[inline]
    pub(crate) fn close_parse_frame(&mut self, _: ParseOpen) {
        self.puc.loads.parse.pop();
    }

    /// The message of the load that just failed, made as PUC makes it.
    #[inline]
    pub(crate) fn set_load_msg(&mut self, m: Gc<LuaStr>) {
        self.puc.loads.load_msg = Some(m);
    }

    /// The message [`Heap::set_load_msg`] left, if any, taken.
    #[inline]
    pub(crate) fn take_load_msg(&mut self) -> Option<Gc<LuaStr>> {
        self.puc.loads.load_msg.take()
    }

    /// The message [`Heap::set_load_msg`] left, if any.
    #[inline]
    pub(crate) fn load_msg(&self) -> Option<Gc<LuaStr>> {
        self.puc.loads.load_msg
    }

    /// The load about to run was checked for a collection already (or no
    /// more, `false`).
    #[inline]
    pub(crate) fn set_load_checked(&mut self, on: bool) {
        self.puc.loads.load_checked = on;
    }

    /// Whether the load about to run was checked already; the mark taken.
    #[inline]
    pub(crate) fn take_load_checked(&mut self) -> bool {
        std::mem::take(&mut self.puc.loads.load_checked)
    }

    /// Whether the failed load's replay shrank the stack where
    /// `luaD_pcall` caught its error, now `on`.
    #[inline]
    pub(crate) fn load_caught(&mut self, on: bool) -> bool {
        std::mem::replace(&mut self.puc.loads.load_caught, on)
    }

    /// A load that must not collect is running.
    #[inline]
    pub(crate) fn loads_held(&self) -> bool {
        self.puc.loads.load_holds
    }

    /// A load that must not collect starts, until the token is handed to
    /// [`Heap::release_loads`].
    #[inline]
    pub(crate) fn hold_loads(&mut self) -> LoadsHeld {
        LoadsHeld(std::mem::replace(&mut self.puc.loads.load_holds, true))
    }

    /// The load [`Heap::hold_loads`] started is over.
    #[inline]
    pub(crate) fn release_loads(&mut self, held: LoadsHeld) {
        self.puc.loads.load_holds = held.0;
    }

    /// Mark the message [`Heap::set_load_msg`] left.
    #[inline]
    pub(crate) fn mark_load_msg(&self, m: &mut Marker) {
        m.mark_strings(self.puc.loads.load_msg.iter());
    }
}

impl Heap {
    /// The loads' parser frames, lent to the marker `l`.
    pub(in crate::runtime::puc) fn lend_parse_frames(&mut self, l: &mut Lent) {
        l.parse = std::mem::take(&mut self.puc.loads.parse);
    }

    /// The frames the marker `l` borrowed, back, unless frames were opened
    /// meanwhile and the marker's are none.
    pub(in crate::runtime::puc) fn settle_parse_frames(&mut self, l: &mut Lent) {
        if !l.parse.is_empty() || self.puc.loads.parse.is_empty() {
            self.puc.loads.parse = std::mem::take(&mut l.parse);
        }
    }
}
