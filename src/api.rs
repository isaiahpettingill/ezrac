//! Public, in-process compiler API.
//!
//! This module compiles EZRA source supplied by another Rust program without
//! invoking the `ezrac` CLI or writing build artifacts. It produces target
//! assembly; executable packaging remains a CLI concern.

use std::path::{Path, PathBuf};

use crate::{
    asm::{
        AssemblyOptions, emit_ez80_assembly_with_options, emit_lr35902_assembly_with_options,
        emit_mos6502_assembly_with_options,
    },
    ast::Program,
    compile::{
        CompileOptions, CompileReport, SdkResolver, check_source_with_sdk,
        parse_and_resolve_imports_with_sdk,
    },
    diagnostic::Diagnostic,
    layout::default_layout_for_target,
    target::{Address24, CpuFamily, DEFAULT_TARGET_TRIPLE, resolve_target_profile},
    tbir::diagnostics::validate_program,
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
    let target = resolve_target_profile(Some(&request.target)).map_err(Diagnostic::new)?;
    let sdk = request.sdk_resolver();
    let options = CompileOptions {
        source: request.source_path.clone(),
        debug_comments: request.debug_comments,
        default_sdk_symbols: request.default_sdk_symbols,
    };
    let report = check_source_with_sdk(source, &options, &sdk)?;
    let program = parse_and_resolve_imports_with_sdk(&request.source_path, source, &sdk)?;
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

    #[test]
    fn compiles_in_memory_source_to_ez80_assembly() {
        let request = CompileRequest::new("memory.ezra", "custom-unknown-ez80");
        let compilation = compile_source_to_assembly("fn main() {}", &request).unwrap();

        assert!(compilation.report.has_main);
        assert!(compilation.assembly.contains("__ezra_start:"));
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
