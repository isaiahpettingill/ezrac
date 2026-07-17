//! Target-independent preprocessing for parsed assembly syntax.
//!
//! This module expands includes, defines, conditionals, and macros over the
//! structured syntax produced by [`parse_assembly_syntax`]. No preprocessor
//! operation renders and reparses complete assembly lines.

use crate::compat::prelude::*;
use crate::{
    asm::frontend::{
        AssemblyInstruction, AssemblyProgram, LocatedParsedAssemblyItem, ParsedAssembly,
        ParsedAssemblyDataValue, ParsedAssemblyItem, lower_parsed_assembly, parse_assembly_syntax,
    },
    diagnostic::{Diagnostic, SourceLocation},
    workspace::{Workspace, normalize_virtual_path},
};

#[cfg(feature = "std")]
use std::{fs, path::Path};

const DEFAULT_MACRO_DEPTH_LIMIT: usize = 32;

/// Configuration used while preprocessing assembly syntax.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AssemblyPreprocessOptions {
    /// Target triple tested by `%if target("...")`.
    pub target: String,
    /// Assembler CPU name tested by `%if cpu("...")`.
    pub cpu: String,
    /// Feature names tested by `%if feature("...")`.
    pub enabled_features: Vec<String>,
    /// Maximum number of recursively nested macro invocations.
    pub macro_depth_limit: usize,
}

impl AssemblyPreprocessOptions {
    /// Create options without implicitly enabling Cargo features.
    pub fn new(target: impl Into<String>, cpu: impl Into<String>) -> Self {
        Self {
            target: target.into(),
            cpu: cpu.into(),
            enabled_features: Vec::new(),
            macro_depth_limit: DEFAULT_MACRO_DEPTH_LIMIT,
        }
    }

    /// Create options whose feature set reflects the features used to compile
    /// this crate.
    pub fn for_compiled_features(target: impl Into<String>, cpu: impl Into<String>) -> Self {
        let mut options = Self::new(target, cpu);
        for (name, enabled) in [
            ("std", cfg!(feature = "std")),
            ("no-std", cfg!(feature = "no-std")),
            ("test-runner", cfg!(feature = "test-runner")),
            ("intel", cfg!(feature = "intel")),
            ("i8086", cfg!(feature = "i8086")),
            ("z80", cfg!(feature = "z80")),
            ("lr35902", cfg!(feature = "lr35902")),
            ("avr", cfg!(feature = "avr")),
            ("m6800", cfg!(feature = "m6800")),
            ("m68k", cfg!(feature = "m68k")),
            ("mos6502", cfg!(feature = "mos6502")),
            ("mos6502-emulator", cfg!(feature = "mos6502-emulator")),
            ("tms9900", cfg!(feature = "tms9900")),
            ("dcpu", cfg!(feature = "dcpu")),
            ("lsp", cfg!(feature = "lsp")),
        ] {
            if enabled {
                options.enabled_features.push(name.to_owned());
            }
        }
        options
    }

    fn feature_enabled(&self, feature: &str) -> bool {
        self.enabled_features
            .iter()
            .any(|enabled| enabled == feature)
    }
}

impl Default for AssemblyPreprocessOptions {
    fn default() -> Self {
        Self::for_compiled_features(String::new(), String::new())
    }
}

/// An include resolved to an owned canonical source name and UTF-8 source.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedAssemblyInclude {
    pub source_name: String,
    pub source: String,
}

impl ResolvedAssemblyInclude {
    pub fn new(source_name: impl Into<String>, source: impl Into<String>) -> Self {
        Self {
            source_name: source_name.into(),
            source: source.into(),
        }
    }
}

/// Resolves an assembly include relative to the source containing it.
///
/// Implementations must return a stable, canonical source name. Canonical
/// names are used both for diagnostics and include-cycle detection.
pub trait AssemblyIncludeResolver {
    fn resolve_include(
        &self,
        including_source_name: &str,
        include_path: &str,
    ) -> Result<ResolvedAssemblyInclude, Diagnostic>;
}

/// Resolver used by source-only preprocessing. Every include is rejected.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct NullAssemblyIncludeResolver;

