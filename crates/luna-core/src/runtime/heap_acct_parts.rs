//! PUC's sizes of tables, strings and prototypes, and the order PUC
//! allocates their parts in.

use super::*;
use crate::runtime::table::Dialect;

/// A Lua closure's header bit: see [`Heap::env51`].
pub(crate) const ENV51: u8 = 0x40;

impl Heap {
    /// The bytes of a table's array part of `n` slots.
    fn array_bytes(&self, n: usize) -> usize {
        match (self.table_dialect, n) {
            (_, 0) => 0,
            // 5.5 keeps the values, then a byte per tag, around an unsigned
            (Dialect::L55, n) => n * 9 + 4,
            (_, n) => n * self.sizes.tvalue,
        }
    }

    /// The bytes of a table's hash part of `h` nodes: 5.5 puts a `lastfree`
    /// pointer (`Limbox`, padded to a node's alignment) in front of eight
    /// nodes or more.
    fn hash_bytes(&self, h: usize) -> usize {
        let limbox = if cfg!(target_arch = "wasm32") {
            8
        } else {
            self.sizes.ptr
        };
        match self.table_dialect {
            Dialect::L55 if h >= 8 => h * self.sizes.node + limbox,
            _ => h * self.sizes.node,
        }
    }

    /// A table's parts going from `old_a` array slots and `old_h` nodes to
    /// `new_a` and `new_h`, as PUC's `luaH_resize` allocates and frees them:
    /// up to 5.3 a growing array first, then the new nodes, a shrinking
    /// array, the old nodes; from 5.4 the new nodes, the array, the old
    /// nodes. A part of no nodes is PUC's shared dummy node, never
    /// allocated.
    pub(crate) fn acct_table_resize(
        &mut self,
        old_a: usize,
        old_h: usize,
        new_a: usize,
        new_h: usize,
    ) {
        let (oh, nh) = (self.hash_bytes(old_h), self.hash_bytes(new_h));
        let (oa, na) = (self.array_bytes(old_a), self.array_bytes(new_a));
        // with no trace to write, only the sum is seen
        if cfg!(not(feature = "gc-trace")) {
            if self.puc.silent == 0 {
                self.bytes = self.bytes + nh + na - oh - oa;
            }
            return;
        }
        match self.table_dialect {
            Dialect::L51 | Dialect::L52 | Dialect::L53 => {
                if new_a > old_a {
                    self.acct_block(oa, na);
                }
                self.acct_block(0, nh);
                if new_a < old_a {
                    self.acct_block(oa, na);
                }
                self.acct_block(oh, 0);
            }
            Dialect::L54 => {
                // the array is reallocated even at the same size
                self.acct_block(0, nh);
                self.acct_block(oa, na);
                self.acct_block(oh, 0);
            }
            Dialect::L55 => {
                self.acct_block(0, nh);
                if new_a != old_a {
                    self.acct_block(0, na);
                    self.acct_block(oa, 0);
                }
                self.acct_block(oh, 0);
            }
        }
    }

    /// A table freed with `a` array slots and `h` nodes (PUC `luaH_free`).
    /// Whether `t` is one of luna's own tables, which nothing counts.
    #[inline]
    pub(crate) fn is_own_table(&self, t: *const crate::runtime::Table) -> bool {
        !self.puc.own_tables.is_empty() && self.puc.own_tables.contains(&t)
    }

    pub(crate) fn acct_table_free(&mut self, a: usize, h: usize) {
        let (ab, hb) = (self.array_bytes(a), self.hash_bytes(h));
        self.acct_block(hb, 0);
        self.acct_block(ab, 0);
        let t = self.sizes.table;
        self.acct_free(t);
    }
}

impl Heap {
    /// A new string of `len` bytes (PUC `createstrobj`).
    #[inline]
    pub(crate) fn acct_string(&mut self, len: usize) {
        // a short string with no trace to write: only the sum is seen
        // (the table grows on the same condition either side of the
        // string's own count)
        if cfg!(not(feature = "gc-trace")) && self.puc.silent == 0 {
            if len > self.puc.short_max {
                let l55 = self.table_dialect == Dialect::L55;
                self.bytes += self.sizes.string(len, l55);
                return;
            }
            if self.puc.strt_nuse >= self.puc.strt_size {
                self.grow_strtab();
            }
            self.bytes += self.sizes.string(len, false);
            self.puc.strt_nuse += 1;
            return;
        }
        let d = self.table_dialect;
        let long = len > crate::runtime::string::MAX_SHORT_LEN && d != Dialect::L51;
        let kind = if long {
            PucKind::LongStr
        } else {
            PucKind::ShortStr
        };
        let size = self.sizes.string(len, long && d == Dialect::L55);
        if long || self.puc.silent > 0 {
            return self.acct_obj(kind, size);
        }
        // the string table grows when it is full: 5.2 on before the new
        // string goes in, 5.1 once it is past full
        if d != Dialect::L51 && self.puc.strt_nuse >= self.puc.strt_size {
            self.grow_strtab();
        }
        self.acct_obj(kind, size);
        self.puc.strt_nuse += 1;
        if d == Dialect::L51 && self.puc.strt_nuse > self.puc.strt_size {
            self.grow_strtab();
        }
    }

