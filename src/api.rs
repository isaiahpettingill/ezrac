//! Public, in-process compiler API.
//!
//! This module compiles EZRA source supplied by another Rust program without
//! invoking the `ezrac` CLI or writing build artifacts. It produces target
//! assembly; executable packaging remains a CLI concern.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

pub use crate::workspace::{Workspace, WorkspaceFile};

use crate::{
    asm::{
        AssemblyOptions, emit_ez80_assembly_with_options, emit_lr35902_assembly_with_options,
        emit_mos6502_assembly_with_options,
    },
    ast::{Declaration, Program},
    compile::{
        CompileOptions, CompileReport, SdkResolver, check_source_with_sdk_and_overrides,
        parse_and_resolve_imports_with_sdk_and_overrides,
        parse_and_resolve_imports_with_sdk_and_workspace,
    },
    diagnostic::Diagnostic,
    layout::default_layout_for_target,
    package::{PackageRequest, package_executable},
    parser::parse_program,
    target::{
        Address24, AssemblerCpu, CpuFamily, DEFAULT_TARGET_TRIPLE, OutputFormat,
        resolve_target_profile,
    },
    tbir::diagnostics::validate_program,
    vm::{AssemblySymbol, assemble_subset_with_symbols_at},
    workspace::normalize_virtual_path,
};

#[cfg(feature = "dcpu")]
use crate::asm::emit_dcpu_assembly_with_options;
#[cfg(feature = "m68k")]
use crate::asm::emit_m68k_assembly_with_options;
#[cfg(feature = "m6800")]
use crate::asm::emit_m6800_assembly_with_options;
#[cfg(feature = "tms9900")]
use crate::asm::emit_tms9900_assembly_with_options;

/// Options for compiling in-memory EZRA source to target assembly.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompileRequest {
    /// Logical path used for diagnostics and relative import resolution.
    pub source_path: PathBuf,
    /// Target triple used for SDK selection, validation, and code generation.
    pub target: String,
    /// Additional project SDK roots. Built-in SDKs are selected from `target`.
    pub sdk_paths: Vec<PathBuf>,
    /// Include generator debug comments in the emitted assembly where supported.
    pub debug_comments: bool,
    /// Enable default target SDK symbols.
    pub default_sdk_symbols: bool,
}

impl CompileRequest {
    /// Create a request with target-appropriate built-in SDK imports enabled.
    pub fn new(source_path: impl Into<PathBuf>, target: impl Into<String>) -> Self {
        Self {
            source_path: source_path.into(),
            target: target.into(),
            sdk_paths: Vec::new(),
            debug_comments: false,
            default_sdk_symbols: true,
        }
    }

    /// Construct a request for EZRA's default target.
    pub fn with_default_target(source_path: impl Into<PathBuf>) -> Self {
        Self::new(source_path, DEFAULT_TARGET_TRIPLE)
    }

    fn sdk_resolver(&self) -> SdkResolver {
        SdkResolver {
            target: Some(self.target.clone()),
            sdk_roots: self.sdk_paths.clone(),
        }
    }
}

impl Workspace<'_> {
    fn text_overrides(&self) -> Result<HashMap<PathBuf, String>, Diagnostic> {
        let mut overrides = HashMap::new();
        for file in self.files {
            let virtual_path = normalize_virtual_path(file.path);
            if !virtual_path.ends_with(".ezra") {
                continue;
            }
            let text = core::str::from_utf8(file.contents).map_err(|_| {
                Diagnostic::new(format!("workspace source `{}` is not UTF-8", file.path))
            })?;
            overrides.insert(PathBuf::from(&virtual_path), text.to_owned());
            let host_path = virtual_path.replace('/', std::path::MAIN_SEPARATOR_STR);
            overrides.insert(PathBuf::from(host_path), text.to_owned());
        }
        Ok(overrides)
    }
}

/// Successful in-process compilation output.
#[derive(Clone, Debug, PartialEq)]
pub struct AssemblyCompilation {
    /// Semantic/import report for the root source unit.
    pub report: CompileReport,
    /// Root program with imports resolved and public imported aliases added.
    pub program: Program,
    /// Target assembly text. The caller owns any further assembly/packaging.
    pub assembly: String,
}

