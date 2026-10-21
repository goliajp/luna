//! `gc-stress` at an interval: the offsets `0..k` of the interval `k`
//! together collect at every checkpoint a full-strength run collects at,
//! each at one offset only.

use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

const SRC: &str = "
local t = {}
for i = 1, 200 do t[i] = {i, tostring(i) .. 'x'} end
local s = 0
for i = 1, #t do s = s + #t[i][2] + #{t[i][1]} end
return s";

/// The checkpoints that collected running `SRC` at `every` from `offset`,
/// and what it returned.
fn collected(v: LuaVersion, every: u64, offset: u64) -> (Vec<u64>, String) {
    let mut vm = Vm::new(v);
    vm.open_base().expect("open the library");
    vm.gc_stress_interval(every, offset);
    vm.gc_stress_log();
    let r = format!("{:?}", vm.eval(SRC).expect("runs")[0]);
    (vm.gc_stress_collected().to_vec(), r)
}

#[test]
fn the_offsets_of_an_interval_cover_every_checkpoint_once() {
    for v in [LuaVersion::Lua51, LuaVersion::Lua52, LuaVersion::Lua54] {
        let (full, want) = collected(v, 1, 0);
        assert!(full.len() > 100, "{v:?}: {} checkpoints", full.len());
        assert!(full.iter().copied().eq(0..full.len() as u64), "{v:?}");
        for k in [2, 3, 7] {
            let mut all = Vec::new();
            for i in 0..k {
                let (c, r) = collected(v, k, i);
                assert_eq!(r, want, "{v:?} {k} {i}");
                assert!(c.iter().all(|n| n % k == i), "{v:?} {k} {i}");
                all.extend(c);
            }
            all.sort_unstable();
            assert_eq!(all, full, "{v:?} {k}");
        }
    }
}
