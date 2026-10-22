//! The state of PUC's count luna keeps besides `totalbytes`.

use super::*;

/// PUC's structures luna has no object for, as far as the count goes.
pub(crate) struct PucModel {
    /// PUC's `totalbytes`: every object and block at the size PUC gives it
    /// (`sizes`), not what luna's own objects take (see `runtime::puc`)
    pub(crate) bytes: usize,
    /// where the next collection step falls due (`pace`), or `usize::MAX`
    /// while there is none: the one comparison a safe point makes
    pub(crate) gc_limit: usize,
    /// the sizes of PUC's objects in this dialect, which `bytes` counts
    pub(crate) sizes: super::size::PucSizes,
    /// PUC's collector state and pace
    pub(crate) pace: super::pace::Pace,
    /// 5.1, 5.2: the sweep of PUC's string table under way
    pub(in crate::runtime) strt: super::records::strt::StrtSweep,
    /// 5.1's environments luna has no objects for
    pub(crate) env51_box: Option<Box<super::records::env51::Env51>>,
    /// 5.1 / 5.2: the dead open upvalues the sweep has not reached
    pub(crate) open_dead: super::records::open_upvals::OpenDead,
    /// above 0, nothing is counted (objects PUC made elsewhere, or not at
    /// all, are being built): changed only through [`CountingOff`] and
    /// [`CountingMark`]
    pub(in crate::runtime) silent: u32,
    /// the string table's size and its number of strings
    pub(crate) strt_size: usize,
    pub(crate) strt_nuse: usize,
    /// the longest string PUC keeps in that table (5.1: all of them)
    pub(crate) short_max: usize,
    /// strings PUC's libraries keep and luna's may not: kept alive
    pub(crate) pins: Vec<Gc<crate::runtime::LuaStr>>,
    /// 5.1 and 5.2: the size of the buffer concatenations are made in
    /// (`G(L)->buff`)
    pub(crate) gbuff: fmt::Gbuff,
    /// 5.1: what the last atomic step found to finalize (`udsize`)
    pub(crate) udsize: i64,
    /// uncounted objects of luna's standing for objects of PUC's of the
    /// given size, one entry for each object of PUC's (see
    /// `Heap::stand_sizes_on`)
    pub(crate) stand_ins: Vec<(*mut GcHeader, usize)>,
    /// keepers (`puc::records::keeper`) and their hosts
    pub(crate) keepers: Vec<(*mut GcHeader, *mut GcHeader)>,
    /// 5.3: the threads this cycle's propagation has shrunk; 5.2: the ones
    /// the sweep kept, for the VM to shrink
    pub(crate) shrunk: Vec<*mut GcHeader>,
    /// the message of the last load that failed, made as PUC makes it,
    /// until the load's caller takes it
    pub(crate) load_msg: Option<Gc<crate::runtime::LuaStr>>,
    /// the number the last table, function, thread or userdata made got
    /// (`Heap::take_id`): its hash as a key, where PUC's is its address
    pub(crate) next_id: u32,
    /// the load about to run was checked for a collection already (5.4+
    /// `lua_load`, before its reader read)
    pub(crate) load_checked: bool,
    /// a load that must not collect is running (`Vm::load`)
    pub(crate) load_holds: bool,
    /// the loads a collection step runs in the middle of
    pub(crate) parse: Vec<parse::ParseFrame>,
    /// what loads made before PUC does, off the list (`Heap::set_aside`)
    pub(crate) unborn: Vec<*mut GcHeader>,
    pub(crate) unborn_on: bool,
    /// 5.1's io library opening: records of its environments are on the
    /// stack (see `vm/opening/fenv51.rs`)
    pub(crate) phantoms: bool,
    /// `Vm::open_base` runs: the base library without its file loaders
    pub(crate) sandbox_base: bool,
    /// what loads' readers counted that is set aside and not counted back
    /// yet (`Heap::defer_begin`)
    pub(crate) set_aside: isize,
    /// the library functions made as PUC's light C functions, by their
    /// number in the opening's tables: one value however often pushed
    pub(crate) lights: Vec<Option<Gc<crate::runtime::NativeClosure>>>,
    /// 5.3 on, `luaS_new`'s cache (see `strcache`)
    pub(crate) strcache: strcache::StrCache,
}

impl Default for PucModel {
    fn default() -> PucModel {
        // nothing is counted until the dialect is known (`acct_boot`)
        PucModel {
            bytes: 0,
            gc_limit: usize::MAX,
            sizes: super::size::PucSizes::of(crate::runtime::table::Dialect::L55),
            pace: super::pace::Pace::new(),
            strt: Default::default(),
            env51_box: None,
            open_dead: Default::default(),
            silent: 1,
            strt_size: 0,
            strt_nuse: 0,
            short_max: crate::runtime::string::MAX_SHORT_LEN,
            pins: Vec::new(),
            gbuff: fmt::Gbuff::default(),
            udsize: 0,
            // the standard libraries make about twenty
            stand_ins: Vec::with_capacity(32),
            keepers: Vec::new(),
            shrunk: Vec::new(),
            load_msg: None,
            next_id: 0,
            load_checked: false,
            load_holds: false,
            parse: Vec::new(),
            unborn: Vec::new(),
            unborn_on: false,
            phantoms: false,
            sandbox_base: false,
            set_aside: 0,
            lights: Vec::new(),
            strcache: Default::default(),
        }
    }
}

/// Counting stopped once (luna builds objects PUC made elsewhere, or not
/// at all), until [`Heap::counting_on`] takes the token back. Only
/// [`Heap::counting_off`] makes one.
#[must_use = "counting stays off until the token goes back to `Heap::counting_on`"]
pub(crate) struct CountingOff(());

/// How far counting was off at some point, for a caller to return
/// to it whatever happened since (a load unwound by running out of memory).
#[must_use]
pub(crate) struct CountingMark(u32);

impl Heap {
    /// Whether what is made now is counted.
    #[inline]
    pub(crate) fn counting(&self) -> bool {
        self.puc.silent == 0
    }

    /// Stop counting until the token is handed back.
    #[inline]
    pub(crate) fn counting_off(&mut self) -> CountingOff {
        self.puc.silent += 1;
        CountingOff(())
    }

    /// Count again what [`Heap::counting_off`] stopped.
    #[inline]
    pub(crate) fn counting_on(&mut self, _: CountingOff) {
        self.puc.silent -= 1;
    }

    /// Run `f` with counting off.
    #[inline]
    pub(crate) fn uncounted<R>(&mut self, f: impl FnOnce(&mut Heap) -> R) -> R {
        let off = self.counting_off();
        let r = f(self);
        self.counting_on(off);
        r
    }

    /// Where counting stands now, to return to with [`Heap::restore_counting`].
    #[inline]
    pub(crate) fn counting_mark(&self) -> CountingMark {
        CountingMark(self.puc.silent)
    }

    /// Count everything from here, also in the middle of what is not counted
    /// (an emergency collection): where it stood, to return to.
    #[inline]
    pub(crate) fn count_all(&mut self) -> CountingMark {
        CountingMark(std::mem::replace(&mut self.puc.silent, 0))
    }

    /// Back to where counting stood at `m`.
    #[inline]
    pub(crate) fn restore_counting(&mut self, m: CountingMark) {
        self.puc.silent = m.0;
    }
}
