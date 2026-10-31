//! The state of PUC's count luna keeps besides `totalbytes`.

use super::*;

/// PUC's structures luna has no object for, as far as the count goes.
pub(in crate::runtime) struct PucModel {
    /// PUC's `totalbytes` and when the next step is due
    pub(super) count: Count,
    /// the dialect PUC's sizes, pace and steps are those of
    pub(super) dialect: crate::runtime::dialect::Dialect,
    /// `collectgarbage("stop")`: no step runs on its own while true
    pub(super) stopped: bool,
    /// 5.4 and 5.5: a finalizer is running, which no step may interrupt
    /// (PUC `GCSTPGC`)
    pub(super) fin_stop: bool,
    /// the rules the heap follows in `dialect`
    pub(super) rules: super::for_heap::HeapRules,
    /// `collectgarbage`'s pacing parameters
    pub(super) params: super::GcParams,
    /// the sizes of PUC's objects in this dialect, which `count.bytes`
    /// counts
    pub(super) sizes: super::size::PucSizes,
    /// PUC's collector state and pace
    pub(super) pace: super::pace::Pace,
    /// the records of objects PUC has and luna does not
    pub(super) records: Records,
    /// the loads under way, as the model sees them
    pub(super) loads: Loads,
    /// what the opening of the standard libraries made
    pub(super) opening: Opening,
}

/// PUC's count. A safe point compares `bytes` with `gc_limit`, side by
/// side here, in this order: compiled code reads the two words itself
/// (see [`GC_LIMIT_OFFSET`]).
#[repr(C)]
pub(in crate::runtime::puc) struct Count {
    /// PUC's `totalbytes`: every object and block at the size PUC gives it
    /// (`sizes`), not what luna's own objects take (see `runtime::puc`)
    pub(in crate::runtime::puc) bytes: usize,
    /// where the next collection step falls due (`pace`), or `usize::MAX`
    /// while there is none: the one comparison a safe point makes
    pub(in crate::runtime::puc) gc_limit: usize,
    /// above 0, nothing is counted (objects PUC made elsewhere, or not at
    /// all, are being built): changed only through [`CountingOff`] and
    /// [`CountingMark`]
    pub(in crate::runtime::puc) silent: u32,
}

/// Where `gc_limit` lies from `bytes` in [`Count`].
pub(crate) const GC_LIMIT_OFFSET: usize = std::mem::offset_of!(Count, gc_limit);

/// The records of objects PUC has and luna does not, and the state of
/// PUC's structures luna keeps none of.
pub(in crate::runtime::puc) struct Records {
    /// 5.1, 5.2: the sweep of PUC's string table under way
    pub(in crate::runtime::puc) strt: super::records::strt::StrtSweep,
    /// the string table's size and its number of strings
    pub(in crate::runtime::puc) strt_size: usize,
    pub(in crate::runtime::puc) strt_nuse: usize,
    /// the longest string PUC keeps in that table (5.1: all of them)
    pub(in crate::runtime::puc) short_max: usize,
    /// 5.3 on, `luaS_new`'s cache (see `strcache`)
    pub(in crate::runtime::puc) strcache: strcache::StrCache,
    /// 5.1's environments luna has no objects for
    pub(in crate::runtime::puc) env51_box: Option<Box<super::records::env51::Env51>>,
    /// 5.1 / 5.2: the dead open upvalues the sweep has not reached
    pub(in crate::runtime::puc) open_dead: super::records::open_upvals::OpenDead,
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
    /// uncounted objects of luna's standing for objects of PUC's of the
    /// given size, one entry for each object of PUC's (see
    /// `Heap::stand_sizes_on`)
    pub(in crate::runtime::puc) stand_ins: Vec<(*mut GcHeader, usize)>,
    /// 5.3: the threads this cycle's propagation has shrunk; 5.2: the ones
    /// the sweep kept, for the VM to shrink
    pub(in crate::runtime::puc) shrunk: Vec<*mut GcHeader>,
}

/// The loads under way, as the model sees them.
pub(in crate::runtime::puc) struct Loads {
    /// the message of the last load that failed, made as PUC makes it,
    /// until the load's caller takes it
    pub(in crate::runtime::puc) load_msg: Option<Gc<crate::runtime::LuaStr>>,
    /// the load about to run was checked for a collection already (5.4+
    /// `lua_load`, before its reader read)
    pub(in crate::runtime::puc) load_checked: bool,
    /// the failed load's replay shrank the stack where `luaD_pcall` caught
    /// its error (`Heap::load_caught`)
    pub(in crate::runtime::puc) load_caught: bool,
    /// a load that must not collect is running (`Vm::load`)
    pub(in crate::runtime::puc) load_holds: bool,
    /// the loads a collection step runs in the middle of
    pub(in crate::runtime::puc) parse: Vec<parse::ParseFrame>,
    /// what loads made before PUC does, off the list (`Heap::set_aside`)
    pub(in crate::runtime::puc) unborn: Vec<*mut GcHeader>,
    pub(in crate::runtime::puc) unborn_on: bool,
    /// the first of `unborn`: luna's names PUC makes when first used
    /// (`Heap::try_intern_name`)
    pub(in crate::runtime::puc) names_aside: usize,
    /// what loads' readers counted that is set aside and not counted back
    /// yet (`Heap::reader_call_begins`)
    pub(in crate::runtime::puc) set_aside: isize,
}

/// What the opening of the standard libraries made.
pub(in crate::runtime::puc) struct Opening {
    /// 5.1's io library opening: records of its environments are on the
    /// stack (see `vm/opening/fenv51.rs`)
    pub(in crate::runtime::puc) phantoms: bool,
    /// the library functions made as PUC's light C functions, by their
    /// number in the opening's tables: one value however often pushed
    pub(in crate::runtime::puc) lights: Vec<Option<Gc<crate::runtime::NativeClosure>>>,
}

impl Default for PucModel {
    fn default() -> PucModel {
        let d = crate::runtime::dialect::Dialect::L55;
        // nothing is counted until the dialect is known (`acct_boot`)
        PucModel {
            count: Count {
                bytes: 0,
                gc_limit: usize::MAX,
                silent: 1,
            },
            dialect: d,
            stopped: false,
            fin_stop: false,
            params: super::GcParams::new(crate::version::LuaVersion::Lua55),
            sizes: super::size::PucSizes::of(d),
            rules: super::for_heap::HeapRules::of(d, &super::size::PucSizes::of(d)),
            pace: super::pace::Pace::new(),
            records: Records {
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
                // the standard libraries make about twenty
                stand_ins: Vec::with_capacity(32),
                shrunk: Vec::new(),
            },
            loads: Loads {
                load_msg: None,
                load_checked: false,
                load_caught: false,
                load_holds: false,
                parse: Vec::new(),
                unborn: Vec::new(),
                unborn_on: false,
                names_aside: 0,
                set_aside: 0,
            },
            opening: Opening {
                phantoms: false,
                lights: Vec::new(),
            },
        }
    }
}

impl Heap {
    /// Run `f` with counting off.
    #[inline]
    pub(in crate::runtime) fn uncounted<R>(&mut self, f: impl FnOnce(&mut Heap) -> R) -> R {
        let off = self.counting_off();
        let r = f(self);
        self.counting_on(off);
        r
    }
}
