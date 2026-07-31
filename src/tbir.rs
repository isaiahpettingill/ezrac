use crate::{
    asm::AssemblyOptions,
    ast::{AsmInput, AsmOutput, AssignOp, Expr, Place, Program, Type},
    compat::{SourcePathBuf, prelude::*},
    diagnostic::Diagnostic,
    hir::HirProgram,
};

pub mod bit_ops;
pub mod diagnostics;
pub mod dump;
pub mod ez80;
pub mod model;
pub mod optimize;
pub mod provenance;
pub mod range;

#[derive(Clone, Debug, PartialEq)]
pub struct TbirProgram {
    pub source: SourcePathBuf,
    pub target: TbirTarget,
    pub memory: TbirMemoryModel,
    pub objects: Vec<TbirMemoryObject>,
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TbirMemoryObject {
    pub name: String,
    pub kind: TbirObjectKind,
    pub ty: Type,
    pub address: u32,
    pub size: u32,
    pub region: Option<String>,
    pub access: TbirAccess,
    pub volatile: bool,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TbirOptimizationKind {
    StrengthReduction,
    CopyPropagation,
    CommonSubexpression,
    LoopInvariantCodeMotion,
    MemoryReadLicm,
    Inline,
    TailCall,
    TailRecursion,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TbirOptimizationOutcome {
    Applied,
    Rejected,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TbirOptimizationDecision {
    pub kind: TbirOptimizationKind,
    pub caller: Option<String>,
    pub callee: String,
    pub outcome: TbirOptimizationOutcome,
    pub reason: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TbirOptimizationReport {
    pub constant_folds: usize,
    pub algebraic_simplifications: usize,
    pub strength_reductions: usize,
    pub constant_propagations: usize,
    pub copy_propagations: usize,
    pub common_subexpressions: usize,
    pub loop_invariants_hoisted: usize,
    pub named_memory_reads_hoisted: usize,
    pub dead_statements_marked: usize,
    pub decisions: Vec<TbirOptimizationDecision>,
}

impl TbirOptimizationReport {
    pub fn inline_function_names(&self) -> HashSet<String> {
        self.decisions
            .iter()
            .filter(|decision| {
                decision.kind == TbirOptimizationKind::Inline
                    && decision.outcome == TbirOptimizationOutcome::Applied
            })
            .map(|decision| decision.callee.clone())
            .collect()
    }

    pub fn tail_call_edges(&self) -> HashSet<(String, String)> {
        self.decisions
            .iter()
            .filter(|decision| {
                decision.kind == TbirOptimizationKind::TailCall
                    && decision.outcome == TbirOptimizationOutcome::Applied
            })
            .filter_map(|decision| {
                decision
                    .caller
                    .as_ref()
                    .map(|caller| (caller.clone(), decision.callee.clone()))
            })
            .collect()
    }
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
