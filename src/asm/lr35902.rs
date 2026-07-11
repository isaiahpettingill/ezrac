use std::fs;

use crate::{
    asm::AssemblyOptions,
    ast::{Declaration, EmbedSource, Expr, Program, Stmt},
    diagnostic::Diagnostic,
    target::CpuFamily,
};

pub fn emit_lr35902_assembly_with_options(
    program: &Program,
    options: AssemblyOptions,
) -> Result<String, Diagnostic> {
    if options.cpu != CpuFamily::Lr35902 {
        return Err(Diagnostic::new(
            "LR35902 emitter requires an LR35902 target",
        ));
    }
    if program.main_function().is_none() {
        return Err(Diagnostic::new(
            "Game Boy programs require a `main` function",
        ));
    }

    let mut out = String::new();
    out.push_str("; EZRA LR35902/Game Boy source backend\n");
    out.push_str("di\n");
    out.push_str(&format!("ld sp, {:04X}h\n", options.stack_top.get()));
    out.push_str("call _main\n");
    out.push_str("__ezra_exit:\n");
    out.push_str("halt\n");
    out.push_str("jr __ezra_exit\n\n");

    for declaration in &program.declarations {
        if let Declaration::Function(function) = declaration {
            if !function.params.is_empty() || function.return_type.is_some() {
                return Err(Diagnostic::new(format!(
                    "Game Boy backend currently supports only parameterless void functions; `{}` has an unsupported signature",
                    function.name
                )));
            }
            out.push_str(&format!("_{}:\n", function.name));
            let local_label_prefix = function
                .name
                .chars()
                .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
                .collect::<String>();
            let mut returned = false;
            for statement in &function.body {
                match statement {
                    Stmt::Asm {
                        inputs,
                        outputs,
                        lines,
                        ..
                    } if inputs.is_empty() && outputs.is_empty() => {
                        for line in lines {
                            out.push_str(&line.replace('.', &format!(".{local_label_prefix}_")));
                            out.push('\n');
                        }
                    }
                    Stmt::Expr(Expr::Call { path, args }) if args.is_empty() => {
                        let name = path
                            .last()
                            .ok_or_else(|| Diagnostic::new("empty call path"))?;
                        out.push_str(&format!("call _{name}\n"));
                    }
                    Stmt::Return(None) => {
                        out.push_str("ret\n");
                        returned = true;
                    }
                    _ => {
                        return Err(Diagnostic::new(format!(
                            "Game Boy backend currently supports LR35902 asm blocks, zero-argument function calls, and `return`; unsupported statement in `{}`",
                            function.name
                        )));
                    }
                }
            }
            if !returned {
                out.push_str("ret\n");
            }
            out.push('\n');
        }
    }

    for declaration in &program.declarations {
        if let Declaration::Embed(embed) = declaration {
            let bytes = embed_bytes(program, &embed.source)?;
            out.push_str(&format!("_{}:\n", embed.name));
            for chunk in bytes.chunks(16) {
                out.push_str("db ");
                for (index, byte) in chunk.iter().enumerate() {
                    if index > 0 {
                        out.push_str(", ");
                    }
                    out.push_str(&format!("{:02X}h", byte));
                }
                out.push('\n');
            }
            out.push_str(&format!("_{}_end:\n\n", embed.name));
        }
    }

    Ok(out)
}

fn embed_bytes(program: &Program, source: &EmbedSource) -> Result<Vec<u8>, Diagnostic> {
    match source {
        EmbedSource::File(path) => {
            let path = program
                .source_path
                .parent()
                .unwrap_or_else(|| std::path::Path::new("."))
                .join(path);
            fs::read(&path).map_err(|error| {
                Diagnostic::new(format!(
                    "failed to read embedded asset `{}`: {error}",
                    path.display()
                ))
            })
        }
        EmbedSource::Bytes(values) => values.iter().map(const_u8).collect(),
        EmbedSource::Text(text) => Ok(text.as_bytes().to_vec()),
        EmbedSource::CStr(text) => {
            let mut bytes = text.as_bytes().to_vec();
            bytes.push(0);
            Ok(bytes)
        }
        EmbedSource::Repeat { value, len } => {
            let value = const_u8(value)?;
            let len = const_usize(len)?;
            Ok(vec![value; len])
        }
    }
}

fn const_u8(expr: &Expr) -> Result<u8, Diagnostic> {
    let value = match expr {
        Expr::Int(value) | Expr::TypedInt(value, _) => *value,
        Expr::Bool(value) => i64::from(*value),
        Expr::Char(value) => i64::from(*value),
        _ => {
            return Err(Diagnostic::new(
                "Game Boy embedded bytes must be constant integers",
            ));
        }
    };
    u8::try_from(value)
        .map_err(|_| Diagnostic::new(format!("embedded byte {value} is outside 0..255")))
}

fn const_usize(expr: &Expr) -> Result<usize, Diagnostic> {
    let value = match expr {
        Expr::Int(value) | Expr::TypedInt(value, _) => *value,
        _ => {
            return Err(Diagnostic::new(
                "Game Boy embed repeat length must be a constant integer",
            ));
        }
    };
    usize::try_from(value)
        .map_err(|_| Diagnostic::new("Game Boy embed repeat length must be non-negative"))
}
