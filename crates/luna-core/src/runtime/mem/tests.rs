use std::alloc::Layout;
use std::cell::Cell;
use std::ffi::c_void;

use super::*;
use crate::version::LuaVersion;

/// What the test allocator saw: live bytes, calls, the `osize` of the last
/// new block, and how many more allocations may succeed.
#[derive(Default)]
struct Seen {
    live: Cell<usize>,
    calls: Cell<usize>,
    last_kind: Cell<usize>,
    budget: Cell<Option<usize>>,
}

unsafe extern "C" fn test_alloc(
    ud: *mut c_void,
    p: *mut c_void,
    os: usize,
    ns: usize,
) -> *mut c_void {
    // SAFETY: every test passes a live `Seen` as `ud`
    let s = unsafe { &*(ud as *const Seen) };
    s.calls.set(s.calls.get() + 1);
    let old = if p.is_null() { 0 } else { os };
    if ns == 0 {
        if !p.is_null() {
            // SAFETY: `p` is a live block of `os` bytes this function made
            // with alignment 8
            unsafe { std::alloc::dealloc(p.cast(), Layout::from_size_align(os, 8).unwrap()) };
            s.live.set(s.live.get() - os);
        }
        return std::ptr::null_mut();
    }
    if let Some(b) = s.budget.get() {
        if b == 0 {
            return std::ptr::null_mut();
        }
        s.budget.set(Some(b - 1));
    }
    let q = if p.is_null() {
        s.last_kind.set(os);
        // SAFETY: the size is not 0
        unsafe { std::alloc::alloc(Layout::from_size_align(ns, 8).unwrap()) }
    } else {
        // SAFETY: `p` is a live block of `os` bytes this function made with
        // alignment 8; the new size is not 0
        unsafe { std::alloc::realloc(p.cast(), Layout::from_size_align(os, 8).unwrap(), ns) }
    };
    if !q.is_null() {
        s.live.set(s.live.get() - old + ns);
    }
    q.cast()
}

fn raw_owner(s: &Seen, v: LuaVersion) -> MemOwner {
    // SAFETY: `test_alloc` follows the `lua_Alloc` contract and `s` outlives
    // the owner in every test
    unsafe { MemOwner::raw(test_alloc, s as *const Seen as *mut c_void, v) }
}

#[test]
fn vec_grows_frees_and_counts_through_the_host_function() {
    let s = Seen::default();
    {
        let o = raw_owner(&s, LuaVersion::Lua54);
        let mut v: LVec<u64> = LVec::new(o.mem());
        assert_eq!(s.calls.get(), 0, "an empty vector allocates nothing");
        for i in 0..100 {
            v.push(i).unwrap();
        }
        assert_eq!(v.iter().sum::<u64>(), 4950);
        assert_eq!(s.live.get(), v.capacity() * 8);
        assert_eq!(o.ctx().in_use(), Some(s.live.get()));
        v.truncate(10);
        v.shrink_to_fit();
        assert_eq!(s.live.get(), 80);
        let sl = v.into_slice();
        assert_eq!(&sl[..3], &[0, 1, 2]);
        drop(sl);
        assert_eq!(s.live.get(), 0);
        assert_eq!(o.ctx().in_use(), Some(0));
    }
    assert_eq!(s.live.get(), 0);
}

#[test]
fn failed_growth_leaves_the_vector_unchanged() {
    let s = Seen::default();
    let o = raw_owner(&s, LuaVersion::Lua54);
    let mut v: LVec<u32> = LVec::new(o.mem());
    v.extend_from_slice(&[1, 2, 3, 4]).unwrap();
    let cap = v.capacity();
    s.budget.set(Some(0));
    let mut n = 0;
    while v.len() < cap {
        v.push(9).unwrap();
        n += 1;
    }
    assert_eq!(v.push(5), Err(Oom(o.mem())));
    assert_eq!(v.len(), cap);
    assert_eq!(&v[..4], &[1, 2, 3, 4]);
    assert_eq!(n, cap - 4);
    s.budget.set(None);
    v.push(5).unwrap();
    assert_eq!(v[cap], 5);
}

