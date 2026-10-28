//! On Windows PUC's default package paths name the directory of the
//! executable, strings the collector counts: the count tests compare with
//! what `tests/windows_counts/host.c`, built with MSVC (its hash seed
//! fixed at 1, and no coincidences of addresses), printed when run from
//! `C:\luna-gc-steps` (`windows_counts/<dialect>.windows.out`), and
//! run themselves from a copy of the test binary in that directory.

use luna_core::version::LuaVersion;

/// Where the recordings' PUC ran.
#[cfg(windows)]
const PUC_DIR: &str = r"C:\luna-gc-steps";

/// Set in the copy run from [`PUC_DIR`].
#[cfg(windows)]
const IN_PUC_DIR: &str = "LUNA_TEST_IN_PUC_DIR";

/// On Windows, outside [`PUC_DIR`]: `test` run alone by a copy of this
/// binary there, without the environment's paths, and true once it
/// passed; the caller returns. Elsewhere false: the caller runs on.
pub(super) fn ran_from_puc_dir(test: &str) -> bool {
    #[cfg(windows)]
    {
        if std::env::var_os(IN_PUC_DIR).is_some() {
            return false;
        }
        let dir = std::path::Path::new(PUC_DIR);
        std::fs::create_dir_all(dir).expect("the directory");
        let exe = dir.join(format!("luna-core-it-{}.exe", test.replace("::", "-")));
        std::fs::copy(std::env::current_exe().expect("the test binary"), &exe)
            .expect("a copy of the test binary");
        let mut cmd = std::process::Command::new(&exe);
        cmd.args(["--exact", test, "--test-threads=1", "--include-ignored"])
            .env(IN_PUC_DIR, "1");
        for (k, _) in std::env::vars_os() {
            let name = k.to_string_lossy().to_uppercase();
            if ["LUA_PATH", "LUA_CPATH", "LUA_INIT"]
                .iter()
                .any(|p| name.starts_with(p))
            {
                cmd.env_remove(&k);
            }
        }
        let out = cmd.output().expect("the copy runs");
        assert!(
            out.status.success() && String::from_utf8_lossy(&out.stdout).contains("1 passed"),
            "{test} from {PUC_DIR}:\n{}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        true
    }
    #[cfg(not(windows))]
    {
        let _ = test;
        false
    }
}

/// The lines PUC built with MSVC printed for `v`: the count once the
/// libraries opened with the collector running, what the embedder's chunk
/// returned, then `bytes <added> collected <left>` for a second
/// `luaL_openlibs` and for none, with nothing set up and then with
/// `package.loaded` emptied. Only the run without `gc-stress` was
/// recorded.
pub(super) fn recorded(v: LuaVersion) -> Vec<String> {
    recorded_in("", v)
}

/// The lines recorded for `v` in the directory `dir` of
/// `tests/windows_counts` (`""`: the directory itself). No recording, or
/// a run with `gc-stress`, fails the test: there is nothing to compare with.
pub(super) fn recorded_in(dir: &str, v: LuaVersion) -> Vec<String> {
    if cfg!(feature = "gc-stress") {
        panic!(
            "no recording of PUC on Windows collecting at every check \
             (`gc-stress`): only the run without it was recorded"
        );
    }
    let d = match v {
        LuaVersion::Lua51 => "5.1",
        LuaVersion::Lua52 => "5.2",
        LuaVersion::Lua53 => "5.3",
        LuaVersion::Lua54 => "5.4",
        _ => "5.5",
    };
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/windows_counts")
        .join(dir)
        .join(format!("{d}.windows.out"));
    let text = std::fs::read_to_string(&p).unwrap_or_else(|e| {
        panic!(
            "{}: {e} (recorded on Windows by the record-windows-steps workflow)",
            p.display()
        )
    });
    text.lines().map(str::to_owned).collect()
}

/// The two numbers of a `bytes <added> collected <left>` line.
pub(super) fn bytes_collected(line: &str) -> (i64, i64) {
    let w: Vec<&str> = line.split_whitespace().collect();
    match w[..] {
        ["bytes", a, "collected", c] => (a.parse().expect("bytes"), c.parse().expect("collected")),
        _ => panic!("not a count line: {line}"),
    }
}
