//! luna's `luna_newstate_seeded` and `luna_newstate_with_seeded`, which PUC
//! does not have, and 5.5's `lua_newstate` with its seed:
//! `tests/capi/newstate_seeded.c` makes states with seeds 1 to 16, twice
//! each, and reports whether one seed gave one order of a table's keys and
//! whether the seeds gave more than one.

use super::capi_hosts::{DIALECTS, build, run};

#[test]
fn seeded_states_iterate_in_the_seeds_order() {
    for (v, inc) in DIALECTS {
        let exe = build("newstate_seeded", v, inc).unwrap_or_else(|e| panic!("{e}"));
        let out = run(exe);
        assert!(out.status.success(), "{v}: {out:?}");
        // 5.1 hashes strings without a seed
        let several = if v == "51" { "no" } else { "yes" };
        let mut want = String::new();
        let mut fns = vec!["luna_newstate_seeded", "luna_newstate_with_seeded"];
        if v == "55" {
            fns.push("lua_newstate");
        }
        for f in fns {
            want.push_str(&format!(
                "{f}: one seed, one order: yes; several orders over seeds 1 to 16: {several}\n"
            ));
        }
        let got = String::from_utf8_lossy(&out.stdout).replace("\r\n", "\n");
        assert_eq!(got, want, "{v}");
    }
}
