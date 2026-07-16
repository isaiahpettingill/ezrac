use crate::{asm::AssemblyOptions, ast::Program, compat::prelude::*};

pub fn with_readability_comments(
    assembly: String,
    program: &Program,
    options: &AssemblyOptions,
    backend: &str,
) -> String {
    let mut out = String::new();
    let marker = comment_marker(backend);
    out.push_str(&format!("{marker} EZRA generated assembly for {backend}\n"));
    out.push_str(&format!("{marker} CPU: {:?}\n", options.cpu));
    out.push_str(&format!(
        "{marker} Runtime/compiler glue, inlined functions, SDK functions, and preserved source comments are annotated where the backend can identify them.\n"
    ));

    let comments = source_comments(program);
    if !comments.is_empty() {
        out.push_str(&format!("{marker} EZRA source comments:\n"));
        for comment in comments {
            out.push_str(&format!("{marker}   {comment}\n"));
        }
    }
    out.push('\n');
    let annotated = annotate_assembly(&assembly, marker);
    if let Some((first, rest)) = annotated.split_once('\n')
        && first.trim_start().starts_with(marker)
    {
        let mut preserved = String::new();
        preserved.push_str(first);
        preserved.push('\n');
        preserved.push_str(&out);
        preserved.push_str(rest);
        preserved.push('\n');
        preserved
    } else {
        out.push_str(&annotated);
        out
    }
}

fn comment_marker(_backend: &str) -> &'static str {
    // Every bundled assembler and generated emitter accepts semicolon comments.
    ";"
}

fn source_comments(program: &Program) -> Vec<String> {
    let mut comments = Vec::new();
    let units: Vec<&str> = if program.source_units.is_empty() {
        program.source_text.as_deref().into_iter().collect()
    } else {
        program
            .source_units
            .iter()
            .map(|unit| unit.text.as_str())
            .collect()
    };
    for text in units {
        for line in text.lines() {
            if let Some(comment) = line.trim_start().strip_prefix("//") {
                let comment = comment.trim();
                if !comment.is_empty() {
                    comments.push(comment.to_owned());
                }
            }
        }
    }
    comments.sort();
    comments.dedup();
    comments
}

fn annotate_assembly(assembly: &str, marker: &str) -> String {
    let mut out = String::with_capacity(assembly.len() + assembly.lines().count() * 12);
    for line in assembly.lines() {
        let trimmed = line.trim();
        if trimmed.ends_with(':') && !trimmed.starts_with('.') {
            let label = trimmed.trim_end_matches(':');
            let kind = if label == "main" || label.starts_with("__ezra_") {
                "compiler/runtime label"
            } else if label.contains("sdk") || label.contains("mos") || label.contains("vdp") {
                "SDK-related label"
            } else {
                "EZRA function or local label"
            };
            out.push_str(&format!("{marker} {kind}: {label}\n"));
        } else if is_call(trimmed) {
            out.push_str(&format!("{marker} call into EZRA/compiler/SDK routine\n"));
        } else if is_inline_asm_boundary(trimmed) {
            out.push_str(&format!("{marker} inline assembly from EZRA source\n"));
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

fn is_call(trimmed: &str) -> bool {
    let lower = trimmed.to_ascii_lowercase();
    lower.starts_with("call ")
        || lower.starts_with("jsr ")
        || lower.starts_with("bsr ")
        || lower.starts_with("bl ")
}

fn is_inline_asm_boundary(trimmed: &str) -> bool {
    trimmed.contains("inline asm") || trimmed.contains("inline assembly")
}
