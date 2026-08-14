use alloc::{
    format,
    string::{String, ToString},
};
use ezra::{
    api::{CompileRequest, emit_optimized_i8086_program},
    internal_ir::{ArtifactStage, ProgramArtifact},
    optimization::OptimizationOptions,
};

use crate::dos;

pub fn run(args: &[String]) -> Result<(), String> {
    if args.len() != 2 {
        return Err("usage: EZCG input.ezo output.asm".into());
    }
    let input = &args[0];
    let output = &args[1];
    let bytes = dos::read_file(input)
        .map_err(|error| format!("cannot read `{input}` (DOS error {})", error.0))?;
    let artifact = ProgramArtifact::decode(&bytes).map_err(|error| error.to_string())?;
    if artifact.stage != ArtifactStage::Optimized {
        return Err("EZCG requires an optimized artifact".into());
    }
    let mut request = CompileRequest::new("<dos-ir>", artifact.target);
    request.optimization = OptimizationOptions::new(artifact.optimization_level)
        .map_err(|error| format!("invalid optimization level: {error}"))?;
    let assembly = emit_optimized_i8086_program(&artifact.program, &request)
        .map_err(|diagnostic| diagnostic.to_string())?;
    dos::write_file(output, assembly.as_bytes())
        .map_err(|error| format!("cannot write `{output}` (DOS error {})", error.0))
}
