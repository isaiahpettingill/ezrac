use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq)]
pub struct Program {
    pub source_path: PathBuf,
    pub declarations: Vec<Declaration>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Declaration {
    Import(String),
    Const(ConstDecl),
    Alias(AliasDecl),
    Port(PortDecl),
    Mmio(MmioDecl),
    Embed(EmbedDecl),
    Global(GlobalDecl),
    Struct(StructDecl),
    ExternAsmFunction(ExternFunction),
    Function(Function),
}

#[derive(Clone, Debug, PartialEq)]
pub struct ConstDecl {
    pub public: bool,
    pub name: String,
    pub ty: Type,
    pub value: Expr,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AliasDecl {
    pub public: bool,
    pub name: String,
    pub ty: Type,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PortDecl {
    pub public: bool,
    pub name: String,
    pub value: Expr,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MmioDecl {
    pub public: bool,
    pub volatile: bool,
    pub name: String,
    pub ty: Type,
    pub value: Expr,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EmbedDecl {
    pub public: bool,
    pub name: String,
    pub source: EmbedSource,
    pub section: Option<String>,
    pub align: Option<Expr>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum EmbedSource {
    File(String),
    Bytes(Vec<Expr>),
    Text(String),
    CStr(String),
    Repeat { value: Expr, len: Expr },
}

#[derive(Clone, Debug, PartialEq)]
pub struct GlobalDecl {
    pub public: bool,
    pub name: String,
    pub ty: Type,
    pub value: Expr,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StructDecl {
    pub public: bool,
    pub name: String,
    pub fields: Vec<FieldDecl>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FieldDecl {
    pub name: String,
    pub ty: Type,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Function {
    pub public: bool,
    pub attrs: Vec<String>,
    pub name: String,
    pub params: Vec<Param>,
    pub return_type: Option<Type>,
    pub body: Vec<Stmt>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ExternFunction {
    pub public: bool,
    pub name: String,
    pub params: Vec<Param>,
    pub return_type: Option<Type>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Param {
    pub name: String,
    pub ty: Type,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Stmt {
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
        then_body: Vec<Stmt>,
        else_body: Vec<Stmt>,
    },
    While {
        condition: Expr,
        body: Vec<Stmt>,
    },
    Loop {
        body: Vec<Stmt>,
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
    Out {
        port: String,
        value: Expr,
    },
    Expr(Expr),
}

#[derive(Clone, Debug, PartialEq)]
pub struct AsmInput {
    pub name: String,
    pub ty: Type,
    pub class: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AsmOutput {
    pub name: String,
    pub ty: Type,
    pub class: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Place {
    Ident(String),
    Index { name: String, index: Box<Expr> },
    Field { base: String, field: String },
    Access(AccessPath),
    Deref(Box<Expr>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AssignOp {
    Set,
    Add,
    Sub,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    Int(i64),
    TypedInt(i64, Type),
    Bool(bool),
    Char(u8),
    String(String),
    Array(Vec<Expr>),
    Ident(String),
    In(String),
    Index {
        name: String,
        index: Box<Expr>,
    },
    Field {
        base: String,
        field: String,
    },
    AddressOfIndex {
        name: String,
        index: Box<Expr>,
    },
    AddressOfField {
        base: String,
        field: String,
    },
    Access(AccessPath),
    AddressOfAccess(AccessPath),
    AddressOf(String),
    StructInit {
        ty: String,
        fields: Vec<(String, Expr)>,
    },
    Deref(Box<Expr>),
    Call {
        path: Vec<String>,
        args: Vec<Expr>,
    },
    Unary {
        op: UnaryOp,
        expr: Box<Expr>,
    },
    Binary {
        left: Box<Expr>,
        op: BinaryOp,
        right: Box<Expr>,
    },
    Cast {
        ty: Type,
        expr: Box<Expr>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct AccessPath {
    pub root: String,
    pub segments: Vec<AccessSegment>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum AccessSegment {
    Field(String),
    Index(Box<Expr>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnaryOp {
    Neg,
    BitNot,
    Not,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BinaryOp {
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Type {
    Named(String),
    Ptr(Box<Type>),
    Array { element: Box<Type>, len: String },
}

impl Program {
    pub fn main_function(&self) -> Option<&Function> {
        self.declarations
            .iter()
            .find_map(|declaration| match declaration {
                Declaration::Function(function) if function.name == "main" => Some(function),
                _ => None,
            })
    }
}
