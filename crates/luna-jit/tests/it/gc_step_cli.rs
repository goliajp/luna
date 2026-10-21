//! 5.1's `collectgarbage("step")` sets the threshold `stop` had put out of
//! reach, so the collector runs on its own again after it: the garbage the
//! loop makes is collected as it goes. The `luna` command counts as
//! `lua.c` does; what PUC 5.1.5 printed is below the program.

use std::process::Command;

const PROGRAM: &str = r#"collectgarbage()
collectgarbage("stop")
local base = collectgarbage("count")
collectgarbage("step", 0)
local t = {}
for i = 1, 3000 do t[i % 50] = {i} end
print(string.format("%.0f", (collectgarbage("count") - base) * 1024))
"#;

const PUC: &str = "52968\n";

#[test]
fn a_5_1_step_restarts_the_collector() {
    let dir = std::env::temp_dir().join(format!("luna-gc-step-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a directory");
    std::fs::write(dir.join("step.lua"), PROGRAM).expect("the program");
    for flag in [&["--no-jit"][..], &[][..]] {
        let out = Command::new(env!("CARGO_BIN_EXE_luna"))
            .current_dir(&dir)
            .arg("--lua=5.1")
            .args(flag)
            .arg("step.lua")
            .output()
            .expect("the luna command runs");
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&out.stdout), PUC, "{flag:?}");
    }
    let _ = std::fs::remove_dir_all(&dir); // a leftover temp directory is harmless
}
