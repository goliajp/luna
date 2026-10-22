//! The objects PUC's parser holds on the stack while `load` calls a
//! reader, made only when the debug library looks at them.
//!
//! Each reader call registers where those objects go and what the reader
//! has handed over so far. `debug.getlocal` and `debug.setlocal` fill the
//! slots then (`load_acct/view.rs` finds what PUC holds); the objects are
//! dropped when the reader returns. They are luna's own: nothing counts
//! them, and the collector marks them as it marks luna's other objects.

mod stand_in;
mod undump;
pub(crate) use undump::UndumpShown;

use super::load_acct::{PVal, ReaderView};
use super::*;
use crate::runtime::heap::{ENV_PROTO, LUNA_ONLY};
use crate::runtime::mem::LVec;

/// A reader call of a running `load`.
pub(crate) struct ReaderLayer {
    /// the thread the load runs on
    pub(crate) thread: Option<Gc<Coro>>,
    /// the parser's slots: `n` from `at`
    pub(crate) at: u32,
    pub(crate) n: u32,
    /// what the reader has handed over so far, and the chunk's name; both
    /// the load's, which lasts as long as the call
    pub(crate) src: (*const u8, usize),
    pub(crate) name: (*const u8, usize),
    /// the functions open (`None`: the parse has not started)
    pub(crate) open: Option<u32>,
    /// a binary chunk's undumper and the objects shown for it so far (null
    /// for text), the load's, which last as long as the call
    pub(crate) undumping: *const crate::vm::dump::Undumping,
    pub(crate) shown: *mut UndumpShown,
    pub(crate) filled: bool,
}

impl Vm {
    /// Put PUC's parser objects in the slots of every reader call running
    /// that has not had them yet.
    pub(crate) fn fill_reader_layers(&mut self) -> Result<(), LuaError> {
        for i in 0..self.reader_layers.len() {
            let l = &mut self.reader_layers[i];
            if l.filled {
                continue;
            }
            l.filled = true;
            let (thread, at, n, open) = (l.thread, l.at, l.n, l.open);
            let (undumping, shown) = (l.undumping, l.shown);
            // SAFETY: the load that registered the call holds both buffers,
            // which nothing in the Vm owns or changes, until the call
            // returns, and the call is running
            let (src, name): (&[u8], &[u8]) = unsafe {
                (
                    std::slice::from_raw_parts(l.src.0, l.src.1),
                    std::slice::from_raw_parts(l.name.0, l.name.1),
                )
            };
            self.heap.puc.silent += 1;
            let objs = if undumping.is_null() {
                self.reader_objects(src, name, open)
            } else {
                // SAFETY: as for `src`: the load holds both while the call runs
                let (u, shown) = unsafe { (&*undumping, &mut *shown) };
                self.undump_objects(src, u, shown)
            };
            self.heap.puc.silent -= 1;
            let objs = objs?;
            let stack = self.stack_of_mut(thread);
            for (k, v) in objs.into_iter().take(n as usize).enumerate() {
                stack[at as usize + k] = v;
            }
        }
        Ok(())
    }

    /// The slots of the reader call that is returning, set back to nil.
    pub(crate) fn drop_reader_layer(&mut self) {
        let Some(l) = self.reader_layers.pop() else {
            return;
        };
        // what an undumper holds stays on the stack until the load is done
        if l.filled && l.undumping.is_null() {
            let stack = self.stack_of_mut(l.thread);
            let end = ((l.at + l.n) as usize).min(stack.len());
            stack[l.at as usize..end].fill(Value::Nil);
        }
    }

    /// The value stack of `thread`, running or not.
    fn stack_of_mut(&mut self, thread: Option<Gc<Coro>>) -> &mut LVec<Value> {
        match thread {
            _ if self.is_current_thread(thread) => &mut self.stack,
            Some(co) if self.is_main_coro(co) => self.main_stack_mut(),
            Some(co) => {
                self.heap.barrier_back(co);
                // SAFETY: `co` is a suspended thread the load runs on, not
                // the running one: the Vm holds no reference into its saved
                // stack while this one lasts
                unsafe { &mut co.as_mut().stack }
            }
            None => self.main_stack_mut(),
        }
    }