impl AssemblyIncludeResolver for NullAssemblyIncludeResolver {
    fn resolve_include(
        &self,
        including_source_name: &str,
        include_path: &str,
    ) -> Result<ResolvedAssemblyInclude, Diagnostic> {
        Err(Diagnostic::new(format!(
            "cannot resolve assembly include `{include_path}` from `{including_source_name}` without an include resolver"
        )))
    }
}

/// The semantic assembly program and the fully expanded syntax it came from.
///
/// `syntax` contains no preprocessing or generic directive nodes. Its item
/// locations retain include provenance, while macro-expanded items use the
/// corresponding invocation location.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreprocessedAssembly {
    pub program: AssemblyProgram,
    pub syntax: ParsedAssembly,
}

/// Parse and preprocess assembly without allowing includes.
pub fn preprocess_assembly(
    source_name: &str,
    source: &str,
    options: AssemblyPreprocessOptions,
) -> Result<PreprocessedAssembly, Diagnostic> {
    preprocess_assembly_with_resolver(source_name, source, &NullAssemblyIncludeResolver, options)
}

/// Explicitly named alias for source-only preprocessing.
pub fn preprocess_assembly_source(
    source_name: &str,
    source: &str,
    options: AssemblyPreprocessOptions,
) -> Result<PreprocessedAssembly, Diagnostic> {
    preprocess_assembly(source_name, source, options)
}

/// Parse, recursively resolve, structurally expand, normalize, and lower an
/// assembly source.
pub fn preprocess_assembly_with_resolver(
    source_name: &str,
    source: &str,
    resolver: &dyn AssemblyIncludeResolver,
    options: AssemblyPreprocessOptions,
) -> Result<PreprocessedAssembly, Diagnostic> {
    let mut preprocessor = AssemblyPreprocessor {
        resolver,
        options: &options,
        defines: BTreeMap::new(),
        macros: BTreeMap::new(),
        include_stack: vec![source_name.to_owned()],
        next_expansion_id: 0,
    };
    let items = preprocessor.process_source(source_name, source, 0)?;
    let syntax = ParsedAssembly {
        source_name: source_name.to_owned(),
        items,
    };
    let program = lower_parsed_assembly(syntax.clone())?;
    Ok(PreprocessedAssembly { program, syntax })
}

/// An alloc-only include resolver backed by an immutable virtual workspace.
#[derive(Clone, Copy, Debug)]
pub struct WorkspaceAssemblyResolver<'a> {
    pub workspace: Workspace<'a>,
}

impl<'a> WorkspaceAssemblyResolver<'a> {
    pub const fn new(workspace: Workspace<'a>) -> Self {
        Self { workspace }
    }
}

impl AssemblyIncludeResolver for WorkspaceAssemblyResolver<'_> {
    fn resolve_include(
        &self,
        including_source_name: &str,
        include_path: &str,
    ) -> Result<ResolvedAssemblyInclude, Diagnostic> {
        let source_name = resolve_virtual_include_path(including_source_name, include_path);
        let bytes = self.workspace.file(&source_name).ok_or_else(|| {
            Diagnostic::new(format!(
                "assembly include `{include_path}` referenced from `{including_source_name}` was not found (resolved as `{source_name}`)"
            ))
        })?;
        let source = core::str::from_utf8(bytes).map_err(|_| {
            Diagnostic::new(format!(
                "workspace assembly include `{source_name}` is not UTF-8"
            ))
        })?;
        Ok(ResolvedAssemblyInclude::new(source_name, source))
    }
}

/// Preprocess an assembly root stored in an alloc-only virtual workspace.
pub fn preprocess_assembly_workspace(
    workspace: &Workspace<'_>,
    root: &str,
    options: AssemblyPreprocessOptions,
) -> Result<PreprocessedAssembly, Diagnostic> {
    let root = normalize_virtual_path(root);
    let bytes = workspace.file(&root).ok_or_else(|| {
        Diagnostic::new(format!(
            "workspace does not contain root assembly source `{root}`"
        ))
    })?;
    let source = core::str::from_utf8(bytes).map_err(|_| {
        Diagnostic::new(format!(
            "workspace root assembly source `{root}` is not UTF-8"
        ))
    })?;
    let resolver = WorkspaceAssemblyResolver::new(*workspace);
    preprocess_assembly_with_resolver(&root, source, &resolver, options)
}

