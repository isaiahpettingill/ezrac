//! Version-locked binary artifacts used by the split DOS compiler.

use crate::{
    ast::{Declaration, Program},
    compat::prelude::*,
    diagnostic::Diagnostic,
};

const MAGIC: [u8; 4] = *b"EZDI";
const FORMAT_VERSION: u16 = 1;
const COMPILER_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone, Copy, Debug, Eq, PartialEq, bincode::Encode, bincode::Decode)]
pub enum ArtifactStage {
    Frontend,
    Optimized,
}

#[derive(Clone, Debug, PartialEq, bincode::Encode, bincode::Decode)]
pub struct ProgramArtifact {
    magic: [u8; 4],
    format_version: u16,
    compiler_version: String,
    pub stage: ArtifactStage,
    pub target: String,
    pub optimization_level: u8,
    pub program: Program,
}

impl ProgramArtifact {
    pub fn new(
        stage: ArtifactStage,
        target: impl Into<String>,
        optimization_level: u8,
        mut program: Program,
    ) -> Self {
        strip_formatting_data(&mut program);
        Self {
            magic: MAGIC,
            format_version: FORMAT_VERSION,
            compiler_version: COMPILER_VERSION.to_owned(),
            stage,
            target: target.into(),
            optimization_level,
            program,
        }
    }

    pub fn encode(&self) -> Result<Vec<u8>, Diagnostic> {
        bincode::encode_to_vec(self, binary_config())
            .map_err(|error| Diagnostic::new(format!("cannot encode compiler artifact: {error}")))
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, Diagnostic> {
        let (artifact, consumed): (Self, usize) =
            bincode::decode_from_slice(bytes, binary_config()).map_err(|error| {
                Diagnostic::new(format!("cannot decode compiler artifact: {error}"))
            })?;
        if consumed != bytes.len() {
            return Err(Diagnostic::new("compiler artifact has trailing data"));
        }
        if artifact.magic != MAGIC {
            return Err(Diagnostic::new("not an EZRA DOS compiler artifact"));
        }
        if artifact.format_version != FORMAT_VERSION {
            return Err(Diagnostic::new(format!(
                "compiler artifact format {} is not supported; expected {}",
                artifact.format_version, FORMAT_VERSION
            )));
        }
        if artifact.compiler_version != COMPILER_VERSION {
            return Err(Diagnostic::new(format!(
                "compiler artifact was written by EZRAC {}; this stage is {}",
                artifact.compiler_version, COMPILER_VERSION
            )));
        }
        Ok(artifact)
    }
}

fn binary_config() -> impl bincode::config::Config {
    bincode::config::standard()
        .with_little_endian()
        .with_fixed_int_encoding()
}

fn strip_formatting_data(program: &mut Program) {
    program.source_text = None;
    program.source_units.clear();
    for declaration in &mut program.declarations {
        strip_declaration_formatting(declaration);
    }
}

fn strip_declaration_formatting(declaration: &mut Declaration) {
    match declaration {
        Declaration::Cfg { declaration, .. } | Declaration::Bank { declaration, .. } => {
            strip_declaration_formatting(declaration);
        }
        Declaration::Function(function) => function.body_spans.clear(),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_program;

    #[test]
    fn binary_program_artifact_round_trips_without_formatting_data() {
        let program = parse_program("main.ezra", "fn main() { let value: u8 = 1 }").unwrap();
        let artifact = ProgramArtifact::new(ArtifactStage::Frontend, "msdos-com-i8086", 1, program);
        let bytes = artifact.encode().unwrap();
        let decoded = ProgramArtifact::decode(&bytes).unwrap();
        assert_eq!(decoded.stage, ArtifactStage::Frontend);
        assert_eq!(decoded.target, "msdos-com-i8086");
        assert!(decoded.program.source_text.is_none());
        assert!(decoded.program.source_units.is_empty());
    }
}
