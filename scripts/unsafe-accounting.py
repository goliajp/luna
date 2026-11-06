#!/usr/bin/env python3
"""Checks docs/unsafe-accounting.md against the tree.

Counts the lines matching ci.yml's unsafe pattern in every .rs file under
crates/, puts each file in exactly one row of the doc's per-module table
(section 2), and fails when a row's count, the table's total, the
snapshot's totals or ci.yml's ceiling differ from the tree, or when a file
fits no row. Run from the repository root.
"""

import re
import sys
from pathlib import Path

PATTERN = re.compile(r"unsafe (\{|fn |impl |trait |extern )")
DOC = Path("docs/unsafe-accounting.md")
CI = Path(".github/workflows/ci.yml")


def is_test(parts):
    """A file of tests: under tests/, benches/ or examples/, or a path part
    under src/ naming tests."""
    if parts[0] in ("tests", "benches", "examples"):
        return True
    return any("tests" in p for p in parts[1:])


def core_row(rel):
    p = rel.split("/")
    if p[0] == "tests":
        return "`tests/`"
    if is_test(p):
        return "unit-test files under `src/`"
    s = "/".join(p[1:])
    if re.fullmatch(r"vm/exec/(fast\.rs|fast/.*|num\.rs|num/words\.rs)", s):
        return "`vm/exec` fast loop"
    if s.startswith("vm/exec/index"):
        return "`vm/exec/index*`"
    if s.startswith("vm/exec/") or s == "vm/exec.rs":
        return "`vm/exec` (other)"
    if s.startswith(("runtime/heap", "runtime/gc_hooks")):
        return "`runtime/heap*`"
    if s.startswith("runtime/puc"):
        return "`runtime/puc`"
    if s.startswith("__private"):
        return "`__private`"
    if s.startswith("runtime/table"):
        return "`runtime/table*`"
    if s.startswith("runtime/mem"):
        return "`runtime/mem`"
    if s.startswith("runtime/") or s == "runtime.rs":
        return "`runtime` (other)"
    if s.startswith("vm/lib_"):
        return "`vm/lib_*`"
    if s.startswith("vm/") or s == "vm.rs":
        return "`vm` (other)"
    if s == "stdio.rs":
        return "`stdio.rs`"
    if s == "native_stack.rs" or s.startswith("native_stack/"):
        return "`native_stack.rs`"
    if s.startswith(("jit", "frontend")):
        return "`jit`, `frontend`"
    return None


def jit_row(rel):
    p = rel.split("/")
    if p[0] != "src":
        return "`tests/`, `benches/`, `examples/`"
    if is_test(p) or p[-1].startswith("chunk_tests"):
        return "unit-test files under `src/`"
    if p[1].startswith("capi"):
        return "`capi*`"
    if p[1].startswith("jit_backend"):
        return "`jit_backend`"
    return None


def llvm_row(rel):
    return "`src/`" if rel.startswith("src/") else "`tests/`"


def row_of(path):
    """The (crate, row) of a file under crates/: the row is the start of the
    module column's text, or "" for a crate with one row."""
    crate, rel = path.parts[1], "/".join(path.parts[2:])
    if crate == "luna-core":
        return crate, core_row(rel)
    if crate == "luna-jit":
        return crate, jit_row(rel)
    if crate == "luna-jit-llvm":
        return crate, llvm_row(rel)
    return crate, ""


def doc_rows(text):
    """The section 2 rows as {(crate, module): count}, and the total."""
    sec = text.split("## 2.", 1)[1].split("\n## ", 1)[0]
    rows, crate, total = {}, None, None
    for line in sec.splitlines():
        cells = [c.strip() for c in line.strip().strip("|").split("|")]
        if len(cells) < 3 or not cells[2].strip("*").isdigit():
            continue
        n = int(cells[2].strip("*"))
        if cells[0] == "**Total**":
            total = n
            continue
        if cells[0]:
            crate = cells[0].strip("`").split("`, `")[0]
        rows[(crate, cells[1])] = n
    return rows, total


def match(doc, key):
    """The doc row whose module text starts with the classifier's row."""
    crate, row = key
    hits = [k for k in doc if k[0] == crate and k[1].startswith(row)]
    return hits[0] if len(hits) == 1 else None


def main():
    counts, tests, unplaced = {}, 0, []
    for f in sorted(Path("crates").rglob("*.rs")):
        n = sum(1 for line in f.open(encoding="utf-8") if PATTERN.search(line))
        if n == 0:
            continue
        key = row_of(f)
        if is_test(f.parts[2:]):
            tests += n
        if key[1] is None:
            unplaced.append(f"{f}: {n}")
            continue
        counts[key] = counts.get(key, 0) + n
    text = DOC.read_text(encoding="utf-8")
    doc, doc_total = doc_rows(text)
    errors = [f"no row for {u}" for u in unplaced]
    seen = set()
    for key, n in sorted(counts.items()):
        k = match(doc, key)
        if k is None:
            errors.append(f"{key[0]} {key[1]!r}: {n} sites, no single row in the doc")
            continue
        seen.add(k)
        if doc[k] != n:
            errors.append(f"{k[0]} {k[1][:40]!r}: doc says {doc[k]}, tree has {n}")
    for k, n in doc.items():
        if k not in seen and n != 0:
            errors.append(f"{k[0]} {k[1][:40]!r}: doc says {n}, tree has 0")
    total = sum(counts.values()) + sum(int(u.rsplit(": ", 1)[1]) for u in unplaced)
    if doc_total != total:
        errors.append(f"section 2 total: doc says {doc_total}, tree has {total}")
    snap = re.search(r"every `\.rs` file under `crates/`\) \| \*\*(\d+)\*\*", text)
    if not snap or int(snap.group(1)) != total:
        errors.append(f"snapshot total: doc says {snap and snap.group(1)}, tree has {total}")
    snap_tests = re.search(r"of which in tests, benches and examples \| (\d+) \|", text)
    if not snap_tests or int(snap_tests.group(1)) != tests:
        errors.append(
            f"snapshot tests: doc says {snap_tests and snap_tests.group(1)}, tree has {tests}"
        )
    ceiling = re.search(r"ceiling=(\d+)", CI.read_text(encoding="utf-8"))
    if not ceiling or int(ceiling.group(1)) != total:
        errors.append(f"ci.yml ceiling: {ceiling and ceiling.group(1)}, tree has {total}")
    for e in errors:
        print(f"unsafe-accounting: {e}")
    print(f"unsafe-accounting: {total} sites, {tests} in tests, {len(counts)} rows checked")
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main())