fn resolve_virtual_include_path(including_source_name: &str, include_path: &str) -> String {
    if include_path.starts_with('/') || include_path.starts_with('\\') {
        return normalize_virtual_path(include_path);
    }
    let including_source_name = normalize_virtual_path(including_source_name);
    let directory = including_source_name
        .rsplit_once('/')
        .map(|(directory, _)| directory)
        .unwrap_or("");
    if directory.is_empty() {
        normalize_virtual_path(include_path)
    } else {
        normalize_virtual_path(&format!("{directory}/{include_path}"))
    }
}

/// Host-filesystem include resolver using canonical paths and UTF-8 files.
#[cfg(feature = "std")]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct FilesystemAssemblyResolver;

#[cfg(feature = "std")]
impl AssemblyIncludeResolver for FilesystemAssemblyResolver {
    fn resolve_include(
        &self,
        including_source_name: &str,
        include_path: &str,
    ) -> Result<ResolvedAssemblyInclude, Diagnostic> {
        let including_path = Path::new(including_source_name);
        let base = including_path.parent().unwrap_or_else(|| Path::new("."));
        let candidate = base.join(include_path);
        let canonical = fs::canonicalize(&candidate).map_err(|error| {
            Diagnostic::new(format!(
                "failed to resolve assembly include `{include_path}` from `{including_source_name}`: {error}"
            ))
        })?;
        let bytes = fs::read(&canonical).map_err(|error| {
            Diagnostic::new(format!(
                "failed to read assembly include `{}`: {error}",
                canonical.display()
            ))
        })?;
        let source = String::from_utf8(bytes).map_err(|_| {
            Diagnostic::new(format!(
                "assembly include `{}` is not UTF-8",
                canonical.display()
            ))
        })?;
        Ok(ResolvedAssemblyInclude::new(
            canonical.to_string_lossy().into_owned(),
            source,
        ))
    }
}

/// Canonicalize, read, and preprocess a UTF-8 assembly file.
#[cfg(feature = "std")]
pub fn preprocess_assembly_file(
    path: impl AsRef<Path>,
    options: AssemblyPreprocessOptions,
) -> Result<PreprocessedAssembly, Diagnostic> {
    let path = path.as_ref();
    let canonical = fs::canonicalize(path).map_err(|error| {
        Diagnostic::new(format!(
            "failed to resolve assembly source `{}`: {error}",
            path.display()
        ))
    })?;
    let bytes = fs::read(&canonical).map_err(|error| {
        Diagnostic::new(format!(
            "failed to read assembly source `{}`: {error}",
            canonical.display()
        ))
    })?;
    let source = String::from_utf8(bytes).map_err(|_| {
        Diagnostic::new(format!(
            "assembly source `{}` is not UTF-8",
            canonical.display()
        ))
    })?;
    let source_name = canonical.to_string_lossy().into_owned();
    preprocess_assembly_with_resolver(&source_name, &source, &FilesystemAssemblyResolver, options)
}

#[derive(Clone, Debug)]
struct AssemblyMacroDefinition {
    parameters: Vec<String>,
    body: Vec<LocatedParsedAssemblyItem>,
}

struct AssemblyPreprocessor<'a> {
    resolver: &'a dyn AssemblyIncludeResolver,
    options: &'a AssemblyPreprocessOptions,
    defines: BTreeMap<String, String>,
    macros: BTreeMap<String, AssemblyMacroDefinition>,
    include_stack: Vec<String>,
    next_expansion_id: u64,
}

