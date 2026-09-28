// every integration test that can share a process lives in this one binary:
// each separate test binary pays its own fat-LTO link. tests that need a
// process to themselves stay as files next to this directory

mod aot_alpine_smoke;
mod aot_cross_compile;
mod aot_cross_compile_traces;
mod aot_helpers_in_staticlib;
mod aot_iconst_reloc;
mod aot_inline_side_exit_fire;
mod aot_inlined_recursive;
mod aot_int_chunk_lower_into_object;
mod aot_link_and_run;
mod aot_msvc_link;
mod aot_recursive_trace;
mod aot_strkey_resolver;
mod aot_trace_fires;
mod aot_trace_lower_into_object;
mod aot_windows_mingw_link;
mod scaffold_smoke;
