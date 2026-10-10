//! A function compiled by the method JIT away from the Vm that runs it:
//! luna-jit's LLVM backend runs a function on Cranelift's code first and
//! hands it here on its compile thread.

use crate::codegen::{ChunkSource, compile_source, shape_of};
use crate::storage::EnginePair;
use luna_core::runtime::function::Proto;

/// A function's code, copied out of its Vm, for the method JIT. Copying
/// is all the Vm's thread does; whether the method JIT takes the function
/// is found out by [`ChunkJob::compile`], on the thread that compiles.
#[doc(hidden)]
pub struct ChunkJob {
    src: ChunkSource,
}

/// What [`ChunkJob::compile`] made: the entry, of the
/// `extern "C" fn(i64, …) -> i64` shape the job was compiled for, and the
/// pair that keeps its code mapped.
#[doc(hidden)]
pub struct CompiledChunk {
    pub entry: usize,
    pub pair: EnginePair,
}

impl ChunkJob {
    pub fn of(proto: &Proto) -> ChunkJob {
        ChunkJob {
            src: ChunkSource::of(proto),
        }
    }

    /// The function compiled, when the method JIT takes it with
    /// `num_args` integer arguments and one result (`returns_one`) or none.
    pub fn compile(&self, num_args: u8, returns_one: bool) -> Option<CompiledChunk> {
        if shape_of(&self.src)? != (num_args, returns_one) {
            return None;
        }
        let (entry, pair) = compile_source(&self.src)?;
        Some(CompiledChunk {
            entry: entry as usize,
            pair,
        })
    }
}