impl AssemblyPreprocessor<'_> {
    fn process_source(
        &mut self,
        source_name: &str,
        source: &str,
        macro_depth: usize,
    ) -> Result<Vec<LocatedParsedAssemblyItem>, Diagnostic> {
        let parsed = parse_assembly_syntax(source_name, source)?;
        self.process_items(parsed.items, macro_depth)
    }

    fn process_items(
        &mut self,
        items: Vec<LocatedParsedAssemblyItem>,
        macro_depth: usize,
    ) -> Result<Vec<LocatedParsedAssemblyItem>, Diagnostic> {
        let mut output = Vec::new();
        for item in items {
            let location = item.location;
            match item.kind {
                ParsedAssemblyItem::Include { path } => {
                    let path = substitute_defines(&path, &self.defines);
                    let included = self
                        .resolver
                        .resolve_include(&source_location_name(&location), &path)
                        .map_err(|error| error.with_location_if_missing(location.clone()))?;
                    if let Some(cycle_start) = self
                        .include_stack
                        .iter()
                        .position(|source| source == &included.source_name)
                    {
                        let mut chain = self.include_stack[cycle_start..].to_vec();
                        chain.push(included.source_name.clone());
                        return Err(Diagnostic::at(
                            location,
                            format!("assembly include cycle: {}", chain.join(" -> ")),
                        ));
                    }
                    self.include_stack.push(included.source_name.clone());
                    let result =
                        self.process_source(&included.source_name, &included.source, macro_depth);
                    self.include_stack.pop();
                    output.extend(result?);
                }
                ParsedAssemblyItem::Define { name, value } => {
                    let name = substitute_defines(&name, &self.defines);
                    let value = substitute_defines(&value, &self.defines);
                    self.defines.insert(name, value);
                }
                ParsedAssemblyItem::MacroDefinition {
                    name,
                    parameters,
                    body,
                } => {
                    let name = substitute_defines(&name, &self.defines);
                    self.macros
                        .insert(name, AssemblyMacroDefinition { parameters, body });
                }
                ParsedAssemblyItem::MacroInvocation { name, arguments } => {
                    self.expand_macro_invocation(
                        &mut output,
                        location,
                        name,
                        arguments,
                        macro_depth,
                    )?;
                }
                ParsedAssemblyItem::Conditional {
                    condition,
                    then_items,
                    else_items,
                } => {
                    let condition = substitute_defines(&condition, &self.defines);
                    let selected = if self.evaluate_condition(&condition, &location)? {
                        then_items
                    } else {
                        else_items
                    };
                    output.extend(self.process_items(selected, macro_depth)?);
                }
                kind => {
                    if let Some(kind) = self.normalize_ordinary_item(kind, &location)? {
                        output.push(LocatedParsedAssemblyItem { location, kind });
                    }
                }
            }
        }
        Ok(output)
    }

    fn expand_macro_invocation(
        &mut self,
        output: &mut Vec<LocatedParsedAssemblyItem>,
        location: SourceLocation,
        name: String,
        arguments: Vec<String>,
        macro_depth: usize,
    ) -> Result<(), Diagnostic> {
        let name = substitute_defines(&name, &self.defines);
        let definition = self.macros.get(&name).cloned().ok_or_else(|| {
            Diagnostic::at(location.clone(), format!("unknown assembly macro `{name}`"))
        })?;
        if arguments.len() != definition.parameters.len() {
            return Err(Diagnostic::at(
                location,
                format!(
                    "macro `{name}` expects {} arguments, got {}",
                    definition.parameters.len(),
                    arguments.len()
                ),
            ));
        }
        if macro_depth >= self.options.macro_depth_limit {
            return Err(Diagnostic::at(
                location,
                format!(
                    "assembly macro expansion exceeded {} nested invocations",
                    self.options.macro_depth_limit
                ),
            ));
        }

        let arguments = arguments
            .into_iter()
            .map(|argument| substitute_defines(&argument, &self.defines))
            .collect::<Vec<_>>();
        let parameters = definition
            .parameters
            .iter()
            .zip(arguments.iter())
            .map(|(parameter, argument)| (parameter.clone(), argument.clone()))
            .collect::<BTreeMap<_, _>>();
        let expansion_id = self.next_expansion_id;
        self.next_expansion_id = self.next_expansion_id.saturating_add(1);
        let hygiene_prefix = format!("__ezra_macro_{expansion_id}_");
        let expanded = definition
            .body
            .into_iter()
            .map(|item| {
                substitute_parsed_item(item, &parameters, &self.defines, &hygiene_prefix, &location)
            })
            .collect();
        output.extend(self.process_items(expanded, macro_depth + 1)?);
        Ok(())
    }

    fn evaluate_condition(
        &self,
        condition: &str,
        location: &SourceLocation,
    ) -> Result<bool, Diagnostic> {
        let (predicate, argument) = parse_condition(condition)
            .map_err(|message| Diagnostic::at(location.clone(), message))?;
        match predicate {
            "cpu" => Ok(self.options.cpu
                == condition_string_argument(predicate, argument)
                    .map_err(|message| Diagnostic::at(location.clone(), message))?),
            "target" => Ok(self.options.target
                == condition_string_argument(predicate, argument)
                    .map_err(|message| Diagnostic::at(location.clone(), message))?),
            "feature" => {
                let feature = condition_string_argument(predicate, argument)
                    .map_err(|message| Diagnostic::at(location.clone(), message))?;
                Ok(self.options.feature_enabled(&feature))
            }
            "defined" => {
                let name = argument.trim();
                if name.is_empty() || !name.chars().all(is_define_name_character) {
                    return Err(Diagnostic::at(
                        location.clone(),
                        "`defined` expects a non-empty define name",
                    ));
                }
                Ok(self.defines.contains_key(name))
            }
            _ => Err(Diagnostic::at(
                location.clone(),
                format!("unsupported assembly condition `{condition}`"),
            )),
        }
    }

    fn normalize_ordinary_item(
        &self,
        kind: ParsedAssemblyItem,
        location: &SourceLocation,
    ) -> Result<Option<ParsedAssemblyItem>, Diagnostic> {
        let kind = substitute_ordinary_item(kind, &self.defines);
        let ParsedAssemblyItem::Directive { name, arguments } = kind else {
            return Ok(Some(kind));
        };
        if name.eq_ignore_ascii_case("global") || name.eq_ignore_ascii_case("globl") {
            return Ok(None);
        }
        if name.eq_ignore_ascii_case("assume") {
            let assumption = arguments
                .join(",")
                .chars()
                .filter(|character| !character.is_whitespace())
                .collect::<String>()
                .to_ascii_lowercase();
            return match assumption.as_str() {
                "adl=1" => Ok(None),
                "adl=0" => Err(Diagnostic::at(
                    location.clone(),
                    "`.assume adl=0` is not supported; assembly currently supports eZ80 ADL mode only",
                )),
                _ => Err(Diagnostic::at(
                    location.clone(),
                    format!("unsupported `.assume` directive `{}`", arguments.join(", ")),
                )),
            };
        }
        Err(Diagnostic::at(
            location.clone(),
            format!("unsupported shared assembly directive `{name}`"),
        ))
    }
}

