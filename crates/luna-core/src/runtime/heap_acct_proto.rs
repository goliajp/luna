//! PUC's sizes of prototypes and closures, and the order PUC frees a
//! prototype's arrays in.

use super::*;
use crate::runtime::table::Dialect;

/// A Lua closure's header bit: see [`Heap::env51`].
pub(crate) const ENV51: u8 = 0x40;

impl Heap {
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
    /// counted when freed either. The arrays it keeps for its free are
    /// those counted here: none while nothing is.
    pub(crate) fn acct_proto(&mut self, p: Gc<crate::runtime::function::Proto>) {
        let h = p.as_ptr() as *mut GcHeader;
        self.acct_new(h, PucKind::Proto, self.sizes.proto);
        let arrays = match self.puc.silent {
            0 => self.proto_arrays(&p),
            _ => [0; 7],
        };
        p.puc_arrays.set(arrays);
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
            self.acct_part(a[i] as usize, 0);
        }
        let s = self.sizes.proto;
        self.acct_part(s, 0);
    }

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
