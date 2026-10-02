//! Every .lua file of the vendored official test suites must
//! lex + parse under its suite's version mode. The suite paths are absolute:
//! the runner in this binary changes the process cwd while these run.

use luna_core::frontend::lexer::Lexer;
use luna_core::frontend::parse;
use luna_core::version::LuaVersion;

fn parse_suite(dir: &str, version: LuaVersion) {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(dir);
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()))
        .map(|entry| entry.unwrap().path())
        .filter(|p| p.extension().is_some_and(|ext| ext == "lua"))
        .collect();
    files.sort();
    assert!(
        !files.is_empty(),
        "no .lua files found in {}",
        dir.display()
    );
    let mut failures = Vec::new();
    for path in &files {
        let src = std::fs::read(path).unwrap();
        // these are file chunks: strip the shebang/BOM as the file loaders do
        let src = Lexer::strip_shebang_bom(&src);
        if let Err(e) = parse(src, version) {
            failures.push(format!("{}:{}", path.display(), e));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} files failed to parse:\n{}",
        failures.len(),
        files.len(),
        failures.join("\n")
    );
}

#[test]
fn lua_5_5_suite() {
    parse_suite("tests/official/lua-5.5.1-tests", LuaVersion::Lua55);
}

#[test]
fn lua_5_4_suite() {
    parse_suite("tests/official/lua-5.4.9-tests", LuaVersion::Lua54);
}

#[test]
fn lua_5_3_suite() {
    parse_suite("tests/official/lua-5.3.4-tests", LuaVersion::Lua53);
}

#[test]
fn lua_5_2_suite() {
    parse_suite("tests/official/lua-5.2.2-tests", LuaVersion::Lua52);
}

#[test]
fn lua_5_1_suite() {
    parse_suite("tests/official/lua5.1-tests", LuaVersion::Lua51);
}
