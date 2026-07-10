use crate::{
    ast::{Declaration, Expr},
    diagnostic::Diagnostic,
    target::Address24,
};

pub fn validate_ez80_program(program: &crate::ast::Program) -> Result<(), Diagnostic> {
    for declaration in &program.declarations {
        match declaration {
            Declaration::Port(port) => {
                if let Some(value) = literal_int(&port.value)
                    && !(0..=0xFF).contains(&value)
                {
                    return Err(Diagnostic::new(format!(
                        "port `{}` value 0x{value:X} is outside the eZ80 8-bit port range",
                        port.name
                    )));
                }
            }
            Declaration::Mmio(mmio) => {
                if let Some(value) = literal_int(&mmio.value)
                    && !(0..=Address24::MAX as i64).contains(&value)
                {
                    return Err(Diagnostic::new(format!(
                        "mmio `{}` address 0x{value:X} is outside the eZ80 24-bit address space",
                        mmio.name
                    )));
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn literal_int(expr: &Expr) -> Option<i64> {
    match expr {
        Expr::Int(value) | Expr::TypedInt(value, _) => Some(*value),
        _ => None,
    }
}
