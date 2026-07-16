use crate::{
    ast::{Declaration, Expr, Function, Program, Stmt, Type},
    compat::{SourcePathBuf, prelude::*},
    diagnostic::Diagnostic,
};

pub mod dump;

#[derive(Clone, Debug, PartialEq)]
pub struct HirProgram {
    pub source_path: SourcePathBuf,
    pub declarations: Vec<HirDeclaration>,
    pub analysis: HirAnalysis,
}

#[derive(Clone, Debug, PartialEq)]
pub enum HirDeclaration {
    Const(HirObject),
    Alias {
        name: String,
        ty: Type,
    },
    Port(HirObject),
    Mmio {
        object: HirObject,
        volatile: bool,
    },
    Embed {
        name: String,
        section: Option<String>,
    },
    Global(HirObject),
    Struct {
        name: String,
        fields: Vec<HirField>,
    },
    ExternFunction(HirFunctionSig),
    Function(HirFunction),
}

#[derive(Clone, Debug, PartialEq)]
pub struct HirObject {
    pub public: bool,
    pub name: String,
    pub ty: Type,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HirField {
    pub name: String,
    pub ty: Type,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HirFunctionSig {
    pub public: bool,
    pub name: String,
    pub params: Vec<HirParam>,
    pub return_type: Option<Type>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HirParam {
    pub name: String,
    pub ty: Type,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HirFunction {
    pub sig: HirFunctionSig,
    pub attrs: Vec<String>,
    pub body: Vec<Stmt>,
    pub analysis: HirFunctionAnalysis,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct HirAnalysis {
    pub function_count: usize,
    pub shared_library_candidate: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct HirFunctionAnalysis {
    pub recursive: bool,
    pub tail_recursive: bool,
    pub tail_call_candidates: Vec<String>,
    pub loop_candidates: usize,
}

impl HirProgram {
    pub fn from_ast(program: &Program) -> Result<Self, Diagnostic> {
        let declarations = program
            .declarations
            .iter()
            .filter_map(lower_declaration)
            .collect::<Vec<_>>();
        let function_count = declarations
            .iter()
            .filter(|decl| matches!(decl, HirDeclaration::Function(_)))
            .count();
        Ok(Self {
            source_path: program.source_path.clone(),
            declarations,
            analysis: HirAnalysis {
                function_count,
                shared_library_candidate: program.main_function().is_none(),
            },
        })
    }

    pub fn dump_text(&self) -> String {
        dump::text(self)
    }
}

fn lower_declaration(declaration: &Declaration) -> Option<HirDeclaration> {
    match declaration {
        Declaration::Cfg { declaration, .. } => lower_declaration(declaration),
        Declaration::Import(_) => None,
        Declaration::Const(decl) => Some(HirDeclaration::Const(HirObject {
            public: decl.public,
            name: decl.name.clone(),
            ty: decl.ty.clone(),
        })),
        Declaration::Alias(decl) => Some(HirDeclaration::Alias {
            name: decl.name.clone(),
            ty: decl.ty.clone(),
        }),
        Declaration::Port(decl) => Some(HirDeclaration::Port(HirObject {
            public: decl.public,
            name: decl.name.clone(),
            ty: decl.ty.clone(),
        })),
        Declaration::Mmio(decl) => Some(HirDeclaration::Mmio {
            object: HirObject {
                public: decl.public,
                name: decl.name.clone(),
                ty: decl.ty.clone(),
            },
            volatile: decl.volatile,
        }),
        Declaration::Embed(decl) => Some(HirDeclaration::Embed {
            name: decl.name.clone(),
            section: decl.section.clone(),
        }),
        Declaration::Global(decl) => Some(HirDeclaration::Global(HirObject {
            public: decl.public,
            name: decl.name.clone(),
            ty: decl.ty.clone(),
        })),
        Declaration::Struct(decl) => Some(HirDeclaration::Struct {
            name: decl.name.clone(),
            fields: decl
                .fields
                .iter()
                .map(|field| HirField {
                    name: field.name.clone(),
                    ty: field.ty.clone(),
                })
                .collect(),
        }),
        Declaration::ExternAsmFunction(function) => {
            Some(HirDeclaration::ExternFunction(lower_function_sig(
                function.public,
                &function.name,
                &function.params,
                &function.return_type,
            )))
        }
        Declaration::Function(function) => Some(HirDeclaration::Function(lower_function(function))),
    }
}

fn lower_function(function: &Function) -> HirFunction {
    HirFunction {
        sig: lower_function_sig(
            function.public,
            &function.name,
            &function.params,
            &function.return_type,
        ),
        attrs: function.attrs.clone(),
        body: function.body.clone(),
        analysis: analyze_function(function),
    }
}

fn lower_function_sig(
    public: bool,
    name: &str,
    params: &[crate::ast::Param],
    return_type: &Option<Type>,
) -> HirFunctionSig {
    HirFunctionSig {
        public,
        name: name.to_owned(),
        params: params
            .iter()
            .map(|param| HirParam {
                name: param.name.clone(),
                ty: param.ty.clone(),
            })
            .collect(),
        return_type: return_type.clone(),
    }
}

fn analyze_function(function: &Function) -> HirFunctionAnalysis {
    let mut analysis = HirFunctionAnalysis::default();
    analyze_stmts(&function.body, &function.name, &mut analysis);
    if function
        .body
        .iter()
        .any(|stmt| is_tail_call_to(stmt, &function.name))
    {
        analysis.tail_recursive = true;
    }
    analysis
}

fn analyze_stmts(stmts: &[Stmt], function_name: &str, analysis: &mut HirFunctionAnalysis) {
    for stmt in stmts {
        match stmt {
            Stmt::If {
                condition,
                then_body,
                else_body,
            } => {
                analyze_expr(condition, function_name, analysis);
                analyze_stmts(then_body, function_name, analysis);
                analyze_stmts(else_body, function_name, analysis);
            }
            Stmt::While { condition, body } => {
                analysis.loop_candidates += 1;
                analyze_expr(condition, function_name, analysis);
                analyze_stmts(body, function_name, analysis);
            }
            Stmt::Loop { body } => {
                analysis.loop_candidates += 1;
                analyze_stmts(body, function_name, analysis);
            }
            Stmt::Let { value, .. }
            | Stmt::Assign { value, .. }
            | Stmt::Return(Some(value))
            | Stmt::Out { value, .. }
            | Stmt::Expr(value) => analyze_expr(value, function_name, analysis),
            Stmt::Return(None) | Stmt::Break | Stmt::Continue | Stmt::Asm { .. } => {}
        }
        if let Some(target) = tail_call_target(stmt) {
            analysis.tail_call_candidates.push(target);
        }
    }
}

fn analyze_expr(expr: &Expr, function_name: &str, analysis: &mut HirFunctionAnalysis) {
    match expr {
        Expr::Call { path, args } => {
            if path.last().is_some_and(|name| name == function_name) {
                analysis.recursive = true;
            }
            for arg in args {
                analyze_expr(arg, function_name, analysis);
            }
        }
        Expr::Array(values) => {
            for value in values {
                analyze_expr(value, function_name, analysis);
            }
        }
        Expr::Index { index, .. }
        | Expr::AddressOfIndex { index, .. }
        | Expr::Deref(index)
        | Expr::Unary { expr: index, .. }
        | Expr::Cast { expr: index, .. } => analyze_expr(index, function_name, analysis),
        Expr::Access(path) | Expr::AddressOfAccess(path) => {
            for segment in &path.segments {
                if let crate::ast::AccessSegment::Index(index) = segment {
                    analyze_expr(index, function_name, analysis);
                }
            }
        }
        Expr::StructInit { fields, .. } => {
            for (_, value) in fields {
                analyze_expr(value, function_name, analysis);
            }
        }
        Expr::Binary { left, right, .. } => {
            analyze_expr(left, function_name, analysis);
            analyze_expr(right, function_name, analysis);
        }
        Expr::Int(_)
        | Expr::TypedInt(_, _)
        | Expr::Bool(_)
        | Expr::Char(_)
        | Expr::String(_)
        | Expr::Ident(_)
        | Expr::In(_)
        | Expr::Field { .. }
        | Expr::AddressOfField { .. }
        | Expr::AddressOf(_) => {}
    }
}

fn is_tail_call_to(stmt: &Stmt, function_name: &str) -> bool {
    tail_call_target(stmt).is_some_and(|target| target == function_name)
}

fn tail_call_target(stmt: &Stmt) -> Option<String> {
    let Stmt::Return(Some(Expr::Call { path, .. })) = stmt else {
        return None;
    };
    path.last().cloned()
}

#[cfg(test)]
mod tests;
