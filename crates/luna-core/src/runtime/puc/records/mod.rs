//! Records of objects PUC has and luna does not: what they count, how the
//! collector marks and sweeps them.

use super::*;

pub(in crate::runtime) mod env51;
pub(in crate::runtime::puc) mod ghost;
pub(crate) use ghost::{Ghost, GhostRef};
pub(in crate::runtime) mod open_upvals;
pub(in crate::runtime) mod parse;
pub(in crate::runtime) mod strcache;
pub(in crate::runtime) mod strt;

/// The records of objects PUC has and luna does not, and the state of
/// PUC's structures luna keeps none of.
pub(in crate::runtime::puc) struct Records {
    /// 5.1, 5.2: the sweep of PUC's string table under way
    strt: strt::StrtSweep,
    /// the string table's size and its number of strings
    pub(in crate::runtime::puc) strt_size: usize,
    pub(in crate::runtime::puc) strt_nuse: usize,
    /// the longest string PUC keeps in that table (5.1: all of them)
    pub(in crate::runtime::puc) short_max: usize,
    /// 5.3 on, `luaS_new`'s cache (see `strcache`)
    pub(in crate::runtime::puc) strcache: strcache::StrCache,
    /// 5.1's environments luna has no objects for
    pub(in crate::runtime::puc) env51_box: Option<Box<env51::Env51>>,
    /// 5.1 / 5.2: the dead open upvalues the sweep has not reached
    pub(in crate::runtime::puc) open_dead: open_upvals::OpenDead,
    /// strings PUC's libraries keep and luna's may not: kept alive
    pub(in crate::runtime::puc) pins: Vec<Gc<crate::runtime::LuaStr>>,
    /// the reserved words `luaX_init` made and fixed, by their number among
    /// all dialects' (`boot::counts::reserved_words` of 5.5)
    pub(in crate::runtime::puc) reserved: [Option<Gc<crate::runtime::LuaStr>>; 23],
    /// 5.1 and 5.2: the size of the buffer concatenations are made in
    /// (`G(L)->buff`)
    pub(in crate::runtime::puc) gbuff: fmt::Gbuff,
    /// 5.1: what the last atomic step found to finalize (`udsize`)
    pub(in crate::runtime::puc) udsize: i64,
    /// 5.2: the threads the sweep kept, for the VM to shrink (5.3 marks
    /// its own on the thread, `Coro::shrunk_in`)
    pub(in crate::runtime::puc) swept_52: Vec<*mut GcHeader>,
}

impl Records {
    /// No records yet.
    pub(in crate::runtime::puc) fn new() -> Records {
        Records {
            strt: Default::default(),
            strt_size: 0,
            strt_nuse: 0,
            short_max: crate::runtime::string::MAX_SHORT_LEN,
            strcache: Default::default(),
            env51_box: None,
            open_dead: Default::default(),
            pins: Vec::new(),
            reserved: [None; 23],
            gbuff: fmt::Gbuff::default(),
            udsize: 0,
            swept_52: Vec::new(),
        }
    }
}
