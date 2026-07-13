use crate::{
    asm::AssemblyOptions,
    ast::{AsmInput, AsmOutput, AssignOp, Expr, Place, Program, Type},
    diagnostic::Diagnostic,
    hir::HirProgram,
};

pub mod diagnostics;
pub mod dump;
pub mod ez80;
pub mod model;
pub mod optimize;

#[derive(Clone, Debug, PartialEq)]
pub struct TbirProgram {
    pub source: std::path::PathBuf,
    pub target: TbirTarget,
    pub memory: TbirMemoryModel,
    pub declarations: Vec<TbirDeclaration>,
    pub optimizations: TbirOptimizationReport,
    pub lowered_program: Program,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TbirTarget {
    pub name: String,
    pub pointer_width_bits: u8,
    pub native_int_widths: Vec<u8>,
    pub prefer_code_size: bool,
    pub has_cache: bool,
    pub supports_port_io: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TbirMemoryModel {
    pub address_width_bits: u8,
    pub regions: Vec<TbirMemoryRegion>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TbirMemoryRegion {
    pub name: String,
    pub start: u32,
    pub size: u32,
    pub access: TbirAccess,
    pub volatile: bool,
    pub executable: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TbirAccess {
    ReadOnly,
    ReadWrite,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TbirDeclaration {
    Function {
        name: String,
        public: bool,
        attrs: Vec<String>,
        params: Vec<TbirParam>,
        return_type: Option<Type>,
        body: Vec<TbirStmt>,
        effects: Vec<TbirEffect>,
        recursive: bool,
        tail_recursive: bool,
        loop_candidates: usize,
    },
    Object {
        name: String,
        kind: TbirObjectKind,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TbirParam {
    pub name: String,
    pub ty: Type,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TbirStmt {
    Let {
        name: String,
        ty: Type,
        value: Expr,
    },
    Assign {
        target: Place,
        op: AssignOp,
        value: Expr,
    },
    If {
        condition: Expr,
        then_body: Vec<TbirStmt>,
        else_body: Vec<TbirStmt>,
    },
    While {
        condition: Expr,
        body: Vec<TbirStmt>,
    },
    Loop {
        body: Vec<TbirStmt>,
    },
    Break,
    Continue,
    Return(Option<Expr>),
    Asm {
        volatile: bool,
        inputs: Vec<AsmInput>,
        outputs: Vec<AsmOutput>,
        clobbers: Vec<String>,
        lines: Vec<String>,
    },
    PortWrite {
        port: String,
        value: Expr,
    },
    Eval(Expr),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TbirObjectKind {
    Const,
    Port,
    Mmio,
    Embed,
    Global,
    Alias,
    Struct,
    ExternFunction,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TbirEffect {
    Pure,
    VolatileMemory,
    PortIo,
    InlineAsm,
    Call,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TbirOptimizationReport {
    pub constant_folds: usize,
    pub algebraic_simplifications: usize,
    pub constant_propagations: usize,
    pub dead_statements_marked: usize,
    pub inline_candidates: Vec<String>,
    pub tail_call_candidates: Vec<String>,
}

impl TbirProgram {
    pub fn lower(
        hir: &HirProgram,
        lowered_program: &Program,
        options: &AssemblyOptions,
    ) -> Result<Self, Diagnostic> {
        ez80::lower(hir, lowered_program, options)
    }

    pub fn for_ez80(
        hir: &HirProgram,
        lowered_program: &Program,
        options: &AssemblyOptions,
    ) -> Result<Self, Diagnostic> {
        Self::lower(hir, lowered_program, options)
    }

    pub fn dump_text(&self) -> String {
        dump::text(self)
    }
}

#[cfg(test)]
mod tests;
