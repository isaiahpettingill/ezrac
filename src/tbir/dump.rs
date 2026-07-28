use crate::compat::{prelude::*, source_path_text};

use super::{TbirDeclaration, TbirProgram};

pub fn text(program: &TbirProgram) -> String {
    let mut out = String::new();
    out.push_str("TBIR\n");
    out.push_str(&format!("source: {}\n", source_path_text(&program.source)));
    out.push_str(&format!(
        "target: {} pointer={} native={:?} code_size={} cache={}\n",
        program.target.name,
        program.target.pointer_width_bits,
        program.target.native_int_widths,
        program.target.prefer_code_size,
        program.target.has_cache
    ));
    out.push_str(&format!(
        "optimizations: constant_folds={} algebraic_simplifications={} constant_propagations={} dead_removed={} decisions={}\n",
        program.optimizations.constant_folds,
        program.optimizations.algebraic_simplifications,
        program.optimizations.constant_propagations,
        program.optimizations.dead_statements_marked,
        program.optimizations.decisions.len()
    ));
    for decision in &program.optimizations.decisions {
        out.push_str(&format!(
            "optimization kind={:?} outcome={:?} caller={:?} callee={} reason={}\n",
            decision.kind, decision.outcome, decision.caller, decision.callee, decision.reason
        ));
    }
    for region in &program.memory.regions {
        out.push_str(&format!(
            "region {} start=0x{:06X} size=0x{:X} access={:?} volatile={} executable={}\n",
            region.name,
            region.start,
            region.size,
            region.access,
            region.volatile,
            region.executable
        ));
    }
    for declaration in &program.declarations {
        match declaration {
            TbirDeclaration::Function {
                name,
                params,
                return_type,
                body,
                effects,
                recursive,
                tail_recursive,
                loop_candidates,
                ..
            } => out.push_str(&format!(
                "fn {name} params={params:?} return={return_type:?} body={} effects={effects:?} recursive={recursive} tail_recursive={tail_recursive} loops={loop_candidates}\n",
                body.len()
            )),
            TbirDeclaration::Object { name, kind } => {
                out.push_str(&format!("object {name} kind={kind:?}\n"));
            }
        }
    }
    out
}
