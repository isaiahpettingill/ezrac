use crate::compat::{prelude::*, source_path_text};

use super::{HirDeclaration, HirProgram};

pub fn text(program: &HirProgram) -> String {
    let mut out = String::new();
    out.push_str("HIR\n");
    out.push_str(&format!(
        "source: {}\n",
        source_path_text(&program.source_path)
    ));
    out.push_str(&format!(
        "analysis: functions={} shared_library_candidate={}\n",
        program.analysis.function_count, program.analysis.shared_library_candidate
    ));
    for declaration in &program.declarations {
        match declaration {
            HirDeclaration::Function(function) => {
                out.push_str(&format!(
                    "fn {} params={} return={} recursive={} tail_recursive={} loops={} tail_calls={:?}\n",
                    function.sig.name,
                    function.sig.params.len(),
                    function.sig.return_type.is_some(),
                    function.analysis.recursive,
                    function.analysis.tail_recursive,
                    function.analysis.loop_candidates,
                    function.analysis.tail_call_candidates
                ));
            }
            HirDeclaration::ExternFunction(sig) => {
                out.push_str(&format!(
                    "extern fn {} params={}\n",
                    sig.name,
                    sig.params.len()
                ));
            }
            HirDeclaration::Const(object) => object_line(&mut out, "const", &object.name),
            HirDeclaration::Alias { name, .. } => object_line(&mut out, "alias", name),
            HirDeclaration::Port(object) => object_line(&mut out, "port", &object.name),
            HirDeclaration::Mmio { object, volatile } => {
                out.push_str(&format!("mmio {} volatile={}\n", object.name, volatile));
            }
            HirDeclaration::Embed { name, section } => {
                out.push_str(&format!("embed {} section={:?}\n", name, section));
            }
            HirDeclaration::Global(object) => object_line(&mut out, "global", &object.name),
            HirDeclaration::Struct { name, fields } => {
                out.push_str(&format!("struct {} fields={}\n", name, fields.len()));
            }
        }
    }
    out
}

fn object_line(out: &mut String, kind: &str, name: &str) {
    out.push_str(&format!("{kind} {name}\n"));
}
