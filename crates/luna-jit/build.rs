// The C half of the C API: error boundaries and the API functions that
// may raise an error or yield (see `src/capi.rs`).
fn main() {
    println!("cargo:rerun-if-changed=csrc");
    let mut files: Vec<_> = std::fs::read_dir("csrc")
        .expect("csrc")
        .map(|e| e.expect("csrc entry").path())
        .filter(|p| p.extension().is_some_and(|x| x == "c"))
        .collect();
    files.sort();
    for f in &files {
        println!("cargo:rerun-if-changed={}", f.display());
    }
    // the C API tests compile C hosts for the target they run on
    println!(
        "cargo:rustc-env=LUNA_BUILD_TARGET={}",
        std::env::var("TARGET").expect("cargo sets TARGET")
    );
    cc::Build::new()
        .files(&files)
        .include("csrc")
        .warnings(true)
        .compile("luna_capi");
}