/// Complete filesystem-free build output.
#[derive(Clone, Debug, PartialEq)]
pub struct BuildCompilation {
    pub report: CompileReport,
    pub program: Program,
    pub assembly: String,
    pub machine_code: Vec<u8>,
    pub symbols: Vec<AssemblySymbol>,
    pub executable: Vec<u8>,
    pub output_format: OutputFormat,
    pub executable_extension: &'static str,
}

/// Compile in-memory EZRA source to target assembly.
///
/// The source must define `fn main()` because this API emits executable source
/// assembly. For library and SDK diagnostics without an entry point, use
/// [`crate::compile::check_module_diagnostics_with_sdk_and_overrides`] or an
/// EZRA project with `[lsp] mode = "library"`.
pub fn compile_source_to_assembly(
    source: &str,
    request: &CompileRequest,
) -> Result<AssemblyCompilation, Diagnostic> {
    compile_source_to_assembly_with_overrides(source, request, &HashMap::new())
}

/// Compile an Ezra source file using a caller-owned virtual workspace.
///
/// Imports resolve from `workspace` before host filesystem SDK roots. This
/// makes compilation deterministic for embedders and is the std-mode precursor
/// to the alloc-only workspace API.
pub fn compile_workspace_to_assembly(
    workspace: &Workspace<'_>,
    root: &str,
    request: &CompileRequest,
) -> Result<AssemblyCompilation, Diagnostic> {
    let root = normalize_virtual_path(root);
    let source = workspace.file(&root).ok_or_else(|| {
        Diagnostic::new(format!("workspace does not contain root source `{root}`"))
    })?;
    let source = core::str::from_utf8(source)
        .map_err(|_| Diagnostic::new(format!("workspace source `{root}` is not UTF-8")))?;
    let mut request = request.clone();
    request.source_path = PathBuf::from(&root);
    let target = resolve_target_profile(Some(&request.target)).map_err(Diagnostic::new)?;
    let sdk = request.sdk_resolver();
    let overrides = workspace.text_overrides()?;
    let root_program = parse_program(&request.source_path, source)?;
    let imports = root_program
        .declarations
        .iter()
        .filter(|declaration| matches!(declaration, Declaration::Import(_)))
        .count();
    let program = parse_and_resolve_imports_with_sdk_and_workspace(
        &request.source_path,
        source,
        &sdk,
        &overrides,
        workspace,
    )?;
    let has_main = program.main_function().is_some();
    if !has_main {
        return Err(Diagnostic::new("missing required `fn main()`"));
    }
    let main = program.main_function().expect("main presence checked");
    if !main.params.is_empty() {
        return Err(Diagnostic::new("main function cannot take parameters"));
    }
    if main.return_type.is_some() {
        return Err(Diagnostic::new("main function cannot return a value"));
    }
    let report = CompileReport {
        imports,
        declarations: program.declarations.len(),
        has_main,
    };
    let options = assembly_options_for_target(
        &request.target,
        target.triple.cpu,
        request.debug_comments,
        request.default_sdk_symbols,
    );
    let assembly = emit_source_assembly(&program, options)?;
    Ok(AssemblyCompilation {
        report,
        program,
        assembly,
    })
}

/// Compile, assemble, and package a virtual workspace entirely in memory.
pub fn build_workspace(
    workspace: &Workspace<'_>,
    root: &str,
    request: &CompileRequest,
) -> Result<BuildCompilation, Diagnostic> {
    let compilation = compile_workspace_to_assembly(workspace, root, request)?;
    let target = resolve_target_profile(Some(&request.target)).map_err(Diagnostic::new)?;
    let layout = default_layout_for_target(&request.target);
    let assembled = assemble_subset_with_symbols_at(
        AssemblerCpu::from(target.triple.cpu),
        &compilation.assembly,
        layout.entry.get(),
    )?;
    let package_request = PackageRequest {
        target: request.target.clone(),
        output_format: target.output_format,
        load_addr: layout.load.get(),
        entry_addr: layout.entry.get(),
        executable_name: root
            .rsplit('/')
            .next()
            .and_then(|name| name.split('.').next())
            .map(str::to_owned),
    };
    let executable = package_executable(&package_request, &assembled.bytes)
        .map_err(|error| Diagnostic::new(error.message))?;

    Ok(BuildCompilation {
        report: compilation.report,
        program: compilation.program,
        assembly: compilation.assembly,
        machine_code: assembled.bytes,
        symbols: assembled.symbols,
        executable,
        output_format: target.output_format,
        executable_extension: if request.target.starts_with("gameboy-color-") {
            "gbc"
        } else {
            target.output_format.extension()
        },
    })
}