fn parse_condition(condition: &str) -> Result<(&str, &str), String> {
    let condition = condition.trim();
    let open = condition
        .find('(')
        .ok_or_else(|| format!("unsupported assembly condition `{condition}`"))?;
    let predicate = condition[..open].trim();
    let rest = condition[open + 1..].trim();
    let argument = rest
        .strip_suffix(')')
        .ok_or_else(|| format!("assembly condition `{condition}` is missing `)`"))?;
    if predicate.is_empty() || argument.contains(')') {
        return Err(format!("unsupported assembly condition `{condition}`"));
    }
    Ok((predicate, argument.trim()))
}

fn condition_string_argument(predicate: &str, argument: &str) -> Result<String, String> {
    let argument = argument.trim();
    if argument.len() >= 2 {
        let first = argument.as_bytes()[0];
        let last = argument.as_bytes()[argument.len() - 1];
        if (first == b'"' && last == b'"') || (first == b'\'' && last == b'\'') {
            return Ok(argument[1..argument.len() - 1].to_owned());
        }
    }
    if argument.is_empty() || argument.chars().any(char::is_whitespace) {
        Err(format!(
            "assembly condition `{predicate}` expects one string argument"
        ))
    } else {
        Ok(argument.to_owned())
    }
}

fn is_define_name_character(character: char) -> bool {
    character.is_ascii_alphanumeric() || matches!(character, '_' | '.' | '@' | '?' | '%')
}

