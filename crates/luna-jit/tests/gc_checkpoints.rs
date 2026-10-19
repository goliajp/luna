//! What a full collection at every point PUC checks whether a step is due
//! leaves alive: each script in `tests/gc_checkpoints/<dialect>/` logs, as
//! its objects are finalized, the phase it was in, and the luna command
//! built with `gc-stress` must print what PUC, collecting fully at each
//! such point, printed (the recorded `.out`), with and without the JIT.

use std::path::Path;
use std::process::Command;

fn run(dialect: &str, script: &Path, jit: bool) -> String {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_luna"));
    cmd.arg(format!("--lua={dialect}"));
    if !jit {
        cmd.arg("--no-jit");
    }
    let out = cmd.arg(script).env("TZ", "UTC").output().expect("run luna");
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    text.replace("\r\n", "\n")
}

#[test]
fn every_checkpoint_collecting() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/gc_checkpoints");
    let mut failures = Vec::new();
    let mut ran = 0;
    for dialect in ["5.1", "5.2", "5.3", "5.4", "5.5"] {
        let mut scripts: Vec<_> = std::fs::read_dir(root.join(dialect))
            .expect("the dialect's scripts")
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "lua"))
            .collect();
        scripts.sort();
        for script in scripts {
            let want =
                std::fs::read_to_string(script.with_extension("out")).expect("recorded output");
            for jit in [false, true] {
                ran += 1;
                let got = run(dialect, &script, jit);
                if got != want {
                    failures.push(format!(
                        "{} {dialect} jit={jit}\n--- PUC\n{want}--- luna\n{got}",
                        script.display()
                    ));
                }
            }
        }
    }
    assert!(ran > 0, "no scripts ran");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
