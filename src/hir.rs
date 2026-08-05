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
    pub attrs: Vec<String>,
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
    /// The first deterministic reason a `@comptime` body cannot be evaluated.
    pub comptime_rejection: Option<String>,
}

impl HirProgram {
    pub fn from_ast(program: &Program) -> Result<Self, Diagnostic> {
        let comptime_functions = program
            .declarations
            .iter()
            .filter_map(function_declaration)
            .filter(|function| {
                has_attr(&function.attrs, "comptime") && !has_attr(&function.attrs, "no-comptime")
            })
            .map(|function| function.name.clone())
            .collect::<HashSet<_>>();
        let mutable_globals = program
            .declarations
            .iter()
            .filter_map(global_declaration_name)
            .collect::<HashSet<_>>();
        let ports_and_mmio = program
            .declarations
            .iter()
            .filter_map(port_or_mmio_name)
            .collect::<HashSet<_>>();
        let declarations = program
            .declarations
            .iter()
            .filter_map(|declaration| {
                lower_declaration(
                    declaration,
                    &comptime_functions,
                    &mutable_globals,
                    &ports_and_mmio,
                )
            })
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

fn lower_declaration(
    declaration: &Declaration,
    comptime_functions: &HashSet<String>,
    mutable_globals: &HashSet<String>,
    ports_and_mmio: &HashSet<String>,
) -> Option<HirDeclaration> {
    match declaration {
        Declaration::Cfg { declaration, .. } | Declaration::Bank { declaration, .. } => {
            lower_declaration(
                declaration,
                comptime_functions,
                mutable_globals,
                ports_and_mmio,
            )
        }
        Declaration::Import(_) => None,
        Declaration::Const(decl) => Some(HirDeclaration::Const(HirObject {
            public: decl.public,
            attrs: decl.attrs.clone(),
            name: decl.name.clone(),
            ty: decl.ty.clone(),
        })),
        Declaration::Alias(decl) => Some(HirDeclaration::Alias {
            name: decl.name.clone(),
            ty: decl.ty.clone(),
        }),
        Declaration::Port(decl) => Some(HirDeclaration::Port(HirObject {
            public: decl.public,
            attrs: Vec::new(),
            name: decl.name.clone(),
            ty: decl.ty.clone(),
        })),
        Declaration::Mmio(decl) => Some(HirDeclaration::Mmio {
            object: HirObject {
                public: decl.public,
                attrs: Vec::new(),
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
            attrs: Vec::new(),
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
        Declaration::Function(function) => Some(HirDeclaration::Function(lower_function(
            function,
            comptime_functions,
            mutable_globals,
            ports_and_mmio,
        ))),
    }
}

fn lower_function(
    function: &Function,
    comptime_functions: &HashSet<String>,
    mutable_globals: &HashSet<String>,
    ports_and_mmio: &HashSet<String>,
) -> HirFunction {
    HirFunction {
        sig: lower_function_sig(
            function.public,
            &function.name,
            &function.params,
            &function.return_type,
        ),
        attrs: function.attrs.clone(),
        body: function.body.clone(),
        analysis: analyze_function(
            function,
            comptime_functions,
            mutable_globals,
            ports_and_mmio,
        ),
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

fn analyze_function(
    function: &Function,
    comptime_functions: &HashSet<String>,
    mutable_globals: &HashSet<String>,
    ports_and_mmio: &HashSet<String>,
) -> HirFunctionAnalysis {
    let mut analysis = HirFunctionAnalysis::default();
    analyze_stmts(&function.body, &function.name, &mut analysis);
    if function
        .body
        .iter()
        .any(|stmt| is_tail_call_to(stmt, &function.name))
    {
        analysis.tail_recursive = true;
    }
    if has_attr(&function.attrs, "comptime") && !has_attr(&function.attrs, "no-comptime") {
        analysis.comptime_rejection = comptime_rejection(
            function,
            comptime_functions,
            mutable_globals,
            ports_and_mmio,
        );
    }
    analysis
}

fn function_declaration(declaration: &Declaration) -> Option<&Function> {
    match declaration {
        Declaration::Function(function) => Some(function),
        Declaration::Cfg { declaration, .. } | Declaration::Bank { declaration, .. } => {
            function_declaration(declaration)
        }
        _ => None,
    }
}

fn global_declaration_name(declaration: &Declaration) -> Option<String> {
    match declaration {
        Declaration::Global(global) => Some(global.name.clone()),
        Declaration::Cfg { declaration, .. } | Declaration::Bank { declaration, .. } => {
            global_declaration_name(declaration)
        }
        _ => None,
    }
}

fn port_or_mmio_name(declaration: &Declaration) -> Option<String> {
    match declaration {
        Declaration::Port(port) => Some(port.name.clone()),
        Declaration::Mmio(mmio) => Some(mmio.name.clone()),
        Declaration::Cfg { declaration, .. } | Declaration::Bank { declaration, .. } => {
            port_or_mmio_name(declaration)
        }
        _ => None,
    }
}

fn has_attr(attrs: &[String], wanted: &str) -> bool {
    attrs.iter().any(|attr| attr == wanted)
}

fn comptime_rejection(
    function: &Function,
    comptime_functions: &HashSet<String>,
    mutable_globals: &HashSet<String>,
    ports_and_mmio: &HashSet<String>,
) -> Option<String> {
    if function
        .params
        .iter()
        .any(|param| type_contains_pointer(&param.ty))
    {
        return Some("pointers are not supported".to_owned());
    }
    if function
        .return_type
        .as_ref()
        .is_some_and(type_contains_pointer)
    {
        return Some("pointers are not supported".to_owned());
    }
    fn visit_stmts(
        stmts: &[Stmt],
        function_name: &str,
        comptime_functions: &HashSet<String>,
        mutable_globals: &HashSet<String>,
        ports_and_mmio: &HashSet<String>,
    ) -> Option<&'static str> {
        for stmt in stmts {
            let reason = match stmt {
                Stmt::Assign { .. } => Some("side effects are not supported"),
                Stmt::While { .. } | Stmt::Loop { .. } | Stmt::Break | Stmt::Continue => {
                    Some("loops are not supported")
                }
                Stmt::Asm { .. } => Some("inline asm is not supported"),
                Stmt::Out { .. } => Some("ports are not supported"),
                Stmt::Let { value, .. } | Stmt::Return(Some(value)) | Stmt::Expr(value) => {
                    visit_expr(
                        value,
                        function_name,
                        comptime_functions,
                        mutable_globals,
                        ports_and_mmio,
                    )
                }
                Stmt::If {
                    condition,
                    then_body,
                    else_body,
                } => visit_expr(
                    condition,
                    function_name,
                    comptime_functions,
                    mutable_globals,
                    ports_and_mmio,
                )
                .or_else(|| {
                    visit_stmts(
                        then_body,
                        function_name,
                        comptime_functions,
                        mutable_globals,
                        ports_and_mmio,
                    )
                })
                .or_else(|| {
                    visit_stmts(
                        else_body,
                        function_name,
                        comptime_functions,
                        mutable_globals,
                        ports_and_mmio,
                    )
                }),
                Stmt::Return(None) => None,
            };
            if reason.is_some() {
                return reason;
            }
        }
        None
    }

    fn visit_expr(
        expr: &Expr,
        function_name: &str,
        comptime_functions: &HashSet<String>,
        mutable_globals: &HashSet<String>,
        ports_and_mmio: &HashSet<String>,
    ) -> Option<&'static str> {
        match expr {
            Expr::Ident(name) if mutable_globals.contains(name) => {
                Some("mutable globals are not comptime")
            }
            Expr::Ident(name) if ports_and_mmio.contains(name) => {
                Some("MMIO and ports are not supported")
            }
            Expr::In(_) => Some("ports are not supported"),
            Expr::AddressOf(_)
            | Expr::AddressOfIndex { .. }
            | Expr::AddressOfField { .. }
            | Expr::AddressOfAccess(_)
            | Expr::Deref(_)
            | Expr::BankedPointer { .. } => Some("pointers are not supported"),
            Expr::Call { path, args } => {
                let name = path.last().map(String::as_str);
                if name == Some(function_name) {
                    return Some("recursion is not supported");
                }
                if name.is_some_and(|name| !comptime_functions.contains(name)) {
                    return Some("called function is not @comptime");
                }
                args.iter().find_map(|arg| {
                    visit_expr(
                        arg,
                        function_name,
                        comptime_functions,
                        mutable_globals,
                        ports_and_mmio,
                    )
                })
            }
            Expr::Array(values) => values
                .iter()
                .find_map(|value| {
                    visit_expr(
                        value,
                        function_name,
                        comptime_functions,
                        mutable_globals,
                        ports_and_mmio,
                    )
                })
                .or(Some("aggregate values are not supported")),
            Expr::Index { name, index } => {
                named_root_rejection(name, mutable_globals, ports_and_mmio).or_else(|| {
                    visit_expr(
                        index,
                        function_name,
                        comptime_functions,
                        mutable_globals,
                        ports_and_mmio,
                    )
                })
            }
            Expr::Access(path) => named_root_rejection(&path.root, mutable_globals, ports_and_mmio)
                .or_else(|| {
                    path.segments.iter().find_map(|segment| {
                        if let crate::ast::AccessSegment::Index(index) = segment {
                            visit_expr(
                                index,
                                function_name,
                                comptime_functions,
                                mutable_globals,
                                ports_and_mmio,
                            )
                        } else {
                            None
                        }
                    })
                }),
            Expr::StructInit { .. } => Some("aggregate values are not supported"),
            Expr::Unary { expr, .. } | Expr::Cast { expr, .. } => visit_expr(
                expr,
                function_name,
                comptime_functions,
                mutable_globals,
                ports_and_mmio,
            ),
            Expr::Binary { left, right, .. } => visit_expr(
                left,
                function_name,
                comptime_functions,
                mutable_globals,
                ports_and_mmio,
            )
            .or_else(|| {
                visit_expr(
                    right,
                    function_name,
                    comptime_functions,
                    mutable_globals,
                    ports_and_mmio,
                )
            }),
            Expr::Field { base, .. } if mutable_globals.contains(base) => {
                Some("mutable globals are not comptime")
            }
            Expr::Field { base, .. } if ports_and_mmio.contains(base) => {
                Some("MMIO and ports are not supported")
            }
            Expr::String(_) => Some("pointers are not supported"),
            Expr::Int(_)
            | Expr::TypedInt(_, _)
            | Expr::Bool(_)
            | Expr::Char(_)
            | Expr::Field { .. }
            | Expr::Ident(_) => None,
        }
    }

    fn named_root_rejection(
        name: &str,
        mutable_globals: &HashSet<String>,
        ports_and_mmio: &HashSet<String>,
    ) -> Option<&'static str> {
        if mutable_globals.contains(name) {
            Some("mutable globals are not comptime")
        } else if ports_and_mmio.contains(name) {
            Some("MMIO and ports are not supported")
        } else {
            None
        }
    }

    visit_stmts(
        &function.body,
        &function.name,
        comptime_functions,
        mutable_globals,
        ports_and_mmio,
    )
    .map(str::to_owned)
}

fn type_contains_pointer(ty: &Type) -> bool {
    match ty {
        Type::Ptr(_) | Type::Function { .. } => true,
        Type::Array { element, .. } => type_contains_pointer(element),
        Type::Named(name) if name == "ptr" => true,
        Type::Named(_) => false,
    }
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
        | Expr::BankedPointer { pointer: index, .. }
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
