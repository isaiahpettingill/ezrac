//! AST-independent EZIR v1 serialization and conversion.
//!
//! EZIR deliberately owns every value that it serializes. It does not expose
//! parser source units or source spans, so an EZIR round trip preserves the
//! program tree but not parser locations.

use crate::{
    ast,
    compat::{SourcePathBuf, prelude::*},
    diagnostic::Diagnostic,
};
use serde::{Deserialize, Serialize};

/// The only EZIR schema version currently supported by this module.
pub const EZIR_VERSION: u16 = 1;

/// Optional user-defined metadata attached to an EZIR module.
pub type EzirMetadata = BTreeMap<String, String>;

/// The target requirements recorded by an EZIR module.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct EzirTarget {
    pub address_width_bits: u16,
    pub pointer_address_width_bits: u16,
    pub pointer_storage_width_bits: u16,
    pub native_int_widths: Vec<u16>,
    pub supports_port_io: bool,
}

impl EzirTarget {
    fn validate(&self) -> Result<(), Diagnostic> {
        for (name, width) in [
            ("address_width_bits", self.address_width_bits),
            (
                "pointer_address_width_bits",
                self.pointer_address_width_bits,
            ),
            (
                "pointer_storage_width_bits",
                self.pointer_storage_width_bits,
            ),
        ] {
            if !(1..=64).contains(&width) {
                return Err(Diagnostic::new(format!(
                    "EZIR target `{name}` must be between 1 and 64 bits, got {width}"
                )));
            }
        }

        if self.pointer_address_width_bits > self.address_width_bits {
            return Err(Diagnostic::new(format!(
                "EZIR target pointer address width {} exceeds address width {}",
                self.pointer_address_width_bits, self.address_width_bits
            )));
        }
        if self.pointer_storage_width_bits < self.pointer_address_width_bits {
            return Err(Diagnostic::new(format!(
                "EZIR target pointer storage width {} is smaller than pointer address width {}",
                self.pointer_storage_width_bits, self.pointer_address_width_bits
            )));
        }
        if self.native_int_widths.is_empty() {
            return Err(Diagnostic::new(
                "EZIR target native_int_widths must not be empty",
            ));
        }

        let mut widths = BTreeMap::new();
        for width in &self.native_int_widths {
            if !(1..=64).contains(width) {
                return Err(Diagnostic::new(format!(
                    "EZIR target native integer widths must be between 1 and 64 bits, got {width}"
                )));
            }
            if widths.insert(*width, ()).is_some() {
                return Err(Diagnostic::new(format!(
                    "EZIR target native_int_widths contains duplicate width {width}"
                )));
            }
        }

        Ok(())
    }
}

/// A complete, owned EZIR v1 module.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct EzirModule {
    pub version: u16,
    pub source: Option<String>,
    pub target: EzirTarget,
    pub declarations: Vec<EzirDeclaration>,
    pub metadata: Option<EzirMetadata>,
}

impl EzirModule {
    /// Converts an AST program to its owned EZIR representation.
    pub fn from_program(program: &ast::Program, target: EzirTarget) -> Self {
        Self {
            version: EZIR_VERSION,
            source: program.source_text.clone(),
            target,
            declarations: program
                .declarations
                .iter()
                .map(EzirDeclaration::from)
                .collect(),
            metadata: None,
        }
    }

    /// Converts this module back to an AST program.
    ///
    /// Source units and statement spans are intentionally empty because EZIR
    /// stores semantic source text only, not parser locations.
    pub fn into_program(self, source_path: SourcePathBuf) -> Result<ast::Program, Diagnostic> {
        self.validate()?;
        Ok(ast::Program {
            source_path,
            source_text: self.source,
            source_units: Vec::new(),
            declarations: self
                .declarations
                .into_iter()
                .map(ast::Declaration::from)
                .collect(),
        })
    }

    /// Checks the schema version, target requirements, and module symbols.
    pub fn validate(&self) -> Result<(), Diagnostic> {
        if self.version != EZIR_VERSION {
            return Err(Diagnostic::new(format!(
                "unsupported EZIR version {}; expected {}",
                self.version, EZIR_VERSION
            )));
        }
        self.target.validate()?;

        let mut symbols = BTreeMap::new();
        for declaration in &self.declarations {
            validate_declaration(declaration, &mut symbols)?;
        }
        Ok(())
    }

    /// Serializes this module as readable JSON text.
    pub fn to_text(&self) -> Result<String, Diagnostic> {
        self.validate()?;
        serde_json::to_string_pretty(self)
            .map_err(|error| Diagnostic::new(format!("failed to serialize EZIR: {error}")))
    }

    /// Parses and validates an EZIR JSON module.
    pub fn from_text(text: &str) -> Result<Self, Diagnostic> {
        let module: Self = serde_json::from_str(text)
            .map_err(|error| Diagnostic::new(format!("invalid EZIR JSON: {error}")))?;
        module.validate()?;
        Ok(module)
    }
}

