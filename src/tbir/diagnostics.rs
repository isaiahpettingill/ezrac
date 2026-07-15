use crate::{
    ast::{AccessSegment, Declaration, Expr, Stmt},
    diagnostic::Diagnostic,
    target::{Address24, CpuFamily, memory_model_for_cpu},
};

pub fn validate_program(program: &crate::ast::Program, cpu: CpuFamily) -> Result<(), Diagnostic> {
    let supports_port_io = cpu.capabilities().supports_port_io;
    let address_width = memory_model_for_cpu(cpu)
        .map(|memory| memory.address_width_bits)
        .unwrap_or(24);
    let max_address = if address_width >= 24 {
        Address24::MAX as i64
    } else {
        (1i64 << address_width) - 1
    };
    for declaration in &program.declarations {
        match declaration {
            Declaration::Port(port) => {
                if !supports_port_io {
                    return Err(Diagnostic::new(format!(
                        "target CPU `{}` does not support separate port I/O; declare `{}` as mmio instead",
                        cpu.as_str(),
                        port.name
                    )));
                }
                if let Some(value) = literal_int(&port.value)
                    && !(0..=0xFF).contains(&value)
                {
                    return Err(Diagnostic::new(format!(
                        "port `{}` value 0x{value:X} is outside the 8-bit port range for target CPU `{}`",
                        port.name,
                        cpu.as_str()
                    )));
                }
            }
            Declaration::Mmio(mmio) => {
                if let Some(value) = literal_int(&mmio.value)
                    && !(0..=max_address).contains(&value)
                {
                    return Err(Diagnostic::new(format!(
                        "mmio `{}` address 0x{value:X} is outside the {}{}-bit address space",
                        mmio.name,
                        if cpu == CpuFamily::Ez80 { "eZ80 " } else { "" },
                        address_width
                    )));
                }
            }
            Declaration::Function(function) if !supports_port_io => {
                validate_no_port_stmts(&function.body, cpu)?;
            }
            Declaration::Cfg { declaration, .. } => {
                validate_program(
                    &crate::ast::Program {
                        source_path: program.source_path.clone(),
                        source_text: None,
                        source_units: Vec::new(),
                        declarations: vec![(**declaration).clone()],
                    },
                    cpu,
                )?;
            }
            _ => {}
        }
    }
    Ok(())
}

fn validate_no_port_stmts(stmts: &[Stmt], cpu: CpuFamily) -> Result<(), Diagnostic> {
    for stmt in stmts {
        match stmt {
            Stmt::Out { port, .. } => return port_io_error(cpu, port),
            Stmt::Let { value, .. }
            | Stmt::Assign { value, .. }
            | Stmt::Return(Some(value))
            | Stmt::Expr(value) => validate_no_port_expr(value, cpu)?,
            Stmt::If {
                condition,
                then_body,
                else_body,
            } => {
                validate_no_port_expr(condition, cpu)?;
                validate_no_port_stmts(then_body, cpu)?;
                validate_no_port_stmts(else_body, cpu)?;
            }
            Stmt::While { condition, body } => {
                validate_no_port_expr(condition, cpu)?;
                validate_no_port_stmts(body, cpu)?;
            }
            Stmt::Loop { body } => validate_no_port_stmts(body, cpu)?,
            Stmt::Break | Stmt::Continue | Stmt::Return(None) | Stmt::Asm { .. } => {}
        }
    }
    Ok(())
}

fn validate_no_port_expr(expr: &Expr, cpu: CpuFamily) -> Result<(), Diagnostic> {
    match expr {
        Expr::In(port) => return port_io_error(cpu, port),
        Expr::Array(values) => {
            for value in values {
                validate_no_port_expr(value, cpu)?;
            }
        }
        Expr::Index { index, .. }
        | Expr::AddressOfIndex { index, .. }
        | Expr::Deref(index)
        | Expr::Unary { expr: index, .. }
        | Expr::Cast { expr: index, .. } => validate_no_port_expr(index, cpu)?,
        Expr::Access(path) | Expr::AddressOfAccess(path) => {
            for segment in &path.segments {
                if let AccessSegment::Index(index) = segment {
                    validate_no_port_expr(index, cpu)?;
                }
            }
        }
        Expr::StructInit { fields, .. } => {
            for (_, value) in fields {
                validate_no_port_expr(value, cpu)?;
            }
        }
        Expr::Call { args, .. } => {
            for arg in args {
                validate_no_port_expr(arg, cpu)?;
            }
        }
        Expr::Binary { left, right, .. } => {
            validate_no_port_expr(left, cpu)?;
            validate_no_port_expr(right, cpu)?;
        }
        Expr::Int(_)
        | Expr::TypedInt(_, _)
        | Expr::Bool(_)
        | Expr::Char(_)
        | Expr::String(_)
        | Expr::Ident(_)
        | Expr::Field { .. }
        | Expr::AddressOfField { .. }
        | Expr::AddressOf(_) => {}
    }
    Ok(())
}

fn port_io_error<T>(cpu: CpuFamily, port: &str) -> Result<T, Diagnostic> {
    Err(Diagnostic::new(format!(
        "target CPU `{}` does not support separate port I/O `{port}`; use mmio instead",
        cpu.as_str()
    )))
}

fn literal_int(expr: &Expr) -> Option<i64> {
    match expr {
        Expr::Int(value) | Expr::TypedInt(value, _) => Some(*value),
        _ => None,
    }
}