#[test]
fn new_blocks_carry_the_dialect_kind_code() {
    for (v, code) in [
        (LuaVersion::Lua51, 0),
        (LuaVersion::Lua52, 5),
        (LuaVersion::Lua53, 5),
        (LuaVersion::Lua54, 5),
        (LuaVersion::Lua55, 5),
    ] {
        let s = Seen::default();
        let o = raw_owner(&s, v);
        let b = LBox::new_kind(o.mem(), [0u64; 4], BlockKind::Table).unwrap();
        assert_eq!(s.last_kind.get(), code, "{v:?}");
        drop(b);
        let mut w: LVec<u8> = LVec::new(o.mem());
        w.push(1).unwrap();
        assert_eq!(s.last_kind.get(), 0, "a vector is not an object");
    }
    let s = Seen::default();
    let o = raw_owner(&s, LuaVersion::Lua54);
    drop(LBox::new_kind(o.mem(), 0u64, BlockKind::Proto).unwrap());
    assert_eq!(s.last_kind.get(), 10);
    drop(LBox::new_kind(o.mem(), 0u64, BlockKind::Upvalue).unwrap());
    assert_eq!(s.last_kind.get(), 9);
}

#[test]
fn set_raw_alloc_moves_later_calls_to_the_new_function() {
    let a = Seen::default();
    let b = Seen::default();
    let o = raw_owner(&a, LuaVersion::Lua54);
    let mut v: LVec<u64> = LVec::new(o.mem());
    v.push(1).unwrap();
    let calls = a.calls.get();
    // SAFETY: `test_alloc` frees blocks by size, whichever `Seen` made them
    unsafe {
        o.ctx()
            .set_raw_alloc(test_alloc, &b as *const Seen as *mut c_void)
    };
    drop(v);
    assert_eq!(a.calls.get(), calls);
    assert_eq!(b.calls.get(), 1);
}

struct Countdown(usize);

impl MemoryPolicy for Countdown {
    fn allow(&mut self, _old: usize, _new: usize, _kind: BlockKind, _in_use: usize) -> bool {
        if self.0 == 0 {
            return false;
        }
        self.0 -= 1;
        true
    }
}

#[test]
fn policy_refuses_and_counts() {
    let o = MemOwner::policy(Box::new(Countdown(2)));
    let mut v: LVec<u8> = LVec::new(o.mem());
    v.reserve_exact(10).unwrap();
    v.reserve_exact(20).unwrap();
    assert_eq!(o.ctx().in_use(), Some(20));
    assert!(v.reserve_exact(40).is_err());
    assert_eq!(v.capacity(), 20);
    drop(v);
    assert_eq!(o.ctx().in_use(), Some(0));

    let lim = MemOwner::policy(Box::new(MemoryLimit(64)));
    let mut w: LVec<u8> = LVec::new(lim.mem());
    w.reserve_exact(64).unwrap();
    assert!(w.reserve_exact(65).is_err());
}

#[test]
fn system_context_does_not_count() {
    let o = MemOwner::system();
    let mut v: LVec<String> = LVec::new(o.mem());
    v.push("a".to_owned()).unwrap();
    v.insert(0, "b".to_owned()).unwrap();
    assert_eq!(v.remove(1), "a");
    assert_eq!(o.ctx().in_use(), None);
    let c = v.try_clone().unwrap();
    assert_eq!(&c[..], &["b".to_owned()]);
}

#[test]
fn owners_keep_the_context_alive() {
    let s = Seen::default();
    let o = raw_owner(&s, LuaVersion::Lua54);
    let o2 = o.clone();
    let mut v: LVec<u64> = LVec::new(o.mem());
    v.push(7).unwrap();
    drop(o);
    v.push(8).unwrap();
    drop(v);
    assert_eq!(o2.ctx().in_use(), Some(0));
}

#[test]
fn zero_sized_elements_never_allocate() {
    let s = Seen::default();
    let o = raw_owner(&s, LuaVersion::Lua54);
    let mut v: LVec<()> = LVec::new(o.mem());
    for _ in 0..1000 {
        v.push(()).unwrap();
    }
    assert_eq!(v.len(), 1000);
    let sl = v.into_slice();
    assert_eq!(sl.len(), 1000);
    assert_eq!(sl.into_vec().len(), 1000);
    assert_eq!(s.calls.get(), 0);
}

#[test]
fn retain_take_and_swap_remove() {
    let o = MemOwner::system();
    let mut v = LVec::from_slice(o.mem(), &[1, 2, 3, 4, 5, 6]).unwrap();
    v.retain(|x| x % 2 == 0);
    assert_eq!(&v[..], &[2, 4, 6]);
    assert_eq!(v.swap_remove(0), 2);
    assert_eq!(&v[..], &[6, 4]);
    let t = v.take();
    assert!(v.is_empty() && v.capacity() == 0);
    assert_eq!(&t[..], &[6, 4]);
    let mut r = LVec::new(o.mem());
    r.resize(3, 7u8).unwrap();
    r.resize(1, 0).unwrap();
    assert_eq!(&r[..], &[7]);
}
