use crate::ast::Declaration;

/// Returns the declaration represented by transparent source wrappers.
///
/// `cfg` is resolved before code generation and bank placement has no generic
/// backend representation yet, so downstream analysis traverses the wrapped
/// declaration rather than silently omitting it.
pub(crate) fn unwrapped_declaration(declaration: &Declaration) -> &Declaration {
    match declaration {
        Declaration::Cfg { declaration, .. } | Declaration::Bank { declaration, .. } => {
            unwrapped_declaration(declaration)
        }
        declaration => declaration,
    }
}
