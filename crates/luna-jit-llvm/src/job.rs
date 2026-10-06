//! A function compiled by the method JIT away from the Vm that runs it:
//! luna-jit's LLVM backend runs a function on Cranelift's code first and
//! hands it here on its compile thread.

use crate::codegen::{ChunkSource, compile_source, shape_of};
use crate::storage::EnginePair;
use luna_core::runtime::function::Proto;

/// A function's code, copied out of its Vm, that the method JIT takes.
#[doc(hidden)]
pub struct ChunkJob {
    src: ChunkSource,
    num_args: u8,
    returns_one: bool,
}

/// What [`ChunkJob::compile`] made: the entry, of the
/// `extern "C" fn(i64, …) -> i64` shape the job describes, and the pair
/// that keeps its code mapped.
#[doc(hidden)]
pub struct CompiledChunk {
    pub entry: usize,
    pub pair: EnginePair,
}

impl ChunkJob {
    /// `None` when the method JIT does not take `proto`.
    pub fn of(proto: &Proto) -> Option<ChunkJob> {
        let src = ChunkSource::of(proto);
        let (num_args, returns_one) = shape_of(&src)?;
        Some(ChunkJob {
            src,
            num_args,
            returns_one,
        })
    }

    pub fn num_args(&self) -> u8 {
        self.num_args
    }

    /// Whether the code returns one value (else none).
    pub fn returns_one(&self) -> bool {
        self.returns_one
    }

    pub fn compile(&self) -> Option<CompiledChunk> {
        let (entry, pair) = compile_source(&self.src)?;
        Some(CompiledChunk {
            entry: entry as usize,
            pair,
        })
    }
}
