//! The state of PUC's count luna keeps besides `totalbytes`.

use super::*;

/// PUC's structures luna has no object for, as far as the count goes.
pub(crate) struct PucState {
    /// above 0, nothing is counted (objects PUC made elsewhere, or not at all, are being built)
    pub(crate) silent: u32,
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
    /// keepers (`heap_keeper.rs`) and their hosts
    pub(crate) keepers: Vec<(*mut GcHeader, *mut GcHeader)>,
    /// 5.3: the threads this cycle's propagation has shrunk; 5.2: the ones
    /// the sweep kept, for the VM to shrink
    pub(crate) shrunk: Vec<*mut GcHeader>,
    /// tables of luna's own, which PUC has none of: never counted
    pub(crate) own_tables: Vec<*const crate::runtime::Table>,
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

impl Default for PucState {
    fn default() -> PucState {
        // nothing is counted until the dialect is known (`acct_boot`)
        PucState {
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
            own_tables: Vec::new(),
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