fn source_location_name(location: &SourceLocation) -> String {
    #[cfg(feature = "std")]
    {
        location.file.to_string_lossy().into_owned()
    }
    #[cfg(all(feature = "no-std", not(feature = "std")))]
    {
        location.file.clone()
    }
}

fn substitute_parsed_item(
    item: LocatedParsedAssemblyItem,
    parameters: &BTreeMap<String, String>,
    defines: &BTreeMap<String, String>,
    hygiene_prefix: &str,
    invocation_location: &SourceLocation,
) -> LocatedParsedAssemblyItem {
    let substitute = |text: String| {
        let text = substitute_parameters(&text, parameters);
        let text = substitute_defines(&text, defines);
        text.replace("%%", hygiene_prefix)
    };
    let kind = substitute_item_fields(item.kind, &substitute, Some(invocation_location));
    LocatedParsedAssemblyItem {
        location: invocation_location.clone(),
        kind,
    }
}

fn substitute_ordinary_item(
    kind: ParsedAssemblyItem,
    defines: &BTreeMap<String, String>,
) -> ParsedAssemblyItem {
    substitute_item_fields(kind, &|text| substitute_defines(&text, defines), None)
}

fn substitute_item_fields(
    kind: ParsedAssemblyItem,
    substitute: &impl Fn(String) -> String,
    nested_location: Option<&SourceLocation>,
) -> ParsedAssemblyItem {
    match kind {
        ParsedAssemblyItem::Include { path } => ParsedAssemblyItem::Include {
            path: substitute(path),
        },
        ParsedAssemblyItem::Define { name, value } => ParsedAssemblyItem::Define {
            name: substitute(name),
            value: substitute(value),
        },
        ParsedAssemblyItem::MacroDefinition {
            name,
            parameters,
            body,
        } => ParsedAssemblyItem::MacroDefinition {
            name: substitute(name),
            parameters: parameters.into_iter().map(substitute).collect(),
            body: body
                .into_iter()
                .map(|item| LocatedParsedAssemblyItem {
                    location: nested_location
                        .cloned()
                        .unwrap_or_else(|| item.location.clone()),
                    kind: substitute_item_fields(item.kind, substitute, nested_location),
                })
                .collect(),
        },
        ParsedAssemblyItem::MacroInvocation { name, arguments } => {
            ParsedAssemblyItem::MacroInvocation {
                name: substitute(name),
                arguments: arguments.into_iter().map(substitute).collect(),
            }
        }
        ParsedAssemblyItem::Conditional {
            condition,
            then_items,
            else_items,
        } => ParsedAssemblyItem::Conditional {
            condition: substitute(condition),
            then_items: substitute_nested_items(then_items, substitute, nested_location),
            else_items: substitute_nested_items(else_items, substitute, nested_location),
        },
        ParsedAssemblyItem::Label(name) => ParsedAssemblyItem::Label(substitute(name)),
        ParsedAssemblyItem::Equ { name, value } => ParsedAssemblyItem::Equ {
            name: substitute(name),
            value: substitute(value),
        },
        ParsedAssemblyItem::Section(name) => ParsedAssemblyItem::Section(substitute(name)),
        ParsedAssemblyItem::Org(value) => ParsedAssemblyItem::Org(substitute(value)),
        ParsedAssemblyItem::Data { width, values } => ParsedAssemblyItem::Data {
            width,
            values: values
                .into_iter()
                .map(|value| match value {
                    ParsedAssemblyDataValue::Expression(value) => {
                        ParsedAssemblyDataValue::Expression(substitute(value))
                    }
                    ParsedAssemblyDataValue::StringLiteral(value) => {
                        ParsedAssemblyDataValue::StringLiteral(substitute(value))
                    }
                })
                .collect(),
        },
        ParsedAssemblyItem::Directive { name, arguments } => ParsedAssemblyItem::Directive {
            name: substitute(name),
            arguments: arguments.into_iter().map(substitute).collect(),
        },
        ParsedAssemblyItem::Instruction(instruction) => {
            ParsedAssemblyItem::Instruction(AssemblyInstruction {
                mnemonic: substitute(instruction.mnemonic),
                operands: instruction.operands.into_iter().map(substitute).collect(),
            })
        }
    }
}