fn compile_source_to_assembly_with_overrides(
    source: &str,
    request: &CompileRequest,
    source_overrides: &HashMap<PathBuf, String>,
) -> Result<AssemblyCompilation, Diagnostic> {
    let target = resolve_target_profile(Some(&request.target)).map_err(Diagnostic::new)?;
    let sdk = request.sdk_resolver();
    let options = CompileOptions {
        source: request.source_path.clone(),
        debug_comments: request.debug_comments,
        default_sdk_symbols: request.default_sdk_symbols,
    };
    let report = check_source_with_sdk_and_overrides(source, &options, &sdk, source_overrides)?;
    let program = parse_and_resolve_imports_with_sdk_and_overrides(
        &request.source_path,
        source,
        &sdk,
        source_overrides,
    )?;
    let assembly_options = assembly_options_for_target(
        &request.target,
        target.triple.cpu,
        request.debug_comments,
        request.default_sdk_symbols,
    );
    let assembly = emit_source_assembly(&program, assembly_options)?;

    Ok(AssemblyCompilation {
        report,
        program,
        assembly,
    })
}

/// Build target assembly options from a target triple and its default layout.
pub fn assembly_options_for_target(
    target: &str,
    cpu: CpuFamily,
    debug_comments: bool,
    default_sdk_symbols: bool,
) -> AssemblyOptions {
    let layout = default_layout_for_target(target);
    let symbol = |name: &str| {
        layout
            .symbols
            .iter()
            .find(|symbol| symbol.name == name)
            .map(|symbol| symbol.value)
    };
    let defaults = AssemblyOptions::default();
    let is_16_bit = crate::target::memory_model_for_cpu(cpu)
        .is_some_and(|memory| memory.address_width_bits <= 16);

    AssemblyOptions {
        cpu,
        debug_comments,
        default_sdk_symbols,
        mos_executable: layout.name == "agon_light_mos",
        c64_executable: matches!(layout.name.as_str(), "commodore64_6502" | "commodore64_crt"),
        ti_os_executable: target.starts_with("ti83-z80")
            || target.starts_with("ti83plus-z80")
            || target.starts_with("ti84-z80")
            || target.starts_with("ti84plus-z80")
            || target.starts_with("ti84plusce-ez80")
            || target.starts_with("ti83premiumce-ez80"),
        load_addr: symbol("EZRA_LOAD_ADDR").unwrap_or(layout.load),
        entry_addr: symbol("EZRA_ENTRY_ADDR").unwrap_or(layout.entry),
        code_base: symbol("EZRA_CODE_BASE").unwrap_or(layout.entry),
        stack_top: symbol("EZRA_STACK_TOP").unwrap_or(layout.stack),
        ram_base: symbol("EZRA_RAM_BASE")
            .or(is_16_bit.then_some(Address24::new(0xA000)))
            .unwrap_or(defaults.ram_base),
        vram_base: symbol("EZRA_VRAM_BASE")
            .or(is_16_bit.then_some(Address24::new(0)))
            .unwrap_or(defaults.vram_base),
        audio_base: symbol("EZRA_AUDIO_BASE")
            .or(is_16_bit.then_some(Address24::new(0)))
            .unwrap_or(defaults.audio_base),
        asset_base: symbol("EZRA_ASSET_BASE")
            .or(is_16_bit.then_some(Address24::new(0xC000)))
            .unwrap_or(defaults.asset_base),
        rodata_base: symbol("EZRA_RODATA_BASE")
            .or(is_16_bit.then_some(Address24::new(0x8000)))
            .unwrap_or(defaults.rodata_base),
        section_bases: Vec::new(),
    }
}

