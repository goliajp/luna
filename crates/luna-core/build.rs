//! `luna_hot`: set where the build optimises. The interpreter loop's
//! helpers are inlined into it by force only then
//! (`#[cfg_attr(luna_hot, inline(always))]`): unoptimised, every local of
//! every inlined helper keeps a stack slot of its own in the loop's frame,
//! which grows past what deep nesting of Lua and native calls can afford.

fn main() {
    println!("cargo::rustc-check-cfg=cfg(luna_hot)");
    println!("cargo::rerun-if-changed=build.rs");
    if std::env::var("OPT_LEVEL").is_ok_and(|l| l != "0") {
        println!("cargo::rustc-cfg=luna_hot");
    }
}
