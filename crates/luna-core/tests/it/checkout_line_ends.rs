//! The recorded outputs and fixtures the tests read check out as they are
//! committed, with LF line ends, Windows included (`.gitattributes`): a
//! comparison with a recording that gained CRs fails on every line.

use std::path::Path;

fn scan(dir: &Path, crs: &mut Vec<String>) {
    for e in std::fs::read_dir(dir).expect("a directory").flatten() {
        let p = e.path();
        if p.is_dir() {
            scan(&p, crs);
            continue;
        }
        let Ok(bytes) = std::fs::read(&p) else {
            continue;
        };
        // text only: a file with a NUL is data
        if !bytes.contains(&0) && bytes.contains(&b'\r') {
            crs.push(p.display().to_string());
        }
    }
}

#[test]
fn test_files_have_lf_line_ends() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut crs = Vec::new();
    for c in std::fs::read_dir(&crates).expect("the crates").flatten() {
        let tests = c.path().join("tests");
        if tests.is_dir() {
            scan(&tests, &mut crs);
        }
    }
    assert!(
        crs.is_empty(),
        "CR line ends in {} files: {:?}",
        crs.len(),
        &crs[..crs.len().min(5)]
    );
}
