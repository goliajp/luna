//! Coroutine results too many to hand to the resumer.

use super::*;

/// The body of the two tests below: five coroutines that each return
/// about `LUAI_MAXSTACK` values; `coroutine.resume` refuses to transfer
/// them to the main thread (PUC `auxresume`'s `lua_checkstack`).
const RESUME_TOO_MANY_RESULTS: &str = "local lim = 1000000 \
     local out = {} \
     for _, j in ipairs{lim - 10, lim - 5, lim - 1, lim, lim + 1} do \
         local co = coroutine.create(function () \
             local t = {} \
             for i = 1, j do t[i] = i end \
             return table.unpack(t) \
         end) \
         local r = coroutine.resume(co) \
         out[#out + 1] = tostring(r) \
         STEP \
     end \
     return table.concat(out, ',')";

#[test]
fn coroutine_resume_refuses_too_many_results() {
    // 5.4+'s `lua_checkstack`, refused once, leaves the main thread's
    // stack at its error size until a collection shrinks it (PUC
    // `luaD_shrinkstack`); every resume is refused only when a collection
    // runs between them. PUC gets one from the stack allocations of the
    // next coroutine; here it is asked for explicitly.
    check_str(
        &RESUME_TOO_MANY_RESULTS.replace("STEP", "collectgarbage()"),
        b"false,false,false,false,false",
    );
}

/// The same without the explicit collection: PUC's run collects while the
/// next coroutine fills its stack, because stack memory counts toward the
/// collector's debt. Enable once stack allocations count here too.
#[test]
#[ignore]
fn coroutine_resume_refuses_too_many_results_without_explicit_collection() {
    check_str(
        &RESUME_TOO_MANY_RESULTS.replace("STEP", ""),
        b"false,false,false,false,false",
    );
}
