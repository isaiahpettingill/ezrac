use super::{TbirDeclaration, TbirProgram};

pub fn text(program: &TbirProgram) -> String {
    let mut out = String::new();
    out.push_str("TBIR\n");
    out.push_str(&format!("source: {}\n", program.source.display()));
    out.push_str(&format!(
        "target: {} pointer={} native={:?} code_size={} cache={}\n",
        program.target.name,
        program.target.pointer_width_bits,
        program.target.native_int_widths,
        program.target.prefer_code_size,
        program.target.has_cache
    ));
    out.push_str(&format!(
        "optimizations: constant_folds={} dead_marked={} inline_candidates={:?} tail_calls={:?}\n",
        program.optimizations.constant_folds,
        program.optimizations.dead_statements_marked,
        program.optimizations.inline_candidates,
        program.optimizations.tail_call_candidates
    ));
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
                effects,
                recursive,
                tail_recursive,
                loop_candidates,
            } => out.push_str(&format!(
                "fn {name} effects={effects:?} recursive={recursive} tail_recursive={tail_recursive} loops={loop_candidates}\n"
            )),
            TbirDeclaration::Object { name, kind } => {
                out.push_str(&format!("object {name} kind={kind:?}\n"));
            }
        }
    }
    out
}
