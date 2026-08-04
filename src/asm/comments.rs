use crate::{
    asm::AssemblyOptions,
    ast::{
        AccessPath, AccessSegment, AssignOp, BinaryOp, Expr, Place, Program, Stmt, Type, UnaryOp,
    },
    compat::prelude::*,
    tbir::TbirSourceComment,
};

pub fn with_readability_comments(
    assembly: String,
    program: &Program,
    options: &AssemblyOptions,
    backend: &str,
    inline_comments: &[TbirSourceComment],
) -> String {
    let mut out = String::new();
    let marker = comment_marker(backend);
    out.push_str(&format!("{marker} EZRA generated assembly for {backend}\n"));
    out.push_str(&format!("{marker} CPU: {:?}\n", options.cpu));
    out.push_str(&format!(
        "{marker} Runtime/compiler glue, inlined functions, SDK functions, and preserved source comments are annotated where the backend can identify them.\n"
    ));

    let placeable_comments = placeable_comments(&assembly, inline_comments);
    let function_comments = inline_comments
        .iter()
        .map(|comment| Some(comment.function_name.clone()))
        .collect::<Vec<_>>();
    let comments = source_comments(program)
        .into_iter()
        .filter(|comment| {
            !inline_comments.iter().enumerate().any(|(index, inline)| {
                (placeable_comments[index]
                    || function_comments[index]
                        .as_deref()
                        .is_some_and(|function| assembly_has_function_label(&assembly, function)))
                    && inline.text == *comment
            })
        })
        .collect::<Vec<_>>();
    if !comments.is_empty() {
        out.push_str(&format!("{marker} EZRA source comments:\n"));
        for comment in comments {
            out.push_str(&format!("{marker}   {comment}\n"));
        }
    }
    out.push('\n');
    let annotated = annotate_assembly(
        &assembly,
        marker,
        inline_comments,
        &function_comments,
        options.debug_comments,
    );
    let starts_with_source_anchor = assembly
        .lines()
        .next()
        .is_some_and(|line| line.trim().starts_with("; source:"));
    if !starts_with_source_anchor
        && let Some((first, rest)) = annotated.split_once('\n')
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

pub(crate) fn stmt_summary(stmt: &Stmt) -> String {
    match stmt {
        Stmt::Let { name, ty, value } => {
            format!("let {name}: {} = {}", type_display(ty), expr_summary(value))
        }
        Stmt::Assign { target, op, value } => {
            format!(
                "{} {} {}",
                place_summary(target),
                assign_op_summary(*op),
                expr_summary(value)
            )
        }
        Stmt::If { condition, .. } => format!("if {}", expr_summary(condition)),
        Stmt::While { condition, .. } => format!("while {}", expr_summary(condition)),
        Stmt::Loop { .. } => "loop".to_owned(),
        Stmt::Break => "break".to_owned(),
        Stmt::Continue => "continue".to_owned(),
        Stmt::Return(Some(expr)) => format!("return {}", expr_summary(expr)),
        Stmt::Return(None) => "return".to_owned(),
        Stmt::Asm { volatile, .. } => {
            if *volatile {
                "asm volatile".to_owned()
            } else {
                "asm".to_owned()
            }
        }
        Stmt::Out { port, value } => format!("out {port}, {}", expr_summary(value)),
        Stmt::Expr(expr) => expr_summary(expr),
    }
}

fn place_summary(place: &Place) -> String {
    match place {
        Place::Ident(name) => name.clone(),
        Place::Index { name, index } => format!("{name}[{}]", expr_summary(index)),
        Place::Field { base, field } => format!("{base}.{field}"),
        Place::Access(path) => access_path_summary(path),
        Place::Deref(expr) => format!("*{}", expr_summary(expr)),
    }
}

fn expr_summary(expr: &Expr) -> String {
    match expr {
        Expr::Int(value) => value.to_string(),
        Expr::TypedInt(value, ty) => format!("{value}{}", type_display(ty)),
        Expr::Bool(value) => value.to_string(),
        Expr::Char(value) => format!("'{}'", char::from(*value).escape_default()),
        Expr::String(value) => format!("{value:?}"),
        Expr::Array(values) => format!(
            "[{}]",
            values
                .iter()
                .map(expr_summary)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Expr::Ident(name) => name.clone(),
        Expr::In(port) => format!("in {port}"),
        Expr::Index { name, index } => format!("{name}[{}]", expr_summary(index)),
        Expr::Field { base, field } => format!("{base}.{field}"),
        Expr::AddressOfIndex { name, index } => format!("&{name}[{}]", expr_summary(index)),
        Expr::AddressOfField { base, field } => format!("&{base}.{field}"),
        Expr::Access(path) => access_path_summary(path),
        Expr::AddressOfAccess(path) => format!("&{}", access_path_summary(path)),
        Expr::AddressOf(name) => format!("&{name}"),
        Expr::StructInit { ty, fields } => format!(
            "{ty} {{ {} }}",
            fields
                .iter()
                .map(|(name, value)| format!("{name}: {}", expr_summary(value)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Expr::Deref(expr) => format!("*{}", expr_summary(expr)),
        Expr::Call { path, args } => format!(
            "{}({})",
            path.join("."),
            args.iter().map(expr_summary).collect::<Vec<_>>().join(", ")
        ),
        Expr::Unary { op, expr } => format!("{}{}", unary_op_summary(*op), expr_summary(expr)),
        Expr::Binary { left, op, right } => format!(
            "{} {} {}",
            expr_summary(left),
            binary_op_summary(*op),
            expr_summary(right)
        ),
        Expr::Cast { ty, expr } => format!("cast<{}>({})", type_display(ty), expr_summary(expr)),
        Expr::BankedPointer { pointer, bank } => {
            format!("banked_ptr<{bank}>({})", expr_summary(pointer))
        }
    }
}

pub(crate) fn access_path_summary(path: &AccessPath) -> String {
    let mut out = path.root.clone();
    for segment in &path.segments {
        match segment {
            AccessSegment::Field(field) => {
                out.push('.');
                out.push_str(field);
            }
            AccessSegment::Index(index) => {
                out.push('[');
                out.push_str(&expr_summary(index));
                out.push(']');
            }
        }
    }
    out
}

fn assign_op_summary(op: AssignOp) -> &'static str {
    match op {
        AssignOp::Set => "=",
        AssignOp::Add => "+=",
        AssignOp::Sub => "-=",
        AssignOp::Mul => "*=",
        AssignOp::Div => "/=",
        AssignOp::Mod => "%=",
        AssignOp::BitAnd => "&=",
        AssignOp::BitOr => "|=",
        AssignOp::BitXor => "^=",
        AssignOp::Shl => "<<=",
        AssignOp::Shr => ">>=",
    }
}

fn unary_op_summary(op: UnaryOp) -> &'static str {
    match op {
        UnaryOp::Neg => "-",
        UnaryOp::BitNot => "~",
        UnaryOp::Not => "!",
    }
}

fn binary_op_summary(op: BinaryOp) -> &'static str {
    match op {
        BinaryOp::Mul => "*",
        BinaryOp::Div => "/",
        BinaryOp::Mod => "%",
        BinaryOp::Add => "+",
        BinaryOp::Sub => "-",
        BinaryOp::Shl => "<<",
        BinaryOp::Shr => ">>",
        BinaryOp::Lt => "<",
        BinaryOp::Le => "<=",
        BinaryOp::Gt => ">",
        BinaryOp::Ge => ">=",
        BinaryOp::Eq => "==",
        BinaryOp::Ne => "!=",
        BinaryOp::BitAnd => "&",
        BinaryOp::BitXor => "^",
        BinaryOp::BitOr => "|",
        BinaryOp::And => "&&",
        BinaryOp::Or => "||",
    }
}

pub(crate) fn type_display(ty: &Type) -> String {
    match ty {
        Type::Named(name) => name.clone(),
        Type::Ptr(inner) => format!("ptr<{}>", type_display(inner)),
        Type::Function {
            params,
            return_type,
        } => {
            let params = params
                .iter()
                .map(type_display)
                .collect::<Vec<_>>()
                .join(", ");
            match return_type {
                Some(ty) => format!("fn({params}){}", type_display(ty)),
                None => format!("fn({params})"),
            }
        }
        Type::Array { element, len } => {
            format!("[{}; {}]", type_display(element), expr_summary(len))
        }
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

fn annotate_assembly(
    assembly: &str,
    marker: &str,
    inline_comments: &[TbirSourceComment],
    function_comments: &[Option<String>],
    debug_comments: bool,
) -> String {
    let mut out = String::with_capacity(assembly.len() + assembly.lines().count() * 12);
    let mut emitted_comments = vec![false; inline_comments.len()];
    for line in assembly.lines() {
        let trimmed = line.trim();
        let source = trimmed.strip_prefix("; source:");
        if !(source.is_some() && !debug_comments) {
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
            if trimmed.ends_with(':') {
                let label = trimmed.trim_end_matches(':');
                for (index, comment) in inline_comments.iter().enumerate() {
                    if !emitted_comments[index]
                        && !placeable_comment(assembly, comment)
                        && function_comments[index]
                            .as_deref()
                            .is_some_and(|function| function_label_matches(label, function))
                    {
                        out.push_str(&format!("{marker} {}\n", comment.text));
                        emitted_comments[index] = true;
                    }
                }
            }
        }
        if let Some(source) = source {
            let source = normalize_statement(source);
            for (index, comment) in inline_comments.iter().enumerate() {
                if !emitted_comments[index] && statements_match(&comment.statement_text, &source) {
                    out.push_str(&format!("{marker} {}\n", comment.text));
                    emitted_comments[index] = true;
                }
            }
        }
    }
    out
}

fn placeable_comment(assembly: &str, comment: &TbirSourceComment) -> bool {
    assembly
        .lines()
        .filter_map(|line| line.trim().strip_prefix("; source:"))
        .map(normalize_statement)
        .any(|source| statements_match(&comment.statement_text, &source))
}

fn placeable_comments(assembly: &str, comments: &[TbirSourceComment]) -> Vec<bool> {
    let sources = assembly
        .lines()
        .filter_map(|line| line.trim().strip_prefix("; source:"))
        .map(normalize_statement)
        .collect::<Vec<_>>();
    comments
        .iter()
        .map(|comment| {
            sources
                .iter()
                .any(|source| statements_match(&comment.statement_text, source))
        })
        .collect()
}

fn function_label_matches(label: &str, function: &str) -> bool {
    label == function || label.strip_prefix('_') == Some(function)
}

fn assembly_has_function_label(assembly: &str, function: &str) -> bool {
    assembly.lines().any(|line| {
        line.trim()
            .strip_suffix(':')
            .is_some_and(|label| function_label_matches(label, function))
    })
}

fn statements_match(statement: &str, normalized_source: &str) -> bool {
    let statement = normalize_statement(statement);
    statement == normalized_source
        || statement.starts_with(normalized_source)
        || normalized_source.starts_with(&statement)
}

fn normalize_statement(statement: &str) -> String {
    statement
        .chars()
        .filter(|ch| !ch.is_whitespace() && *ch != ';')
        .collect()
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

#[cfg(all(feature = "std", test))]
mod tests {
    use super::*;
    use crate::{
        diagnostic::{SourcePosition, SourceSpan},
        parser::parse_program,
    };
    use std::path::Path;

    #[test]
    fn places_comments_and_hides_source_anchors_in_normal_output() {
        let program = parse_program(Path::new("comments.ezra"), "fn main() {}\n").unwrap();
        let inline_comments = [TbirSourceComment {
            text: "increment the value".to_owned(),
            statement_text: "value += 1".to_owned(),
            statement_span: SourceSpan {
                file: program.source_path.clone(),
                start: SourcePosition { line: 1, column: 1 },
                end: SourcePosition { line: 1, column: 1 },
            },
            function_name: "main".to_owned(),
        }];
        let assembly = "    ; source: value += 1\n    add a, b\n".to_owned();

        let normal = with_readability_comments(
            assembly.clone(),
            &program,
            &AssemblyOptions::default(),
            "test",
            &inline_comments,
        );
        assert!(!normal.contains("; source:"), "{normal}");
        assert!(
            normal.contains("; increment the value\n    add a, b"),
            "{normal}"
        );

        let debug = with_readability_comments(
            assembly,
            &program,
            &AssemblyOptions {
                debug_comments: true,
                ..AssemblyOptions::default()
            },
            "test",
            &inline_comments,
        );
        assert!(debug.contains("; source: value += 1"), "{debug}");
        assert!(debug.contains("; increment the value"), "{debug}");
    }

    #[test]
    fn keeps_comments_for_rewritten_statements_inside_their_function() {
        let program = parse_program(
            Path::new("comments.ezra"),
            "fn main() {\n    // calculate value\n    let value: u8 = 1\n}\n",
        )
        .unwrap();
        let function = program.main_function().unwrap();
        let inline_comments = [TbirSourceComment {
            text: "calculate value".to_owned(),
            statement_text: "let value: u8 = 1".to_owned(),
            statement_span: function.body_spans[0].span.clone(),
            function_name: "main".to_owned(),
        }];

        let output = with_readability_comments(
            "_main:\n    ret\n".to_owned(),
            &program,
            &AssemblyOptions::default(),
            "test",
            &inline_comments,
        );

        assert!(
            output.contains("_main:\n; calculate value\n    ret"),
            "{output}"
        );
        assert!(!output.contains("; EZRA source comments:"), "{output}");
    }
}