fn emit_source_assembly(program: &Program, options: AssemblyOptions) -> Result<String, Diagnostic> {
    validate_program(program, options.cpu)?;
    match options.cpu {
        CpuFamily::Lr35902 => emit_lr35902_assembly_with_options(program, options),
        CpuFamily::Mos6502 | CpuFamily::Cmos65C02 | CpuFamily::Wdc65C816 | CpuFamily::Ricoh2A03 => {
            emit_mos6502_assembly_with_options(program, options)
        }
        CpuFamily::Dcpu => {
            #[cfg(feature = "dcpu")]
            {
                emit_dcpu_assembly_with_options(program, options)
            }
            #[cfg(not(feature = "dcpu"))]
            {
                Err(Diagnostic::new(
                    "DCPU-16 source compilation requires the `dcpu` Cargo feature",
                ))
            }
        }
        CpuFamily::M6800 => {
            #[cfg(feature = "m6800")]
            {
                emit_m6800_assembly_with_options(program, options)
            }
            #[cfg(not(feature = "m6800"))]
            {
                Err(Diagnostic::new(
                    "M6800 source compilation requires the `m6800` Cargo feature",
                ))
            }
        }
        CpuFamily::Tms9900 => {
            #[cfg(feature = "tms9900")]
            {
                emit_tms9900_assembly_with_options(program, options)
            }
            #[cfg(not(feature = "tms9900"))]
            {
                Err(Diagnostic::new(
                    "TMS9900 source compilation requires the `tms9900` Cargo feature",
                ))
            }
        }
        CpuFamily::M68k => {
            #[cfg(feature = "m68k")]
            {
                emit_m68k_assembly_with_options(program, options)
            }
            #[cfg(not(feature = "m68k"))]
            {
                Err(Diagnostic::new(
                    "M68k source compilation requires the `m68k` Cargo feature",
                ))
            }
        }
        CpuFamily::Avr => Err(Diagnostic::new(format!(
            "EZRA source code generation is not implemented for CPU `{}`",
            options.cpu.as_str()
        ))),
        _ => emit_ez80_assembly_with_options(program, options),
    }
}

