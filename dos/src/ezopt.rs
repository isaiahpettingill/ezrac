use alloc::{
    format,
    string::{String, ToString},
};
use ezra::{
    api::{CompileRequest, optimize_i8086_program},
    internal_ir::{ArtifactStage, ProgramArtifact},
    optimization::OptimizationOptions,
};

use crate::dos;

pub fn run(args: &[String]) -> Result<(), String> {
    if args.len() != 2 {
        return Err("usage: EZOPT input.ezi output.ezo".into());
    }
    let input = &args[0];
    let output = &args[1];
    let bytes = dos::read_file(input)
        .map_err(|error| format!("cannot read `{input}` (DOS error {})", error.0))?;
    let artifact = ProgramArtifact::decode(&bytes).map_err(|error| error.to_string())?;
    if artifact.stage != ArtifactStage::Frontend {
        return Err("EZOPT requires a frontend artifact".into());
    }
    let mut request = CompileRequest::new("<dos-ir>", artifact.target.clone());
    request.optimization = OptimizationOptions::new(artifact.optimization_level)
        .map_err(|error| format!("invalid optimization level: {error}"))?;
    let program = optimize_i8086_program(&artifact.program, &request)
        .map_err(|diagnostic| diagnostic.to_string())?;
    let bytes = ProgramArtifact::new(
        ArtifactStage::Optimized,
        artifact.target,
        artifact.optimization_level,
        program,
    )
    .encode()
    .map_err(|diagnostic| diagnostic.to_string())?;
    dos::write_file(output, &bytes)
        .map_err(|error| format!("cannot write `{output}` (DOS error {})", error.0))
}