    /// The string table doubling: 5.1 allocates the new one and frees the
    /// old, later versions reallocate.
    #[cold]
    fn grow_strtab(&mut self) {
        let (old, new) = (self.puc.strt_size, self.puc.strt_size * 2);
        let p = self.sizes.ptr;
        if self.table_dialect == Dialect::L51 {
            self.acct_block(0, new * p);
            self.acct_block(old * p, 0);
        } else {
            self.acct_block(old * p, new * p);
        }
        self.puc.strt_size = new;
    }

    /// The bytes of each of PUC's arrays for `p` (see `Proto::puc_arrays`),
    /// from luna's own: a text load counts them as PUC's parser grows them
    /// instead.
    fn proto_arrays(&self, p: &crate::runtime::function::Proto) -> [u32; 7] {
        let s = self.sizes;
        let n = p.code.len();
        let (lineinfo, abs) = match self.table_dialect {
            Dialect::L54 | Dialect::L55 => (n, abs_lines(p.line_defined, &p.lines)),
            _ => (4 * n, 0),
        };
        [
            4 * n,
            s.ptr * p.protos.len(),
            s.tvalue * p.consts.len(),
            lineinfo,
            s.abslineinfo * abs,
            s.locvar * p.locvars.len(),
            s.upvaldesc * p.upvals.len(),
        ]
        .map(|b| b as u32)
    }

    /// A finished prototype with its arrays.
    /// While luna compiles a text chunk nothing is counted: the load's
    /// replay counts the prototype (and its arrays as PUC grows them), and
    /// one the replay never reaches (a chunk that failed to compile) is not
    /// counted when freed either.
    pub(crate) fn acct_proto(&mut self, p: Gc<crate::runtime::function::Proto>) {
        let arrays = self.proto_arrays(&p);
        p.puc_arrays.set(arrays);
        let h = p.as_ptr() as *mut GcHeader;
        self.acct_new(h, PucKind::Proto, self.sizes.proto);
        for b in arrays {
            self.acct_block(0, b as usize);
        }
    }

    /// A prototype freed.
    pub(crate) fn acct_proto_free(&mut self, p: &crate::runtime::function::Proto) {
        let a = p.puc_arrays.get();
        // 5.5 frees the code and line arrays first
        let order = if self.table_dialect == Dialect::L55 {
            [0, 3, 4, 1, 2, 5, 6]
        } else {
            [0, 1, 2, 3, 4, 5, 6]
        };
        for i in order {
            self.acct_block(a[i] as usize, 0);
        }
        let s = self.sizes.proto;
        self.acct_free(s);
    }
}

impl Heap {
    /// PUC's size of a Lua closure of `proto` with luna's `n` upvalues
    /// (5.1's has no upvalue for the environment luna gives it).
    pub(crate) fn closure_size(&self, proto: &crate::runtime::function::Proto, n: usize) -> usize {
        self.sizes.lua_closure(n - usize::from(self.env51(proto)))
    }

    /// Whether a closure of `proto` has 5.1's environment among its
    /// upvalues (marked [`ENV51`] in the closure's header, so that freeing
    /// it needs no prototype, which may be freed first).
    pub(crate) fn env51(&self, proto: &crate::runtime::function::Proto) -> bool {
        self.table_dialect == Dialect::L51 && proto.env_upval_idx != u8::MAX
    }

    /// A string of `len` bytes freed.
    #[inline]
    pub(crate) fn acct_string_free(&mut self, len: usize) {
        let long = len > self.puc.short_max;
        if !long && self.puc.silent == 0 {
            self.puc.strt_nuse -= 1;
        }
        let size = self
            .sizes
            .string(len, long && self.table_dialect == Dialect::L55);
        self.acct_free(size);
    }
}

/// How many absolute line entries 5.4's `savelineinfo` makes for
/// instructions on `lines`, in a function defined on `line_defined`.
fn abs_lines(line_defined: u32, lines: &[u32]) -> usize {
    let (mut prev, mut since, mut n) = (line_defined, 0u32, 0);
    for &l in lines {
        if (i64::from(l) - i64::from(prev)).abs() >= 0x80 || since >= 128 {
            n += 1;
            since = 0;
        }
        since += 1;
        prev = l;
    }
    n
}