fn validate_declaration(
    declaration: &EzirDeclaration,
    symbols: &mut BTreeMap<String, ()>,
) -> Result<(), Diagnostic> {
    match declaration {
        EzirDeclaration::Cfg { declaration, .. } | EzirDeclaration::Bank { declaration, .. } => {
            validate_declaration(declaration, symbols)
        }
        EzirDeclaration::Import { .. } => Ok(()),
        declaration => {
            let Some(name) = declaration.name() else {
                return Ok(());
            };
            if symbols.insert(name.to_owned(), ()).is_some() {
                return Err(Diagnostic::new(format!("duplicate EZIR symbol `{name}`")));
            }

            if let EzirDeclaration::Struct { fields, .. } = declaration {
                let mut field_names = BTreeMap::new();
                for field in fields {
                    if field_names.insert(field.name.clone(), ()).is_some() {
                        return Err(Diagnostic::new(format!(
                            "duplicate field `{}` in EZIR struct `{name}`",
                            field.name
                        )));
                    }
                }
            }
            if let EzirDeclaration::Function { params, .. }
            | EzirDeclaration::ExternAsmFunction { params, .. } = declaration
            {
                let mut parameter_names = BTreeMap::new();
                for parameter in params {
                    if parameter_names.insert(parameter.name.clone(), ()).is_some() {
                        return Err(Diagnostic::new(format!(
                            "duplicate parameter `{}` in EZIR function `{name}`",
                            parameter.name
                        )));
                    }
                }
            }
            Ok(())
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EzirDeclaration {
    Cfg {
        predicates: Vec<EzirCfgPredicate>,
        declaration: Box<EzirDeclaration>,
    },
    Bank {
        bank: u32,
        declaration: Box<EzirDeclaration>,
    },
    Import {
        path: String,
    },
    Const {
        public: bool,
        attrs: Vec<String>,
        name: String,
        ty: EzirType,
        value: EzirExpr,
    },
    Alias {
        public: bool,
        name: String,
        ty: EzirType,
    },
    Port {
        public: bool,
        name: String,
        ty: EzirType,
        value: EzirExpr,
    },
    Mmio {
        public: bool,
        volatile: bool,
        name: String,
        ty: EzirType,
        value: EzirExpr,
    },
    Embed {
        public: bool,
        name: String,
        ty: Option<EzirType>,
        source: EzirEmbedSource,
        section: Option<String>,
        align: Option<EzirExpr>,
    },
    Global {
        public: bool,
        name: String,
        ty: EzirType,
        value: EzirExpr,
    },
    Struct {
        public: bool,
        name: String,
        fields: Vec<EzirField>,
    },
    ExternAsmFunction {
        public: bool,
        name: String,
        params: Vec<EzirParam>,
        return_type: Option<EzirType>,
        second_return_type: Option<EzirType>,
    },
    Function {
        public: bool,
        attrs: Vec<String>,
        name: String,
        params: Vec<EzirParam>,
        return_type: Option<EzirType>,
        second_return_type: Option<EzirType>,
        body: Vec<EzirStmt>,
    },
}

impl EzirDeclaration {
    fn name(&self) -> Option<&str> {
        match self {
            Self::Cfg { .. } | Self::Bank { .. } | Self::Import { .. } => None,
            Self::Const { name, .. }
            | Self::Alias { name, .. }
            | Self::Port { name, .. }
            | Self::Mmio { name, .. }
            | Self::Embed { name, .. }
            | Self::Global { name, .. }
            | Self::Struct { name, .. }
            | Self::ExternAsmFunction { name, .. }
            | Self::Function { name, .. } => Some(name),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EzirCfgPredicate {
    Target { value: String },
    TargetFamily { value: String },
    Cpu { value: String },
    Vendor { value: String },
    Os { value: String },
    PointerWidth { value: u16 },
    AddressWidth { value: u16 },
    Feature { value: String },
    Debug,
    Release,
    All { predicates: Vec<EzirCfgPredicate> },
    Any { predicates: Vec<EzirCfgPredicate> },
    Not { predicate: Box<EzirCfgPredicate> },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EzirEmbedSource {
    File { path: String },
    Bytes { values: Vec<EzirExpr> },
    Text { value: String },
    CStr { value: String },
    Repeat { value: EzirExpr, len: EzirExpr },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct EzirField {
    pub name: String,
    pub ty: EzirType,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct EzirParam {
    pub name: String,
    pub ty: EzirType,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct EzirAsmInput {
    pub name: String,
    pub ty: EzirType,
    pub class: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct EzirAsmOutput {
    pub name: String,
    pub ty: EzirType,
    pub class: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct EzirAccessPath {
    pub root: String,
    pub segments: Vec<EzirAccessSegment>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EzirAccessSegment {
    Field { name: String },
    Index { value: Box<EzirExpr> },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EzirStmt {
    Let {
        name: String,
        ty: EzirType,
        value: EzirExpr,
    },
    LetTwo {
        first_name: String,
        first_ty: EzirType,
        second_name: String,
        second_ty: EzirType,
        value: EzirExpr,
    },
    Assign {
        target: EzirPlace,
        op: EzirAssignOp,
        value: EzirExpr,
    },
    If {
        condition: EzirExpr,
        then_body: Vec<EzirStmt>,
        else_body: Vec<EzirStmt>,
    },
    While {
        condition: EzirExpr,
        body: Vec<EzirStmt>,
    },
    Loop {
        body: Vec<EzirStmt>,
    },
    Break,
    Continue,
    Return {
        value: Option<EzirExpr>,
    },
    ReturnTwo {
        first: EzirExpr,
        second: EzirExpr,
    },
    Asm {
        volatile: bool,
        inputs: Vec<EzirAsmInput>,
        outputs: Vec<EzirAsmOutput>,
        clobbers: Vec<String>,
        lines: Vec<String>,
    },
    Out {
        port: String,
        value: EzirExpr,
    },
    Expr {
        value: EzirExpr,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EzirPlace {
    Ident { name: String },
    Index { name: String, index: Box<EzirExpr> },
    Field { base: String, field: String },
    Access { path: EzirAccessPath },
    Deref { pointer: Box<EzirExpr> },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EzirAssignOp {
    Set,
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EzirExpr {
    Int {
        value: i64,
    },
    TypedInt {
        value: i64,
        ty: EzirType,
    },
    Bool {
        value: bool,
    },
    Char {
        value: u8,
    },
    String {
        value: String,
    },
    Array {
        values: Vec<EzirExpr>,
    },
    Ident {
        name: String,
    },
    In {
        name: String,
    },
    Index {
        name: String,
        index: Box<EzirExpr>,
    },
    Field {
        base: String,
        field: String,
    },
    AddressOfIndex {
        name: String,
        index: Box<EzirExpr>,
    },
    AddressOfField {
        base: String,
        field: String,
    },
    Access {
        path: EzirAccessPath,
    },
    AddressOfAccess {
        path: EzirAccessPath,
    },
    AddressOf {
        name: String,
    },
    StructInit {
        ty: String,
        fields: Vec<EzirFieldValue>,
    },
    Deref {
        pointer: Box<EzirExpr>,
    },
    BankedPointer {
        pointer: Box<EzirExpr>,
        bank: u32,
    },
    Call {
        path: Vec<String>,
        args: Vec<EzirExpr>,
    },
    Unary {
        op: EzirUnaryOp,
        expr: Box<EzirExpr>,
    },
    Binary {
        left: Box<EzirExpr>,
        op: EzirBinaryOp,
        right: Box<EzirExpr>,
    },
    Cast {
        ty: EzirType,
        expr: Box<EzirExpr>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct EzirFieldValue {
    pub name: String,
    pub value: EzirExpr,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EzirUnaryOp {
    Neg,
    BitNot,
    Not,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EzirBinaryOp {
    Mul,
    Div,
    Mod,
    Add,
    Sub,
    Shl,
    Shr,
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Ne,
    BitAnd,
    BitXor,
    BitOr,
    And,
    Or,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EzirType {
    Named {
        name: String,
    },
    Ptr {
        ty: Box<EzirType>,
    },
    Function {
        params: Vec<EzirType>,
        return_type: Option<Box<EzirType>>,
    },
    Array {
        element: Box<EzirType>,
        len: Box<EzirExpr>,
    },
}

impl From<&ast::Declaration> for EzirDeclaration {
    fn from(declaration: &ast::Declaration) -> Self {
        match declaration {
            ast::Declaration::Cfg {
                predicates,
                declaration,
            } => Self::Cfg {
                predicates: predicates.iter().map(EzirCfgPredicate::from).collect(),
                declaration: Box::new(Self::from(declaration.as_ref())),
            },
            ast::Declaration::Bank { bank, declaration } => Self::Bank {
                bank: *bank,
                declaration: Box::new(Self::from(declaration.as_ref())),
            },
            ast::Declaration::Import(path) => Self::Import { path: path.clone() },
            ast::Declaration::Const(value) => Self::Const {
                public: value.public,
                attrs: value.attrs.clone(),
                name: value.name.clone(),
                ty: EzirType::from(&value.ty),
                value: EzirExpr::from(&value.value),
            },
            ast::Declaration::Alias(value) => Self::Alias {
                public: value.public,
                name: value.name.clone(),
                ty: EzirType::from(&value.ty),
            },
            ast::Declaration::Port(value) => Self::Port {
                public: value.public,
                name: value.name.clone(),
                ty: EzirType::from(&value.ty),
                value: EzirExpr::from(&value.value),
            },
            ast::Declaration::Mmio(value) => Self::Mmio {
                public: value.public,
                volatile: value.volatile,
                name: value.name.clone(),
                ty: EzirType::from(&value.ty),
                value: EzirExpr::from(&value.value),
            },
            ast::Declaration::Embed(value) => Self::Embed {
                public: value.public,
                name: value.name.clone(),
                ty: value.ty.as_ref().map(EzirType::from),
                source: EzirEmbedSource::from(&value.source),
                section: value.section.clone(),
                align: value.align.as_ref().map(EzirExpr::from),
            },
            ast::Declaration::Global(value) => Self::Global {
                public: value.public,
                name: value.name.clone(),
                ty: EzirType::from(&value.ty),
                value: EzirExpr::from(&value.value),
            },
            ast::Declaration::Struct(value) => Self::Struct {
                public: value.public,
                name: value.name.clone(),
                fields: value.fields.iter().map(EzirField::from).collect(),
            },
            ast::Declaration::ExternAsmFunction(value) => Self::ExternAsmFunction {
                public: value.public,
                name: value.name.clone(),
                params: value.params.iter().map(EzirParam::from).collect(),
                return_type: value.return_type.as_ref().map(EzirType::from),
                second_return_type: value.second_return_type.as_ref().map(EzirType::from),
            },
            ast::Declaration::Function(value) => Self::Function {
                public: value.public,
                attrs: value.attrs.clone(),
                name: value.name.clone(),
                params: value.params.iter().map(EzirParam::from).collect(),
                return_type: value.return_type.as_ref().map(EzirType::from),
                second_return_type: value.second_return_type.as_ref().map(EzirType::from),
                body: value.body.iter().map(EzirStmt::from).collect(),
            },
        }
    }
}

impl From<&ast::CfgPredicate> for EzirCfgPredicate {
    fn from(predicate: &ast::CfgPredicate) -> Self {
        match predicate {
            ast::CfgPredicate::Target(value) => Self::Target {
                value: value.clone(),
            },
            ast::CfgPredicate::TargetFamily(value) => Self::TargetFamily {
                value: value.clone(),
            },
            ast::CfgPredicate::Cpu(value) => Self::Cpu {
                value: value.clone(),
            },
            ast::CfgPredicate::Vendor(value) => Self::Vendor {
                value: value.clone(),
            },
            ast::CfgPredicate::Os(value) => Self::Os {
                value: value.clone(),
            },
            ast::CfgPredicate::PointerWidth(value) => Self::PointerWidth { value: *value },
            ast::CfgPredicate::AddressWidth(value) => Self::AddressWidth { value: *value },
            ast::CfgPredicate::Feature(value) => Self::Feature {
                value: value.clone(),
            },
            ast::CfgPredicate::Debug => Self::Debug,
            ast::CfgPredicate::Release => Self::Release,
            ast::CfgPredicate::All(predicates) => Self::All {
                predicates: predicates.iter().map(Self::from).collect(),
            },
            ast::CfgPredicate::Any(predicates) => Self::Any {
                predicates: predicates.iter().map(Self::from).collect(),
            },
            ast::CfgPredicate::Not(predicate) => Self::Not {
                predicate: Box::new(Self::from(predicate.as_ref())),
            },
        }
    }
}

impl From<&ast::EmbedSource> for EzirEmbedSource {
    fn from(source: &ast::EmbedSource) -> Self {
        match source {
            ast::EmbedSource::File(path) => Self::File { path: path.clone() },
            ast::EmbedSource::Bytes(values) => Self::Bytes {
                values: values.iter().map(EzirExpr::from).collect(),
            },
            ast::EmbedSource::Text(value) => Self::Text {
                value: value.clone(),
            },
            ast::EmbedSource::CStr(value) => Self::CStr {
                value: value.clone(),
            },
            ast::EmbedSource::Repeat { value, len } => Self::Repeat {
                value: EzirExpr::from(value),
                len: EzirExpr::from(len),
            },
        }
    }
}

impl From<&ast::FieldDecl> for EzirField {
    fn from(field: &ast::FieldDecl) -> Self {
        Self {
            name: field.name.clone(),
            ty: EzirType::from(&field.ty),
        }
    }
}

impl From<&ast::Param> for EzirParam {
    fn from(param: &ast::Param) -> Self {
        Self {
            name: param.name.clone(),
            ty: EzirType::from(&param.ty),
        }
    }
}

impl From<&ast::AsmInput> for EzirAsmInput {
    fn from(input: &ast::AsmInput) -> Self {
        Self {
            name: input.name.clone(),
            ty: EzirType::from(&input.ty),
            class: input.class.clone(),
        }
    }
}

impl From<&ast::AsmOutput> for EzirAsmOutput {
    fn from(output: &ast::AsmOutput) -> Self {
        Self {
            name: output.name.clone(),
            ty: EzirType::from(&output.ty),
            class: output.class.clone(),
        }
    }
}

impl From<&ast::AccessPath> for EzirAccessPath {
    fn from(path: &ast::AccessPath) -> Self {
        Self {
            root: path.root.clone(),
            segments: path.segments.iter().map(EzirAccessSegment::from).collect(),
        }
    }
}

impl From<&ast::AccessSegment> for EzirAccessSegment {
    fn from(segment: &ast::AccessSegment) -> Self {
        match segment {
            ast::AccessSegment::Field(name) => Self::Field { name: name.clone() },
            ast::AccessSegment::Index(value) => Self::Index {
                value: Box::new(EzirExpr::from(value.as_ref())),
            },
        }
    }
}

impl From<&ast::Stmt> for EzirStmt {
    fn from(statement: &ast::Stmt) -> Self {
        match statement {
            ast::Stmt::Let { name, ty, value } => Self::Let {
                name: name.clone(),
                ty: EzirType::from(ty),
                value: EzirExpr::from(value),
            },
            ast::Stmt::LetTwo {
                first_name,
                first_ty,
                second_name,
                second_ty,
                value,
            } => Self::LetTwo {
                first_name: first_name.clone(),
                first_ty: EzirType::from(first_ty),
                second_name: second_name.clone(),
                second_ty: EzirType::from(second_ty),
                value: EzirExpr::from(value),
            },
            ast::Stmt::Assign { target, op, value } => Self::Assign {
                target: EzirPlace::from(target),
                op: EzirAssignOp::from(*op),
                value: EzirExpr::from(value),
            },
            ast::Stmt::If {
                condition,
                then_body,
                else_body,
            } => Self::If {
                condition: EzirExpr::from(condition),
                then_body: then_body.iter().map(EzirStmt::from).collect(),
                else_body: else_body.iter().map(EzirStmt::from).collect(),
            },
            ast::Stmt::While { condition, body } => Self::While {
                condition: EzirExpr::from(condition),
                body: body.iter().map(EzirStmt::from).collect(),
            },
            ast::Stmt::Loop { body } => Self::Loop {
                body: body.iter().map(EzirStmt::from).collect(),
            },
            ast::Stmt::Break => Self::Break,
            ast::Stmt::Continue => Self::Continue,
            ast::Stmt::Return(value) => Self::Return {
                value: value.as_ref().map(EzirExpr::from),
            },
            ast::Stmt::ReturnTwo { first, second } => Self::ReturnTwo {
                first: EzirExpr::from(first),
                second: EzirExpr::from(second),
            },
            ast::Stmt::Asm {
                volatile,
                inputs,
                outputs,
                clobbers,
                lines,
            } => Self::Asm {
                volatile: *volatile,
                inputs: inputs.iter().map(EzirAsmInput::from).collect(),
                outputs: outputs.iter().map(EzirAsmOutput::from).collect(),
                clobbers: clobbers.clone(),
                lines: lines.clone(),
            },
            ast::Stmt::Out { port, value } => Self::Out {
                port: port.clone(),
                value: EzirExpr::from(value),
            },
            ast::Stmt::Expr(value) => Self::Expr {
                value: EzirExpr::from(value),
            },
        }
    }
}

impl From<&ast::Place> for EzirPlace {
    fn from(place: &ast::Place) -> Self {
        match place {
            ast::Place::Ident(name) => Self::Ident { name: name.clone() },
            ast::Place::Index { name, index } => Self::Index {
                name: name.clone(),
                index: Box::new(EzirExpr::from(index.as_ref())),
            },
            ast::Place::Field { base, field } => Self::Field {
                base: base.clone(),
                field: field.clone(),
            },
            ast::Place::Access(path) => Self::Access {
                path: EzirAccessPath::from(path),
            },
            ast::Place::Deref(pointer) => Self::Deref {
                pointer: Box::new(EzirExpr::from(pointer.as_ref())),
            },
        }
    }
}

impl From<ast::AssignOp> for EzirAssignOp {
    fn from(op: ast::AssignOp) -> Self {
        match op {
            ast::AssignOp::Set => Self::Set,
            ast::AssignOp::Add => Self::Add,
            ast::AssignOp::Sub => Self::Sub,
            ast::AssignOp::Mul => Self::Mul,
            ast::AssignOp::Div => Self::Div,
            ast::AssignOp::Mod => Self::Mod,
            ast::AssignOp::BitAnd => Self::BitAnd,
            ast::AssignOp::BitOr => Self::BitOr,
            ast::AssignOp::BitXor => Self::BitXor,
            ast::AssignOp::Shl => Self::Shl,
            ast::AssignOp::Shr => Self::Shr,
        }
    }
}

impl From<&ast::Expr> for EzirExpr {
    fn from(expression: &ast::Expr) -> Self {
        match expression {
            ast::Expr::Int(value) => Self::Int { value: *value },
            ast::Expr::TypedInt(value, ty) => Self::TypedInt {
                value: *value,
                ty: EzirType::from(ty),
            },
            ast::Expr::Bool(value) => Self::Bool { value: *value },
            ast::Expr::Char(value) => Self::Char { value: *value },
            ast::Expr::String(value) => Self::String {
                value: value.clone(),
            },
            ast::Expr::Array(values) => Self::Array {
                values: values.iter().map(EzirExpr::from).collect(),
            },
            ast::Expr::Ident(name) => Self::Ident { name: name.clone() },
            ast::Expr::In(name) => Self::In { name: name.clone() },
            ast::Expr::Index { name, index } => Self::Index {
                name: name.clone(),
                index: Box::new(EzirExpr::from(index.as_ref())),
            },
            ast::Expr::Field { base, field } => Self::Field {
                base: base.clone(),
                field: field.clone(),
            },
            ast::Expr::AddressOfIndex { name, index } => Self::AddressOfIndex {
                name: name.clone(),
                index: Box::new(EzirExpr::from(index.as_ref())),
            },
            ast::Expr::AddressOfField { base, field } => Self::AddressOfField {
                base: base.clone(),
                field: field.clone(),
            },
            ast::Expr::Access(path) => Self::Access {
                path: EzirAccessPath::from(path),
            },
            ast::Expr::AddressOfAccess(path) => Self::AddressOfAccess {
                path: EzirAccessPath::from(path),
            },
            ast::Expr::AddressOf(name) => Self::AddressOf { name: name.clone() },
            ast::Expr::StructInit { ty, fields } => Self::StructInit {
                ty: ty.clone(),
                fields: fields
                    .iter()
                    .map(|(name, value)| EzirFieldValue {
                        name: name.clone(),
                        value: EzirExpr::from(value),
                    })
                    .collect(),
            },
            ast::Expr::Deref(pointer) => Self::Deref {
                pointer: Box::new(EzirExpr::from(pointer.as_ref())),
            },
            ast::Expr::BankedPointer { pointer, bank } => Self::BankedPointer {
                pointer: Box::new(EzirExpr::from(pointer.as_ref())),
                bank: *bank,
            },
            ast::Expr::Call { path, args } => Self::Call {
                path: path.clone(),
                args: args.iter().map(EzirExpr::from).collect(),
            },
            ast::Expr::Unary { op, expr } => Self::Unary {
                op: EzirUnaryOp::from(*op),
                expr: Box::new(EzirExpr::from(expr.as_ref())),
            },
            ast::Expr::Binary { left, op, right } => Self::Binary {
                left: Box::new(EzirExpr::from(left.as_ref())),
                op: EzirBinaryOp::from(*op),
                right: Box::new(EzirExpr::from(right.as_ref())),
            },
            ast::Expr::Cast { ty, expr } => Self::Cast {
                ty: EzirType::from(ty),
                expr: Box::new(EzirExpr::from(expr.as_ref())),
            },
        }
    }
}

impl From<ast::UnaryOp> for EzirUnaryOp {
    fn from(op: ast::UnaryOp) -> Self {
        match op {
            ast::UnaryOp::Neg => Self::Neg,
            ast::UnaryOp::BitNot => Self::BitNot,
            ast::UnaryOp::Not => Self::Not,
        }
    }
}

impl From<ast::BinaryOp> for EzirBinaryOp {
    fn from(op: ast::BinaryOp) -> Self {
        match op {
            ast::BinaryOp::Mul => Self::Mul,
            ast::BinaryOp::Div => Self::Div,
            ast::BinaryOp::Mod => Self::Mod,
            ast::BinaryOp::Add => Self::Add,
            ast::BinaryOp::Sub => Self::Sub,
            ast::BinaryOp::Shl => Self::Shl,
            ast::BinaryOp::Shr => Self::Shr,
            ast::BinaryOp::Lt => Self::Lt,
            ast::BinaryOp::Le => Self::Le,
            ast::BinaryOp::Gt => Self::Gt,
            ast::BinaryOp::Ge => Self::Ge,
            ast::BinaryOp::Eq => Self::Eq,
            ast::BinaryOp::Ne => Self::Ne,
            ast::BinaryOp::BitAnd => Self::BitAnd,
            ast::BinaryOp::BitXor => Self::BitXor,
            ast::BinaryOp::BitOr => Self::BitOr,
            ast::BinaryOp::And => Self::And,
            ast::BinaryOp::Or => Self::Or,
        }
    }
}

impl From<&ast::Type> for EzirType {
    fn from(ty: &ast::Type) -> Self {
        match ty {
            ast::Type::Named(name) => Self::Named { name: name.clone() },
            ast::Type::Ptr(ty) => Self::Ptr {
                ty: Box::new(Self::from(ty.as_ref())),
            },
            ast::Type::Function {
                params,
                return_type,
            } => Self::Function {
                params: params.iter().map(Self::from).collect(),
                return_type: return_type
                    .as_ref()
                    .map(|ty| Box::new(Self::from(ty.as_ref()))),
            },
            ast::Type::Array { element, len } => Self::Array {
                element: Box::new(Self::from(element.as_ref())),
                len: Box::new(EzirExpr::from(len.as_ref())),
            },
        }
    }
}

impl From<EzirDeclaration> for ast::Declaration {
    fn from(declaration: EzirDeclaration) -> Self {
        match declaration {
            EzirDeclaration::Cfg {
                predicates,
                declaration,
            } => ast::Declaration::Cfg {
                predicates: predicates
                    .into_iter()
                    .map(ast::CfgPredicate::from)
                    .collect(),
                declaration: Box::new(ast::Declaration::from(*declaration)),
            },
            EzirDeclaration::Bank { bank, declaration } => ast::Declaration::Bank {
                bank,
                declaration: Box::new(ast::Declaration::from(*declaration)),
            },
            EzirDeclaration::Import { path } => ast::Declaration::Import(path),
            EzirDeclaration::Const {
                public,
                attrs,
                name,
                ty,
                value,
            } => ast::Declaration::Const(ast::ConstDecl {
                public,
                attrs,
                name,
                ty: ast::Type::from(ty),
                value: ast::Expr::from(value),
            }),
            EzirDeclaration::Alias { public, name, ty } => {
                ast::Declaration::Alias(ast::AliasDecl {
                    public,
                    name,
                    ty: ast::Type::from(ty),
                })
            }
            EzirDeclaration::Port {
                public,
                name,
                ty,
                value,
            } => ast::Declaration::Port(ast::PortDecl {
                public,
                name,
                ty: ast::Type::from(ty),
                value: ast::Expr::from(value),
            }),
            EzirDeclaration::Mmio {
                public,
                volatile,
                name,
                ty,
                value,
            } => ast::Declaration::Mmio(ast::MmioDecl {
                public,
                volatile,
                name,
                ty: ast::Type::from(ty),
                value: ast::Expr::from(value),
            }),
            EzirDeclaration::Embed {
                public,
                name,
                ty,
                source,
                section,
                align,
            } => ast::Declaration::Embed(ast::EmbedDecl {
                public,
                name,
                ty: ty.map(ast::Type::from),
                source: ast::EmbedSource::from(source),
                section,
                align: align.map(ast::Expr::from),
            }),
            EzirDeclaration::Global {
                public,
                name,
                ty,
                value,
            } => ast::Declaration::Global(ast::GlobalDecl {
                public,
                name,
                ty: ast::Type::from(ty),
                value: ast::Expr::from(value),
            }),
            EzirDeclaration::Struct {
                public,
                name,
                fields,
            } => ast::Declaration::Struct(ast::StructDecl {
                public,
                name,
                fields: fields.into_iter().map(ast::FieldDecl::from).collect(),
            }),
            EzirDeclaration::ExternAsmFunction {
                public,
                name,
                params,
                return_type,
                second_return_type,
            } => ast::Declaration::ExternAsmFunction(ast::ExternFunction {
                public,
                name,
                params: params.into_iter().map(ast::Param::from).collect(),
                return_type: return_type.map(ast::Type::from),
                second_return_type: second_return_type.map(ast::Type::from),
            }),
            EzirDeclaration::Function {
                public,
                attrs,
                name,
                params,
                return_type,
                second_return_type,
                body,
            } => ast::Declaration::Function(ast::Function {
                public,
                attrs,
                name,
                params: params.into_iter().map(ast::Param::from).collect(),
                return_type: return_type.map(ast::Type::from),
                second_return_type: second_return_type.map(ast::Type::from),
                body: body.into_iter().map(ast::Stmt::from).collect(),
                body_spans: Vec::new(),
            }),
        }
    }
}

impl From<EzirCfgPredicate> for ast::CfgPredicate {
    fn from(predicate: EzirCfgPredicate) -> Self {
        match predicate {
            EzirCfgPredicate::Target { value } => Self::Target(value),
            EzirCfgPredicate::TargetFamily { value } => Self::TargetFamily(value),
            EzirCfgPredicate::Cpu { value } => Self::Cpu(value),
            EzirCfgPredicate::Vendor { value } => Self::Vendor(value),
            EzirCfgPredicate::Os { value } => Self::Os(value),
            EzirCfgPredicate::PointerWidth { value } => Self::PointerWidth(value),
            EzirCfgPredicate::AddressWidth { value } => Self::AddressWidth(value),
            EzirCfgPredicate::Feature { value } => Self::Feature(value),
            EzirCfgPredicate::Debug => Self::Debug,
            EzirCfgPredicate::Release => Self::Release,
            EzirCfgPredicate::All { predicates } => Self::All(
                predicates
                    .into_iter()
                    .map(ast::CfgPredicate::from)
                    .collect(),
            ),
            EzirCfgPredicate::Any { predicates } => Self::Any(
                predicates
                    .into_iter()
                    .map(ast::CfgPredicate::from)
                    .collect(),
            ),
            EzirCfgPredicate::Not { predicate } => {
                Self::Not(Box::new(ast::CfgPredicate::from(*predicate)))
            }
        }
    }
}

impl From<EzirEmbedSource> for ast::EmbedSource {
    fn from(source: EzirEmbedSource) -> Self {
        match source {
            EzirEmbedSource::File { path } => Self::File(path),
            EzirEmbedSource::Bytes { values } => {
                Self::Bytes(values.into_iter().map(ast::Expr::from).collect())
            }
            EzirEmbedSource::Text { value } => Self::Text(value),
            EzirEmbedSource::CStr { value } => Self::CStr(value),
            EzirEmbedSource::Repeat { value, len } => Self::Repeat {
                value: ast::Expr::from(value),
                len: ast::Expr::from(len),
            },
        }
    }
}

impl From<EzirField> for ast::FieldDecl {
    fn from(field: EzirField) -> Self {
        Self {
            name: field.name,
            ty: ast::Type::from(field.ty),
        }
    }
}

impl From<EzirParam> for ast::Param {
    fn from(param: EzirParam) -> Self {
        Self {
            name: param.name,
            ty: ast::Type::from(param.ty),
        }
    }
}

impl From<EzirAsmInput> for ast::AsmInput {
    fn from(input: EzirAsmInput) -> Self {
        Self {
            name: input.name,
            ty: ast::Type::from(input.ty),
            class: input.class,
        }
    }
}

impl From<EzirAsmOutput> for ast::AsmOutput {
    fn from(output: EzirAsmOutput) -> Self {
        Self {
            name: output.name,
            ty: ast::Type::from(output.ty),
            class: output.class,
        }
    }
}

impl From<EzirAccessPath> for ast::AccessPath {
    fn from(path: EzirAccessPath) -> Self {
        Self {
            root: path.root,
            segments: path
                .segments
                .into_iter()
                .map(ast::AccessSegment::from)
                .collect(),
        }
    }
}

impl From<EzirAccessSegment> for ast::AccessSegment {
    fn from(segment: EzirAccessSegment) -> Self {
        match segment {
            EzirAccessSegment::Field { name } => Self::Field(name),
            EzirAccessSegment::Index { value } => Self::Index(Box::new(ast::Expr::from(*value))),
        }
    }
}

impl From<EzirStmt> for ast::Stmt {
    fn from(statement: EzirStmt) -> Self {
        match statement {
            EzirStmt::Let { name, ty, value } => Self::Let {
                name,
                ty: ast::Type::from(ty),
                value: ast::Expr::from(value),
            },
            EzirStmt::LetTwo {
                first_name,
                first_ty,
                second_name,
                second_ty,
                value,
            } => Self::LetTwo {
                first_name,
                first_ty: ast::Type::from(first_ty),
                second_name,
                second_ty: ast::Type::from(second_ty),
                value: ast::Expr::from(value),
            },
            EzirStmt::Assign { target, op, value } => Self::Assign {
                target: ast::Place::from(target),
                op: ast::AssignOp::from(op),
                value: ast::Expr::from(value),
            },
            EzirStmt::If {
                condition,
                then_body,
                else_body,
            } => Self::If {
                condition: ast::Expr::from(condition),
                then_body: then_body.into_iter().map(ast::Stmt::from).collect(),
                else_body: else_body.into_iter().map(ast::Stmt::from).collect(),
            },
            EzirStmt::While { condition, body } => Self::While {
                condition: ast::Expr::from(condition),
                body: body.into_iter().map(ast::Stmt::from).collect(),
            },
            EzirStmt::Loop { body } => Self::Loop {
                body: body.into_iter().map(ast::Stmt::from).collect(),
            },
            EzirStmt::Break => Self::Break,
            EzirStmt::Continue => Self::Continue,
            EzirStmt::Return { value } => Self::Return(value.map(ast::Expr::from)),
            EzirStmt::ReturnTwo { first, second } => Self::ReturnTwo {
                first: ast::Expr::from(first),
                second: ast::Expr::from(second),
            },
            EzirStmt::Asm {
                volatile,
                inputs,
                outputs,
                clobbers,
                lines,
            } => Self::Asm {
                volatile,
                inputs: inputs.into_iter().map(ast::AsmInput::from).collect(),
                outputs: outputs.into_iter().map(ast::AsmOutput::from).collect(),
                clobbers,
                lines,
            },
            EzirStmt::Out { port, value } => Self::Out {
                port,
                value: ast::Expr::from(value),
            },
            EzirStmt::Expr { value } => Self::Expr(ast::Expr::from(value)),
        }
    }
}

impl From<EzirPlace> for ast::Place {
    fn from(place: EzirPlace) -> Self {
        match place {
            EzirPlace::Ident { name } => Self::Ident(name),
            EzirPlace::Index { name, index } => Self::Index {
                name,
                index: Box::new(ast::Expr::from(*index)),
            },
            EzirPlace::Field { base, field } => Self::Field { base, field },
            EzirPlace::Access { path } => Self::Access(ast::AccessPath::from(path)),
            EzirPlace::Deref { pointer } => Self::Deref(Box::new(ast::Expr::from(*pointer))),
        }
    }
}

impl From<EzirAssignOp> for ast::AssignOp {
    fn from(op: EzirAssignOp) -> Self {
        match op {
            EzirAssignOp::Set => Self::Set,
            EzirAssignOp::Add => Self::Add,
            EzirAssignOp::Sub => Self::Sub,
            EzirAssignOp::Mul => Self::Mul,
            EzirAssignOp::Div => Self::Div,
            EzirAssignOp::Mod => Self::Mod,
            EzirAssignOp::BitAnd => Self::BitAnd,
            EzirAssignOp::BitOr => Self::BitOr,
            EzirAssignOp::BitXor => Self::BitXor,
            EzirAssignOp::Shl => Self::Shl,
            EzirAssignOp::Shr => Self::Shr,
        }
    }
}

impl From<EzirExpr> for ast::Expr {
    fn from(expression: EzirExpr) -> Self {
        match expression {
            EzirExpr::Int { value } => Self::Int(value),
            EzirExpr::TypedInt { value, ty } => Self::TypedInt(value, ast::Type::from(ty)),
            EzirExpr::Bool { value } => Self::Bool(value),
            EzirExpr::Char { value } => Self::Char(value),
            EzirExpr::String { value } => Self::String(value),
            EzirExpr::Array { values } => {
                Self::Array(values.into_iter().map(ast::Expr::from).collect())
            }
            EzirExpr::Ident { name } => Self::Ident(name),
            EzirExpr::In { name } => Self::In(name),
            EzirExpr::Index { name, index } => Self::Index {
                name,
                index: Box::new(ast::Expr::from(*index)),
            },
            EzirExpr::Field { base, field } => Self::Field { base, field },
            EzirExpr::AddressOfIndex { name, index } => Self::AddressOfIndex {
                name,
                index: Box::new(ast::Expr::from(*index)),
            },
            EzirExpr::AddressOfField { base, field } => Self::AddressOfField { base, field },
            EzirExpr::Access { path } => Self::Access(ast::AccessPath::from(path)),
            EzirExpr::AddressOfAccess { path } => {
                Self::AddressOfAccess(ast::AccessPath::from(path))
            }
            EzirExpr::AddressOf { name } => Self::AddressOf(name),
            EzirExpr::StructInit { ty, fields } => Self::StructInit {
                ty,
                fields: fields
                    .into_iter()
                    .map(|field| (field.name, ast::Expr::from(field.value)))
                    .collect(),
            },
            EzirExpr::Deref { pointer } => Self::Deref(Box::new(ast::Expr::from(*pointer))),
            EzirExpr::BankedPointer { pointer, bank } => Self::BankedPointer {
                pointer: Box::new(ast::Expr::from(*pointer)),
                bank,
            },
            EzirExpr::Call { path, args } => Self::Call {
                path,
                args: args.into_iter().map(ast::Expr::from).collect(),
            },
            EzirExpr::Unary { op, expr } => Self::Unary {
                op: ast::UnaryOp::from(op),
                expr: Box::new(ast::Expr::from(*expr)),
            },
            EzirExpr::Binary { left, op, right } => Self::Binary {
                left: Box::new(ast::Expr::from(*left)),
                op: ast::BinaryOp::from(op),
                right: Box::new(ast::Expr::from(*right)),
            },
            EzirExpr::Cast { ty, expr } => Self::Cast {
                ty: ast::Type::from(ty),
                expr: Box::new(ast::Expr::from(*expr)),
            },
        }
    }
}

impl From<EzirUnaryOp> for ast::UnaryOp {
    fn from(op: EzirUnaryOp) -> Self {
        match op {
            EzirUnaryOp::Neg => Self::Neg,
            EzirUnaryOp::BitNot => Self::BitNot,
            EzirUnaryOp::Not => Self::Not,
        }
    }
}

impl From<EzirBinaryOp> for ast::BinaryOp {
    fn from(op: EzirBinaryOp) -> Self {
        match op {
            EzirBinaryOp::Mul => Self::Mul,
            EzirBinaryOp::Div => Self::Div,
            EzirBinaryOp::Mod => Self::Mod,
            EzirBinaryOp::Add => Self::Add,
            EzirBinaryOp::Sub => Self::Sub,
            EzirBinaryOp::Shl => Self::Shl,
            EzirBinaryOp::Shr => Self::Shr,
            EzirBinaryOp::Lt => Self::Lt,
            EzirBinaryOp::Le => Self::Le,
            EzirBinaryOp::Gt => Self::Gt,
            EzirBinaryOp::Ge => Self::Ge,
            EzirBinaryOp::Eq => Self::Eq,
            EzirBinaryOp::Ne => Self::Ne,
            EzirBinaryOp::BitAnd => Self::BitAnd,
            EzirBinaryOp::BitXor => Self::BitXor,
            EzirBinaryOp::BitOr => Self::BitOr,
            EzirBinaryOp::And => Self::And,
            EzirBinaryOp::Or => Self::Or,
        }
    }
}

impl From<EzirType> for ast::Type {
    fn from(ty: EzirType) -> Self {
        match ty {
            EzirType::Named { name } => Self::Named(name),
            EzirType::Ptr { ty } => Self::Ptr(Box::new(ast::Type::from(*ty))),
            EzirType::Function {
                params,
                return_type,
            } => Self::Function {
                params: params.into_iter().map(ast::Type::from).collect(),
                return_type: return_type.map(|ty| Box::new(ast::Type::from(*ty))),
            },
            EzirType::Array { element, len } => Self::Array {
                element: Box::new(ast::Type::from(*element)),
                len: Box::new(ast::Expr::from(*len)),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target() -> EzirTarget {
        EzirTarget {
            address_width_bits: 24,
            pointer_address_width_bits: 24,
            pointer_storage_width_bits: 24,
            native_int_widths: vec![8, 16, 24],
            supports_port_io: true,
        }
    }

    #[test]
    fn json_and_program_roundtrip() {
        let module = EzirModule {
            version: EZIR_VERSION,
            source: Some("fn main() {}".to_owned()),
            target: target(),
            declarations: vec![EzirDeclaration::Function {
                public: true,
                attrs: vec!["entry".to_owned()],
                name: "main".to_owned(),
                params: Vec::new(),
                return_type: None,
                second_return_type: None,
                body: vec![EzirStmt::Return { value: None }],
            }],
            metadata: Some(BTreeMap::from([("producer".to_owned(), "test".to_owned())])),
        };

        let text = module.to_text().expect("EZIR should serialize");
        assert!(text.contains("\"kind\": \"function\""));
        let decoded = EzirModule::from_text(&text).expect("EZIR should deserialize");
        assert_eq!(module, decoded);

        let program = decoded
            .into_program(SourcePathBuf::from("main.ezra"))
            .expect("EZIR should convert to an AST program");
        assert_eq!(program.source_text, Some("fn main() {}".to_owned()));
        assert!(program.source_units.is_empty());
        assert_eq!(program.declarations.len(), 1);
        assert!(matches!(
            program.declarations[0],
            ast::Declaration::Function(_)
        ));
    }

    #[test]
    fn rejects_unsupported_version() {
        let module = EzirModule {
            version: EZIR_VERSION,
            source: None,
            target: target(),
            declarations: Vec::new(),
            metadata: None,
        };
        let text = module
            .to_text()
            .expect("EZIR should serialize")
            .replace("\"version\": 1", "\"version\": 2");
        let error = EzirModule::from_text(&text).expect_err("version 2 must be rejected");
        assert!(error.message.contains("unsupported EZIR version"));
    }
}
