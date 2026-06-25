//! Shared serde-derive output schemas across the luna-tools CLIs.
//!
//! Field names are part of the public CLI contract: downstream
//! consumers (a future `luna-heap-diff`, dashboards, IDE plugins)
//! parse these via `serde_json::from_reader`. Add new optional
//! fields with `#[serde(default)]`; never rename or remove an
//! existing field without bumping `LUNA_TOOLS_SCHEMA_VERSION`.

use serde::{Deserialize, Serialize};

/// Wire-format version embedded in every JSON-mode output as the
/// top-level `schema_version` field. Bumped on breaking field
/// renames / removals; new optional fields don't bump this.
pub const LUNA_TOOLS_SCHEMA_VERSION: u32 = 1;

/// `luna-heap-dump --out json` top-level payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeapSnapshot {
    /// See [`LUNA_TOOLS_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// luna package version that produced this snapshot. Matches
    /// `env!("CARGO_PKG_VERSION")` at the `luna-tools` binary's
    /// build time — useful for downstream diffing tools to reject
    /// mixed-version pairs.
    pub luna_version: String,
    /// Total live GC objects ([`luna_jit::vm::Vm::heap`]'s
    /// `live_objects`).
    pub total_objects: u64,
    /// Approximate total bytes ([`luna_jit::vm::Vm::heap`]'s
    /// `bytes`). Lower-bound — see the field's rustdoc on
    /// `Heap::bytes` for what's not tracked.
    pub total_bytes: u64,
    /// Per-GC-type breakdown.
    pub buckets: Vec<HeapTypeBucket>,
}

/// One row in [`HeapSnapshot::buckets`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeapTypeBucket {
    /// GC type tag name, e.g. `"table"`, `"string"`, `"proto"`.
    /// Matches the `luna_jit::inspect::ObjTag` discriminant
    /// name (lower-cased) — see `crates/luna-core/src/vm/inspect.rs`.
    pub type_name: String,
    /// Number of live objects of this type.
    pub count: u64,
    /// Approximate bytes (shells only, mirrors `Heap::bytes()`'s
    /// scope — see that field's rustdoc).
    pub bytes_approx: u64,
}

/// `luna-bin-inspect --out json` top-level payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BinInspect {
    /// See [`LUNA_TOOLS_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// Path the binary was loaded from, for downstream display.
    pub path: String,
    /// Detected file format (`"elf"`, `"macho"`, `"pe"`,
    /// `"unknown"`).
    pub format: String,
    /// Detected architecture (`"x86_64"`, `"aarch64"`, ...).
    pub arch: String,
    /// All sections whose name starts with `.luna.` / `luna_` /
    /// `.lt_` (the AOT-produced section namespace).
    pub luna_sections: Vec<BinSection>,
    /// Number of `AotTraceIndexEntry`-sized records visible in
    /// the `luna_trace_meta` / `.lt_meta` section (zero if the
    /// binary was built without trace lowering).
    pub aot_trace_entries: u32,
    /// Number of `PerExitInlineEntry`-sized records visible in
    /// the `luna_inline_chnx` / `.lt_chai` section.
    pub aot_inline_entries: u32,
    /// Length in bytes of the `.luna.bytecode` section if present
    /// (always `Some` for AOT binaries, `None` for any other
    /// binary the user mistakenly passed in).
    pub bytecode_bytes: Option<u64>,
}

/// One row in [`BinInspect::luna_sections`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BinSection {
    /// Raw section name from the object file.
    pub name: String,
    /// Section size in bytes.
    pub size: u64,
    /// Hex-formatted virtual address (`"0x100002000"`).
    pub addr: String,
}