/// Resolve the source path used by a compilation request relative to a host
/// application without requiring it to exist on disk.
pub fn source_path(root: impl AsRef<Path>, relative: impl AsRef<Path>) -> PathBuf {
    root.as_ref().join(relative)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{EmbedSource, Expr};

    fn materialized_embed_bytes(program: &Program, name: &str) -> Vec<u8> {
        let embed = program
            .declarations
            .iter()
            .find_map(|declaration| match declaration {
                Declaration::Embed(embed) if embed.name == name => Some(embed),
                _ => None,
            })
            .expect("materialized embed declaration");
        let EmbedSource::Bytes(values) = &embed.source else {
            panic!("workspace file embed was not materialized");
        };
        values
            .iter()
            .map(|value| match value {
                Expr::Int(value) => *value as u8,
                _ => panic!("materialized workspace byte is not an integer"),
            })
            .collect()
    }

    #[test]
    fn compiles_in_memory_source_to_ez80_assembly() {
        let request = CompileRequest::new("memory.ezra", "custom-unknown-ez80");
        let compilation = compile_source_to_assembly("fn main() {}", &request).unwrap();

        assert!(compilation.report.has_main);
        assert!(compilation.assembly.contains("__ezra_start:"));
    }

    #[test]
    fn compiles_imports_from_a_virtual_workspace() {
        let files = [
            WorkspaceFile::text(
                "src/main.ezra",
                "import math\nfn main() { let value: u8 = math.VALUE }\n",
            ),
            WorkspaceFile::text("src/math.ezra", "pub const VALUE: u8 = 42\n"),
        ];
        let request = CompileRequest::new("ignored.ezra", "custom-unknown-ez80");
        let compilation =
            compile_workspace_to_assembly(&Workspace::new(&files), "src/main.ezra", &request)
                .unwrap();

        assert!(compilation.report.has_main);
        assert!(compilation.assembly.contains("_main:"));
    }

    #[test]
    fn materializes_root_relative_workspace_assets() {
        let files = [
            WorkspaceFile::text(
                "src/main.ezra",
                "embed blob: bytes = file(\"assets/blob.bin\")\nfn main() {}\n",
            ),
            WorkspaceFile::new("src/assets/blob.bin", &[0xA5, 0x00, 0xFF]),
        ];
        let compilation = compile_workspace_to_assembly(
            &Workspace::new(&files),
            "src/main.ezra",
            &CompileRequest::new("ignored.ezra", "cpm-2.2-z80"),
        )
        .unwrap();

        assert_eq!(
            materialized_embed_bytes(&compilation.program, "blob"),
            [0xA5, 0x00, 0xFF]
        );
    }

    #[test]
    fn materializes_imported_module_relative_workspace_assets() {
        let files = [
            WorkspaceFile::text("src/main.ezra", "import lib.media\nfn main() {}\n"),
            WorkspaceFile::text(
                "src/lib/media.ezra",
                "pub embed sprite: bytes = file(\"assets/sprite.bin\")\n",
            ),
            WorkspaceFile::new("src/lib/assets/sprite.bin", &[0xDE, 0xAD]),
        ];
        let build = build_workspace(
            &Workspace::new(&files),
            "src/main.ezra",
            &CompileRequest::new("ignored.ezra", "cpm-2.2-z80"),
        )
        .unwrap();

        assert_eq!(
            materialized_embed_bytes(&build.program, "sprite"),
            [0xDE, 0xAD]
        );
        assert!(!build.machine_code.is_empty());
    }

    #[test]
    fn reports_missing_virtual_workspace_assets() {
        let files = [WorkspaceFile::text(
            "src/main.ezra",
            "embed blob: bytes = file(\"assets/missing.bin\")\nfn main() {}\n",
        )];
        let error = compile_workspace_to_assembly(
            &Workspace::new(&files),
            "src/main.ezra",
            &CompileRequest::new("ignored.ezra", "cpm-2.2-z80"),
        )
        .unwrap_err();

        assert_eq!(
            error.message,
            "virtual workspace asset `assets/missing.bin` referenced from `src/main.ezra` was not found (resolved as `src/assets/missing.bin`)"
        );
    }

    #[test]
    fn builds_and_packages_virtual_workspace_for_agon() {
        let files = [WorkspaceFile::text("main.ezra", "fn main() {}")];
        let build = build_workspace(
            &Workspace::new(&files),
            "main.ezra",
            &CompileRequest::new("main.ezra", "agonlight-mos-ez80"),
        )
        .unwrap();

        assert_eq!(build.executable_extension, "bin");
        assert_eq!(&build.executable[64..69], b"MOS\0\x01");
        assert!(!build.machine_code.is_empty());
    }

    #[test]
    fn builds_and_packages_virtual_workspace_for_cpm() {
        let files = [WorkspaceFile::text("main.ezra", "fn main() {}")];
        let build = build_workspace(
            &Workspace::new(&files),
            "main.ezra",
            &CompileRequest::new("main.ezra", "cpm-2.2-z80"),
        )
        .unwrap();

        assert_eq!(build.executable_extension, "com");
        assert_eq!(build.executable, build.machine_code);
    }

    #[cfg(feature = "mos6502")]
    #[test]
    fn builds_and_packages_virtual_workspace_for_c64() {
        let files = [WorkspaceFile::text("main.ezra", "fn main() {}")];
        let build = build_workspace(
            &Workspace::new(&files),
            "main.ezra",
            &CompileRequest::new("main.ezra", "commodore64-6502"),
        )
        .unwrap();

        assert_eq!(build.executable_extension, "prg");
        assert_eq!(&build.executable[..2], &[0x01, 0x08]);
    }

    #[test]
    fn rejects_incompatible_platform_cpu_combinations() {
        let request = CompileRequest::new("memory.ezra", "zxspectrum-ez80");
        let error = compile_source_to_assembly("fn main() {}", &request).unwrap_err();

        assert_eq!(
            error.message,
            "target `zxspectrum-ez80` requires CPU `z80`, not `ez80`"
        );
    }

    #[test]
    fn resolves_sdk_roots_for_in_memory_compilation() {
        let root = std::env::temp_dir().join(format!("ezrac-api-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("math.ezra"), "pub const VALUE: u8 = 42\n").unwrap();
        let mut request = CompileRequest::new(root.join("main.ezra"), "custom-unknown-ez80");
        request.sdk_paths.push(root.clone());

        let compilation = compile_source_to_assembly(
            "import math\nfn main() { let value: u8 = math.VALUE }\n",
            &request,
        )
        .unwrap();
        assert!(compilation.report.has_main);

        let _ = std::fs::remove_dir_all(root);
    }
}