    /// What PUC's parser holds on the stack, in order.
    fn reader_objects(
        &mut self,
        src: &[u8],
        name: &[u8],
        open: Option<u32>,
    ) -> Result<Vec<Value>, LuaError> {
        if open.is_none() {
            // only 5.5's anchor table, still empty
            let t = self.reader_table()?;
            return Ok(vec![Value::Table(t)]);
        }
        let source = self.heap.try_intern(name)?;
        let Some(view) = self.reader_view(src, source) else {
            return Ok(Vec::new());
        };
        let tabs = self.reader_tables(&view)?;
        let tab = |id: Option<u32>| match id.and_then(|id| tabs.get(&id)) {
            Some(&t) => Value::Table(t),
            None => Value::Nil,
        };
        let main = Value::Closure(view.main);
        Ok(match self.version {
            LuaVersion::Lua51 => {
                let mut out = Vec::with_capacity(2 * view.fs_tabs.len());
                for &h in &view.fs_tabs {
                    out.push(tab(h));
                    out.push(self.proto_value()?);
                }
                out
            }
            LuaVersion::Lua52 => std::iter::once(main)
                .chain(view.fs_tabs.iter().map(|&h| tab(h)))
                .collect(),
            LuaVersion::Lua55 => vec![tab(view.chunk_tab)],
            _ => vec![main, tab(view.chunk_tab)],
        })
    }

    /// The parser tables of `view`, with the entries PUC's parser put in
    /// them, in its order.
    fn reader_tables(
        &mut self,
        view: &ReaderView,
    ) -> Result<std::collections::HashMap<u32, Gc<Table>>, LuaError> {
        // SAFETY: the closure stands for PUC's, an object of luna's own;
        // only its class byte is written
        unsafe { (*view.main.header()).cls |= LUNA_ONLY };
        let mut tabs = std::collections::HashMap::new();
        let named = |v: &Option<PVal>| match v {
            Some(PVal::Tab(id)) => Some(*id),
            _ => None,
        };
        let ids = view.fs_tabs.iter().flatten().copied().chain(view.chunk_tab);
        let ids = ids.chain(view.ops.iter().flat_map(|op| {
            [Some(op.tab), named(&Some(op.key)), named(&op.val)]
                .into_iter()
                .flatten()
        }));
        for id in ids {
            if let std::collections::hash_map::Entry::Vacant(e) = tabs.entry(id) {
                e.insert(self.reader_table()?);
            }
        }
        let resolve = |v: PVal| match v {
            PVal::V(v) => v,
            PVal::Tab(id) => tabs.get(&id).map_or(Value::Nil, |&t| Value::Table(t)),
            PVal::Closure => Value::Closure(view.main),
        };
        for op in &view.ops {
            let (Some(&t), k) = (tabs.get(&op.tab), resolve(op.key)) else {
                continue;
            };
            let v = op.val.map_or(Value::Nil, resolve);
            // SAFETY: a table made above, reached by nothing else yet
            let r = self.table_op(|vm| unsafe { t.as_mut() }.set(&mut vm.heap, k, v));
            if let Err(e) = r {
                return Err(self.table_error(e));
            }
        }
        // the parts are PUC's size: the same keys put in in the same order.
        // But 5.5's chunk table, which loses keys: a new key whose main
        // position holds a removed one takes that node in PUC, which
        // depends on where its objects are, as PUC's addresses do from run
        // to run; there the parts are given the sizes the count has
        let lost: std::collections::HashSet<u32> = view
            .ops
            .iter()
            .filter(|op| op.val.is_none())
            .map(|op| op.tab)
            .collect();
        let l55 = self.version == LuaVersion::Lua55;
        for &(id, a, n) in &view.sizes {
            let Some(&t) = tabs.get(&id) else { continue };
            if (t.asize() as u64, t.node_count() as u64) == (a, n) {
                continue;
            }
            if l55 && lost.contains(&id) {
                // SAFETY: a table made above, reached by nothing else yet
                let r = unsafe { t.as_mut() }.resize(&mut self.heap, a as usize, n as usize);
                r.map_err(|e| self.table_error(e))?;
            } else {
                self.g.reader_size_mismatch += 1;
            }
        }
        Ok(tabs)
    }

    /// How many parser tables the debug library was shown with parts of
    /// another size than PUC's (none, unless luna's tables grow otherwise).
    #[doc(hidden)]
    pub fn reader_size_mismatches(&self) -> u32 {
        self.g.reader_size_mismatch
    }

    /// A parser table: luna's own, not listed or counted.
    fn reader_table(&mut self) -> Result<Gc<Table>, LuaError> {
        let t = self.alloc_table()?;
        Ok(t)
    }

    /// A 5.1 prototype as the stack holds it: a userdata of type "proto".
    fn proto_value(&mut self) -> Result<Value, LuaError> {
        let u = self.lib_userdata(crate::runtime::userdata::UserdataPayload::Empty, false, 0)?;
        self.heap.set_env51(u.as_ptr() as *mut _, ENV_PROTO);
        Ok(Value::Userdata(u))
    }
}