fn substitute_nested_items(
    items: Vec<LocatedParsedAssemblyItem>,
    substitute: &impl Fn(String) -> String,
    nested_location: Option<&SourceLocation>,
) -> Vec<LocatedParsedAssemblyItem> {
    items
        .into_iter()
        .map(|item| LocatedParsedAssemblyItem {
            location: nested_location
                .cloned()
                .unwrap_or_else(|| item.location.clone()),
            kind: substitute_item_fields(item.kind, substitute, nested_location),
        })
        .collect()
}

fn substitute_defines(text: &str, defines: &BTreeMap<String, String>) -> String {
    let mut output = text.to_owned();
    for (name, value) in defines {
        output = output.replace(&format!("${{{name}}}"), value);
    }
    output
}

fn substitute_parameters(text: &str, parameters: &BTreeMap<String, String>) -> String {
    let mut output = text.to_owned();
    for (name, value) in parameters {
        output = replace_parameter(&output, name, value);
    }
    output
}

fn replace_parameter(text: &str, parameter: &str, value: &str) -> String {
    if parameter.is_empty() {
        return text.to_owned();
    }
    let pattern = format!("${parameter}");
    let mut output = String::new();
    let mut remaining = text;
    while let Some(index) = remaining.find(&pattern) {
        output.push_str(&remaining[..index]);
        let after = &remaining[index + pattern.len()..];
        if after.chars().next().is_some_and(is_define_name_character) {
            output.push_str(&pattern);
        } else {
            output.push_str(value);
        }
        remaining = after;
    }
    output.push_str(remaining);
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asm::frontend::AssemblyItem;
    use crate::workspace::WorkspaceFile;

    #[derive(Default)]
    struct MemoryResolver {
        files: BTreeMap<String, String>,
    }

    impl MemoryResolver {
        fn with(mut self, path: &str, source: &str) -> Self {
            self.files.insert(path.to_owned(), source.to_owned());
            self
        }
    }

    impl AssemblyIncludeResolver for MemoryResolver {
        fn resolve_include(
            &self,
            including_source_name: &str,
            include_path: &str,
        ) -> Result<ResolvedAssemblyInclude, Diagnostic> {
            let path = resolve_virtual_include_path(including_source_name, include_path);
            let source = self.files.get(&path).ok_or_else(|| {
                Diagnostic::new(format!("missing in-memory assembly include `{path}`"))
            })?;
            Ok(ResolvedAssemblyInclude::new(path, source.clone()))
        }
    }

    fn options() -> AssemblyPreprocessOptions {
        let mut options = AssemblyPreprocessOptions::new("agonlight-mos-ez80", "ez80");
        options.enabled_features.push("z80".to_owned());
        options
    }

    fn instruction_texts(preprocessed: &PreprocessedAssembly) -> Vec<String> {
        preprocessed
            .program
            .items
            .iter()
            .filter_map(|item| match &item.kind {
                AssemblyItem::Instruction(instruction) => Some(instruction.to_text()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn expands_nested_macros_and_delimiter_aware_arguments() {
        let source = r#"%macro inner(value)
    ld hl, $value
%endmacro
%macro outer(value)
    %inner ($value + 1)
%endmacro
%outer (2 + (3 * 4))
"#;
        let result = preprocess_assembly("main.asm", source, options()).unwrap();
        assert_eq!(instruction_texts(&result), ["ld hl, 2 + (3 * 4) + 1"]);
    }

    #[test]
    fn assigns_unique_hygienic_labels_across_nested_expansions() {
        let source = r#"%macro leaf()
%%loop:
    jp %%loop
%endmacro
%macro pair()
    %leaf
    %leaf
%endmacro
%pair
%leaf
"#;
        let result = preprocess_assembly("main.asm", source, options()).unwrap();
        let labels = result
            .program
            .items
            .iter()
            .filter_map(|item| match &item.kind {
                AssemblyItem::Label(label) => Some(label.clone()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(labels.len(), 3);
        assert!(
            labels
                .iter()
                .all(|label| label.starts_with("__ezra_macro_"))
        );
        assert_ne!(labels[0], labels[1]);
        assert_ne!(labels[1], labels[2]);
    }

    #[test]
    fn evaluates_nested_conditions_without_mutating_inactive_state() {
        let source = r#"%if cpu("ez80")
    %define ACTIVE 7
    %if target("agonlight-mos-ez80")
        db ${ACTIVE}
    %else
        %define LEAK 1
    %endif
%else
    %define LEAK 2
%endif
%if feature("z80")
    db 8
%endif
%if defined(LEAK)
    db 9
%else
    db 10
%endif
"#;
        let result = preprocess_assembly("main.asm", source, options()).unwrap();
        let values = result
            .syntax
            .items
            .iter()
            .filter_map(|item| match &item.kind {
                ParsedAssemblyItem::Data { values, .. } => match &values[0] {
                    ParsedAssemblyDataValue::Expression(value) => Some(value.as_str()),
                    _ => None,
                },
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(values, ["7", "8", "10"]);
    }

    #[test]
    fn includes_retain_provenance_and_cycles_report_the_chain() {
        let resolver = MemoryResolver::default()
            .with("lib/outer.inc", "include \"inner.inc\"\nnop\n")
            .with("lib/inner.inc", "halt\n");
        let result = preprocess_assembly_with_resolver(
            "main.asm",
            "include \"lib/outer.inc\"\n",
            &resolver,
            options(),
        )
        .unwrap();
        assert_eq!(
            source_location_name(&result.syntax.items[0].location),
            "lib/inner.inc"
        );
        assert_eq!(
            source_location_name(&result.syntax.items[1].location),
            "lib/outer.inc"
        );

        let cyclic = MemoryResolver::default()
            .with("a.inc", "include \"b.inc\"\n")
            .with("b.inc", "include \"a.inc\"\n");
        let error = preprocess_assembly_with_resolver(
            "main.asm",
            "include \"a.inc\"\n",
            &cyclic,
            options(),
        )
        .unwrap_err();
        assert!(error.message.contains("a.inc -> b.inc -> a.inc"));
        assert_eq!(source_location_name(&error.location().unwrap()), "b.inc");
    }

    #[test]
    fn rejects_wrong_arity_and_unterminated_blocks_at_the_source_location() {
        let arity = preprocess_assembly(
            "arity.asm",
            "%macro one(value)\nnop\n%endmacro\n%one\n",
            options(),
        )
        .unwrap_err();
        assert!(arity.message.contains("expects 1 arguments, got 0"));
        assert_eq!(arity.location().unwrap().line, 4);

        let unterminated =
            preprocess_assembly("unterminated.asm", "%if cpu(\"ez80\")\nnop\n", options())
                .unwrap_err();
        assert!(unterminated.span.is_some());
    }

    #[test]
    fn macro_expansion_reports_the_invocation_origin() {
        let source = "%macro broken(value)\n    org $value +\n%endmacro\n\n%broken 1\n";
        let error = preprocess_assembly("origin.asm", source, options()).unwrap_err();
        let location = error.location().unwrap();
        assert_eq!(location.line, 5);
        assert_eq!(source_location_name(&location), "origin.asm");
    }

    #[test]
    fn workspace_resolver_handles_relative_includes() {
        let files = [
            WorkspaceFile::text("src/main.asm", "include \"../lib/code.inc\"\n"),
            WorkspaceFile::text("lib/code.inc", "nop\n"),
        ];
        let workspace = Workspace::new(&files);
        let result =
            preprocess_assembly_workspace(&workspace, r"src\.\main.asm", options()).unwrap();
        assert_eq!(instruction_texts(&result), ["nop"]);
        assert_eq!(
            source_location_name(&result.syntax.items[0].location),
            "lib/code.inc"
        );
    }

    #[test]
    fn normalizes_compatibility_directives() {
        let result = preprocess_assembly(
            "compat.asm",
            ".global entry\n.assume adl = 1\nentry:\n    nop\n",
            options(),
        )
        .unwrap();
        assert_eq!(result.program.items.len(), 2);

        let error = preprocess_assembly("compat.asm", ".assume adl=0\n", options()).unwrap_err();
        assert!(error.message.contains("adl=0"));
        assert_eq!(error.location().unwrap().line, 1);
    }
}
