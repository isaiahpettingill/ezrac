use std::{
    collections::HashSet,
    env,
    ffi::{OsStr, OsString},
    fs,
    path::{Path, PathBuf},
    process::ExitCode,
};

use ezra::{
    asm::{
        AssemblyOptions, AssemblyPreprocessOptions, GameBoyBankingMapper, GameBoyBankingOptions,
        emit_ez80_assembly_with_options, emit_lr35902_assembly_with_options,
        emit_mos6502_assembly_with_options, preprocess_assembly_file,
    },
    ast::Program,
    cart::CartridgeHeader,
    compile::{SdkResolver, load_program_with_sdk_and_embed_resolver},
    diagnostic::{Diagnostic, SourceLocation, diagnostic_span},
    disk::{DiskFile, DiskFormat, DiskRequest, create_disk_image},
    hir::HirProgram,
    layout::{Layout, parse_layout},
    optimization::{OptimizationOptions, OptimizationPass},
    parser::parse_program,
    project::{
        ArduboyConfig, AssetConfig, BankingConfig, GameBoyConfig, GameBoyMapper, SegaConfig,
        ZxSpectrumConfig, load_nearest_project_config, load_project_config,
    },
    target::{
        Address24, AssemblerCpu, CpuFamily, OutputFormat, TargetProfile, parse_output_format,
        parse_target_triple, resolve_target_profile,
    },
    tbir::TbirProgram,
    vm::TestRunOptions,
};

#[cfg(feature = "avr")]
use ezra::asm::emit_avr_assembly_with_options;
#[cfg(feature = "dcpu")]
use ezra::asm::emit_dcpu_assembly_with_options;
#[cfg(feature = "i8086")]
use ezra::asm::emit_i8086_assembly_with_options;
#[cfg(feature = "m68k")]
use ezra::asm::emit_m68k_assembly_with_options;
#[cfg(feature = "m6800")]
use ezra::asm::emit_m6800_assembly_with_options;
#[cfg(feature = "m6809")]
use ezra::asm::emit_m6809_assembly_with_options;
#[cfg(feature = "msp430")]
use ezra::asm::emit_msp430_assembly_with_options;
#[cfg(feature = "pic18")]
use ezra::asm::emit_pic18_assembly_with_options;
#[cfg(feature = "tms9900")]
use ezra::asm::emit_tms9900_assembly_with_options;

mod asset_pipeline;
#[cfg(feature = "lsp")]
mod lsp_server;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let args = env::args_os().skip(1).collect::<Vec<_>>();
    if args.is_empty() {
        print_usage();
        return Ok(());
    }
    let command = args.first().and_then(|arg| arg.to_str()).ok_or_else(|| {
        format!(
            "unknown command (command names must be valid UTF-8)\n{}",
            usage()
        )
    })?;
    match command {
        "check" => {
            let options = CommandOptions::parse(&args[1..])?;
            check(&options)
        }
        "build" => {
            let (build_args, size_budgets) = parse_size_budget_args(&args[1..])?;
            let options = BuildCommandOptions::parse(&build_args)?;
            build_with_size_budgets(&options, &size_budgets)
        }
        "disk" => {
            let options = DiskCommandOptions::parse(&args[1..])?;
            create_disk(&options)
        }
        "emit-asm" => {
            let options = CommandOptions::parse(&args[1..])?;
            emit_asm(&options)
        }
        "emit-ir" => {
            let options = EmitIrOptions::parse(&args[1..])?;
            emit_ir(&options)
        }
        "test" => {
            let options = TestCommandOptions::parse(&args[1..])?;
            match options.path.as_ref() {
                Some(path) => {
                    test_source_with_command_options(&options.command_with_path(path.clone()))
                }
                None => test_project_with_command_options(&options),
            }
        }
        "assemble" => {
            let options = AssembleOptions::parse(&args[1..])?;
            assemble_file(&options)
        }
        "init" => {
            let options = InitOptions::parse(&args[1..])?;
            init_project(&options)
        }
        "install-syntax" => {
            let options = InstallSyntaxOptions::parse(&args[1..])?;
            install_syntax(&options)
        }
        "targets" => {
            print_targets();
            Ok(())
        }
        "lsp" => run_lsp(),
        "layout" => print_layout(args.get(1).map(PathBuf::from).as_deref()),
        "header" => print_header(),
        "-h" | "--help" => {
            print_usage();
            Ok(())
        }
        command => Err(format!("unknown command `{command}`\n{}", usage())),
    }
}

#[cfg(feature = "lsp")]
fn run_lsp() -> Result<(), String> {
    lsp_server::run()
}

#[cfg(not(feature = "lsp"))]
fn run_lsp() -> Result<(), String> {
    Err("`ezrac lsp` requires building with `--features lsp`".to_owned())
}

fn cli_text<T: AsRef<OsStr>>(value: &T) -> Result<String, String> {
    value
        .as_ref()
        .to_str()
        .map(str::to_owned)
        .ok_or_else(|| "non-UTF-8 command option; paths may be non-UTF-8".to_owned())
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct InitOptions {
    path: PathBuf,
    name: Option<String>,
    target: String,
    force: bool,
}

impl InitOptions {
    fn parse<T: AsRef<OsStr>>(args: &[T]) -> Result<Self, String> {
        let mut path = None;
        let mut name = None;
        let mut target = "agonlight-mos-ez80".to_owned();
        let mut force = false;
        let mut iter = args.iter();
        while let Some(raw_arg) = iter.next() {
            let arg = raw_arg.as_ref();
            match arg.to_str() {
                Some("--name") => name = Some(cli_text(iter.next().ok_or_else(usage)?)?),
                Some("--target") => {
                    let value = cli_text(iter.next().ok_or_else(usage)?)?;
                    resolve_target_profile(Some(&value))?;
                    target = value;
                }
                Some("--force") => force = true,
                Some(_) if path.is_none() => path = Some(PathBuf::from(arg)),
                None if path.is_none() => path = Some(PathBuf::from(arg)),
                _ => return Err(usage()),
            }
        }
        Ok(Self {
            path: path.unwrap_or_else(|| PathBuf::from(".")),
            name,
            target,
            force,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct DiskCommandOptions {
    output: PathBuf,
    format: DiskFormat,
    label: String,
    files: Vec<DiskInput>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct DiskInput {
    name: String,
    path: PathBuf,
}

impl DiskCommandOptions {
    fn parse<T: AsRef<OsStr>>(args: &[T]) -> Result<Self, String> {
        let mut output = None;
        let mut format = None;
        let mut label = "EZRA DISK".to_owned();
        let mut files = Vec::new();
        let mut iter = args.iter();
        while let Some(raw_arg) = iter.next() {
            let arg = raw_arg.as_ref();
            match arg.to_str() {
                Some("--output" | "-o") => {
                    output = Some(PathBuf::from(iter.next().ok_or_else(usage)?.as_ref()));
                }
                Some("--format") => {
                    let value = cli_text(iter.next().ok_or_else(usage)?)?;
                    format = Some(DiskFormat::from_name(&value).ok_or_else(|| {
                        format!(
                            "unknown disk format `{value}`; expected m35fd, m35fd-be, fat12-720, fat12-720k, fat12-1440, fat12-1440k, d64, dcpu, dcpu-be, cpm, mos, dos, or c64"
                        )
                    })?);
                }
                Some("--label") => label = cli_text(iter.next().ok_or_else(usage)?)?,
                Some("--file") => files.push(DiskInput::parse(iter.next().ok_or_else(usage)?)?),
                Some(value) if value.starts_with("--file=") => {
                    files.push(DiskInput::from_path(&value["--file=".len()..])?)
                }
                Some(value) if !value.starts_with('-') => files.push(DiskInput::parse(raw_arg)?),
                None => files.push(DiskInput::parse(raw_arg)?),
                _ => return Err(usage()),
            }
        }
        let output = output
            .ok_or_else(|| "disk requires `--output <image.dsk|image.img|image.d64>`".to_owned())?;
        let format = format
            .or_else(|| infer_disk_format(&output))
            .ok_or_else(|| {
                "cannot infer disk format from the output name; use `--format m35fd|m35fd-be|fat12-720|fat12-1440|d64`"
                    .to_owned()
            })?;
        Ok(Self {
            output,
            format,
            label,
            files,
        })
    }
}

impl DiskInput {
    fn parse<T: AsRef<OsStr>>(specification: &T) -> Result<Self, String> {
        let specification = specification.as_ref();
        let Some(text) = specification.to_str() else {
            return Self::from_path_os(specification);
        };
        match text.split_once('=') {
            Some((name, path)) if !name.is_empty() && !path.is_empty() => Ok(Self {
                name: name.to_owned(),
                path: PathBuf::from(path),
            }),
            Some(_) => Err(format!(
                "invalid disk file `{text}`; expected PATH or NAME=PATH"
            )),
            None => Self::from_path(text),
        }
    }

    fn from_path(specification: &str) -> Result<Self, String> {
        Self::from_path_os(OsStr::new(specification))
    }

    fn from_path_os(specification: &OsStr) -> Result<Self, String> {
        let path = PathBuf::from(specification);
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .ok_or_else(|| {
                format!(
                    "cannot derive a disk file name from `{}`; use NAME=PATH",
                    path.display()
                )
            })?
            .to_owned();
        Ok(Self { name, path })
    }
}

fn infer_disk_format(output: &Path) -> Option<DiskFormat> {
    let extension = output.extension()?.to_str()?;
    if extension.eq_ignore_ascii_case("d64") {
        Some(DiskFormat::Commodore1541)
    } else if extension.eq_ignore_ascii_case("dsk") {
        Some(DiskFormat::Fat12_720K)
    } else if extension.eq_ignore_ascii_case("img") {
        Some(DiskFormat::Fat12_1440K)
    } else {
        None
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct InstallSyntaxOptions {
    editors: Vec<SyntaxEditor>,
    all: bool,
    dry_run: bool,
}

impl InstallSyntaxOptions {
    fn parse<T: AsRef<OsStr>>(args: &[T]) -> Result<Self, String> {
        let mut editors = Vec::new();
        let mut all = false;
        let mut dry_run = false;
        let mut iter = args.iter();
        while let Some(raw_arg) = iter.next() {
            let arg = raw_arg.as_ref();
            match arg.to_str() {
                Some("--all") => all = true,
                Some("--dry-run") => dry_run = true,
                Some("--editor") => {
                    let value = cli_text(iter.next().ok_or_else(usage)?)?;
                    editors.push(SyntaxEditor::parse(&value)?);
                }
                Some(value) if !value.starts_with('-') => editors.push(SyntaxEditor::parse(value)?),
                _ => return Err(usage()),
            }
        }
        if all {
            editors = SyntaxEditor::all().to_vec();
        }
        editors.sort();
        editors.dedup();
        if editors.is_empty() {
            return Err(
                "install-syntax requires `--all` or at least one editor name; supported editors: vim, neovim, nano, micro, helix, vscode, zed, notepad++".to_owned(),
            );
        }
        Ok(Self {
            editors,
            all,
            dry_run,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum SyntaxEditor {
    Vim,
    Neovim,
    Nano,
    Micro,
    Helix,
    Vscode,
    Zed,
    NotepadPlusPlus,
}

impl SyntaxEditor {
    fn parse(value: &str) -> Result<Self, String> {
        match value.to_ascii_lowercase().as_str() {
            "vim" => Ok(Self::Vim),
            "neovim" | "nvim" => Ok(Self::Neovim),
            "nano" => Ok(Self::Nano),
            "micro" => Ok(Self::Micro),
            "helix" | "hx" => Ok(Self::Helix),
            "vscode" | "vs-code" | "code" => Ok(Self::Vscode),
            "zed" => Ok(Self::Zed),
            "notepad++" | "notepadpp" | "npp" => Ok(Self::NotepadPlusPlus),
            _ => Err(format!(
                "unsupported editor `{value}`; expected vim, neovim, nano, micro, helix, vscode, zed, or notepad++"
            )),
        }
    }

    const fn all() -> &'static [Self] {
        &[
            Self::Vim,
            Self::Neovim,
            Self::Nano,
            Self::Micro,
            Self::Helix,
            Self::Vscode,
            Self::Zed,
            Self::NotepadPlusPlus,
        ]
    }

    const fn name(self) -> &'static str {
        match self {
            Self::Vim => "vim",
            Self::Neovim => "neovim",
            Self::Nano => "nano",
            Self::Micro => "micro",
            Self::Helix => "helix",
            Self::Vscode => "vscode",
            Self::Zed => "zed",
            Self::NotepadPlusPlus => "notepad++",
        }
    }
}

fn parse_optimization_level(value: &str) -> Result<u8, String> {
    let level = value
        .parse::<u8>()
        .map_err(|_| format!("invalid optimization level `{value}`; expected 0, 1, 2, or 3"))?;
    OptimizationOptions::new(level).map(|options| options.level)
}

fn parse_optimization_pass(value: &str) -> Result<OptimizationPass, String> {
    OptimizationPass::parse(value).ok_or_else(|| {
        format!(
            "unknown optimization pass `{value}`; expected one of: {}",
            OptimizationPass::names()
        )
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct BuildCommandOptions {
    path: Option<PathBuf>,
    debug_comments: bool,
    default_sdk_symbols: bool,
    input_kind: Option<InputKind>,
    assembler_cpu: Option<AssemblerCpu>,
    layout_path: Option<PathBuf>,
    target: Option<String>,
    optimization_level: Option<u8>,
    enable_optimizations: Vec<OptimizationPass>,
    disable_optimizations: Vec<OptimizationPass>,
}

impl BuildCommandOptions {
    fn parse<T: AsRef<OsStr>>(args: &[T]) -> Result<Self, String> {
        let mut path = None;
        let mut debug_comments = false;
        let mut default_sdk_symbols = true;
        let mut input_kind = None;
        let mut assembler_cpu = None;
        let mut layout_path = None;
        let mut target = None;
        let mut optimization_level = None;
        let mut enable_optimizations = Vec::new();
        let mut disable_optimizations = Vec::new();
        let mut iter = args.iter();
        while let Some(raw_arg) = iter.next() {
            let arg = raw_arg.as_ref();
            match arg.to_str() {
                Some("--debug-comments") => debug_comments = true,
                Some("--no-default-sdk-symbols") => default_sdk_symbols = false,
                Some("--input-kind") => {
                    let value = cli_text(iter.next().ok_or_else(usage)?)?;
                    input_kind = Some(InputKind::parse(&value)?);
                }
                Some("--cpu") => {
                    let value = cli_text(iter.next().ok_or_else(usage)?)?;
                    assembler_cpu = Some(AssemblerCpu::parse(&value)?);
                }
                Some("--layout") => {
                    layout_path = Some(PathBuf::from(iter.next().ok_or_else(usage)?.as_ref()));
                }
                Some("--target") => {
                    target = Some(cli_text(iter.next().ok_or_else(usage)?)?);
                }
                Some("-O" | "--opt-level") => {
                    optimization_level = Some(parse_optimization_level(&cli_text(
                        iter.next().ok_or_else(usage)?,
                    )?)?);
                }
                Some(value) if value.starts_with("-O") && value.len() == 3 => {
                    optimization_level = Some(parse_optimization_level(&value[2..])?);
                }
                Some("--enable-optimization") => enable_optimizations.push(
                    parse_optimization_pass(&cli_text(iter.next().ok_or_else(usage)?)?)?,
                ),
                Some("--disable-optimization") => disable_optimizations.push(
                    parse_optimization_pass(&cli_text(iter.next().ok_or_else(usage)?)?)?,
                ),
                Some(_) if path.is_none() => path = Some(PathBuf::from(arg)),
                None if path.is_none() => path = Some(PathBuf::from(arg)),
                _ => return Err(usage()),
            }
        }
        Ok(Self {
            path,
            debug_comments,
            default_sdk_symbols,
            input_kind,
            assembler_cpu,
            layout_path,
            target,
            optimization_level,
            enable_optimizations,
            disable_optimizations,
        })
    }

    #[cfg(test)]
    fn with_path<P: Into<PathBuf>>(path: P, debug_comments: bool) -> Self {
        Self {
            path: Some(path.into()),
            debug_comments,
            default_sdk_symbols: true,
            input_kind: None,
            assembler_cpu: None,
            layout_path: None,
            target: None,
            optimization_level: None,
            enable_optimizations: Vec::new(),
            disable_optimizations: Vec::new(),
        }
    }
}

trait BuildOptionsView {
    fn default_sdk_symbols(&self) -> bool;
    fn input_kind(&self) -> Option<InputKind>;
    fn assembler_cpu(&self) -> Option<AssemblerCpu>;
    fn layout_path(&self) -> Option<&Path>;
    fn target(&self) -> Option<&String>;
    fn optimization_level(&self) -> Option<u8>;
    fn enabled_optimizations(&self) -> &[OptimizationPass];
    fn disabled_optimizations(&self) -> &[OptimizationPass];
}

impl BuildOptionsView for BuildCommandOptions {
    fn default_sdk_symbols(&self) -> bool {
        self.default_sdk_symbols
    }

    fn input_kind(&self) -> Option<InputKind> {
        self.input_kind
    }

    fn assembler_cpu(&self) -> Option<AssemblerCpu> {
        self.assembler_cpu
    }

    fn layout_path(&self) -> Option<&Path> {
        self.layout_path.as_deref()
    }

    fn target(&self) -> Option<&String> {
        self.target.as_ref()
    }

    fn optimization_level(&self) -> Option<u8> {
        self.optimization_level
    }

    fn enabled_optimizations(&self) -> &[OptimizationPass] {
        &self.enable_optimizations
    }

    fn disabled_optimizations(&self) -> &[OptimizationPass] {
        &self.disable_optimizations
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CommandOptions {
    path: PathBuf,
    debug_comments: bool,
    default_sdk_symbols: bool,
    layout_path: Option<PathBuf>,
    target: Option<String>,
    optimization_level: Option<u8>,
    enable_optimizations: Vec<OptimizationPass>,
    disable_optimizations: Vec<OptimizationPass>,
}

impl CommandOptions {
    fn parse<T: AsRef<OsStr>>(args: &[T]) -> Result<Self, String> {
        let mut path = None;
        let mut debug_comments = false;
        let mut default_sdk_symbols = true;
        let mut layout_path = None;
        let mut target = None;
        let mut optimization_level = None;
        let mut enable_optimizations = Vec::new();
        let mut disable_optimizations = Vec::new();
        let mut iter = args.iter();
        while let Some(raw_arg) = iter.next() {
            let arg = raw_arg.as_ref();
            match arg.to_str() {
                Some("--debug-comments") => debug_comments = true,
                Some("--no-default-sdk-symbols") => default_sdk_symbols = false,
                Some("--layout") => {
                    layout_path = Some(PathBuf::from(iter.next().ok_or_else(usage)?.as_ref()))
                }
                Some("--target") => target = Some(cli_text(iter.next().ok_or_else(usage)?)?),
                Some("-O" | "--opt-level") => {
                    optimization_level = Some(parse_optimization_level(&cli_text(
                        iter.next().ok_or_else(usage)?,
                    )?)?);
                }
                Some(value) if value.starts_with("-O") && value.len() == 3 => {
                    optimization_level = Some(parse_optimization_level(&value[2..])?);
                }
                Some("--enable-optimization") => enable_optimizations.push(
                    parse_optimization_pass(&cli_text(iter.next().ok_or_else(usage)?)?)?,
                ),
                Some("--disable-optimization") => disable_optimizations.push(
                    parse_optimization_pass(&cli_text(iter.next().ok_or_else(usage)?)?)?,
                ),
                Some(_) if path.is_none() => path = Some(PathBuf::from(arg)),
                None if path.is_none() => path = Some(PathBuf::from(arg)),
                _ => return Err(usage()),
            }
        }
        Ok(Self {
            path: path.ok_or_else(usage)?,
            debug_comments,
            default_sdk_symbols,
            layout_path,
            target,
            optimization_level,
            enable_optimizations,
            disable_optimizations,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct TestCommandOptions {
    path: Option<PathBuf>,
    debug_comments: bool,
    default_sdk_symbols: bool,
    layout_path: Option<PathBuf>,
    target: Option<String>,
    optimization_level: Option<u8>,
    enable_optimizations: Vec<OptimizationPass>,
    disable_optimizations: Vec<OptimizationPass>,
}

impl TestCommandOptions {
    fn parse<T: AsRef<OsStr>>(args: &[T]) -> Result<Self, String> {
        let mut path = None;
        let mut debug_comments = false;
        let mut default_sdk_symbols = true;
        let mut layout_path = None;
        let mut target = None;
        let mut optimization_level = None;
        let mut enable_optimizations = Vec::new();
        let mut disable_optimizations = Vec::new();
        let mut iter = args.iter();
        while let Some(raw_arg) = iter.next() {
            let arg = raw_arg.as_ref();
            match arg.to_str() {
                Some("--debug-comments") => debug_comments = true,
                Some("--no-default-sdk-symbols") => default_sdk_symbols = false,
                Some("--layout") => {
                    layout_path = Some(PathBuf::from(iter.next().ok_or_else(usage)?.as_ref()))
                }
                Some("--target") => target = Some(cli_text(iter.next().ok_or_else(usage)?)?),
                Some("-O" | "--opt-level") => {
                    optimization_level = Some(parse_optimization_level(&cli_text(
                        iter.next().ok_or_else(usage)?,
                    )?)?);
                }
                Some(value) if value.starts_with("-O") && value.len() == 3 => {
                    optimization_level = Some(parse_optimization_level(&value[2..])?);
                }
                Some("--enable-optimization") => enable_optimizations.push(
                    parse_optimization_pass(&cli_text(iter.next().ok_or_else(usage)?)?)?,
                ),
                Some("--disable-optimization") => disable_optimizations.push(
                    parse_optimization_pass(&cli_text(iter.next().ok_or_else(usage)?)?)?,
                ),
                Some(_) if path.is_none() => path = Some(PathBuf::from(arg)),
                None if path.is_none() => path = Some(PathBuf::from(arg)),
                _ => return Err(usage()),
            }
        }
        Ok(Self {
            path,
            debug_comments,
            default_sdk_symbols,
            layout_path,
            target,
            optimization_level,
            enable_optimizations,
            disable_optimizations,
        })
    }

    fn command_with_path(&self, path: PathBuf) -> CommandOptions {
        CommandOptions {
            path,
            debug_comments: self.debug_comments,
            default_sdk_symbols: self.default_sdk_symbols,
            layout_path: self.layout_path.clone(),
            target: self.target.clone(),
            optimization_level: self.optimization_level,
            enable_optimizations: self.enable_optimizations.clone(),
            disable_optimizations: self.disable_optimizations.clone(),
        }
    }
}

impl BuildOptionsView for CommandOptions {
    fn default_sdk_symbols(&self) -> bool {
        self.default_sdk_symbols
    }

    fn input_kind(&self) -> Option<InputKind> {
        None
    }

    fn assembler_cpu(&self) -> Option<AssemblerCpu> {
        None
    }

    fn layout_path(&self) -> Option<&Path> {
        self.layout_path.as_deref()
    }

    fn target(&self) -> Option<&String> {
        self.target.as_ref()
    }

    fn optimization_level(&self) -> Option<u8> {
        self.optimization_level
    }

    fn enabled_optimizations(&self) -> &[OptimizationPass] {
        &self.enable_optimizations
    }

    fn disabled_optimizations(&self) -> &[OptimizationPass] {
        &self.disable_optimizations
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct AssembleOptions {
    path: PathBuf,
    output: Option<PathBuf>,
    base_addr: Option<u32>,
    assembler_cpu: Option<AssemblerCpu>,
    layout_path: Option<PathBuf>,
    map_path: Option<PathBuf>,
    target: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct EmitIrOptions {
    command: CommandOptions,
    stage: IrStage,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum IrStage {
    Hir,
    Tbir,
}

impl EmitIrOptions {
    fn parse<T: AsRef<OsStr>>(args: &[T]) -> Result<Self, String> {
        let mut rest = Vec::<OsString>::new();
        let mut stage = IrStage::Tbir;
        let mut iter = args.iter();
        while let Some(raw_arg) = iter.next() {
            if raw_arg.as_ref() == OsStr::new("--stage") {
                let value = cli_text(iter.next().ok_or_else(usage)?)?;
                stage = IrStage::parse(&value)?;
            } else {
                rest.push(raw_arg.as_ref().to_os_string());
            }
        }
        Ok(Self {
            command: CommandOptions::parse(&rest)?,
            stage,
        })
    }
}

impl IrStage {
    fn parse(text: &str) -> Result<Self, String> {
        match text {
            "hir" => Ok(Self::Hir),
            "tbir" => Ok(Self::Tbir),
            _ => Err(format!(
                "unknown IR stage `{text}`; expected `hir` or `tbir`"
            )),
        }
    }
}

impl AssembleOptions {
    fn parse<T: AsRef<OsStr>>(args: &[T]) -> Result<Self, String> {
        let mut path = None;
        let mut output = None;
        let mut base_addr = None;
        let mut assembler_cpu = None;
        let mut layout_path = None;
        let mut map_path = None;
        let mut target = None;
        let mut iter = args.iter();
        while let Some(raw_arg) = iter.next() {
            let arg = raw_arg.as_ref();
            match arg.to_str() {
                Some("--output" | "-o") => {
                    output = Some(PathBuf::from(iter.next().ok_or_else(usage)?.as_ref()))
                }
                Some("--base") => {
                    let value = cli_text(iter.next().ok_or_else(usage)?)?;
                    base_addr = Some(parse_cli_u24(&value)?);
                }
                Some("--cpu") => {
                    let value = cli_text(iter.next().ok_or_else(usage)?)?;
                    assembler_cpu = Some(AssemblerCpu::parse(&value)?);
                }
                Some("--layout") => {
                    layout_path = Some(PathBuf::from(iter.next().ok_or_else(usage)?.as_ref()))
                }
                Some("--map") => {
                    map_path = Some(PathBuf::from(iter.next().ok_or_else(usage)?.as_ref()))
                }
                Some("--target") => target = Some(cli_text(iter.next().ok_or_else(usage)?)?),
                Some(_) if path.is_none() => path = Some(PathBuf::from(arg)),
                None if path.is_none() => path = Some(PathBuf::from(arg)),
                _ => return Err(usage()),
            }
        }
        Ok(Self {
            path: path.ok_or_else(usage)?,
            output,
            base_addr,
            assembler_cpu,
            layout_path,
            map_path,
            target,
        })
    }
}

fn parse_cli_u24(text: &str) -> Result<u32, String> {
    let value = if let Some(hex) = text.strip_suffix('h') {
        u32::from_str_radix(hex, 16)
    } else if let Some(hex) = text.strip_prefix("0x") {
        u32::from_str_radix(hex, 16)
    } else {
        text.parse()
    }
    .map_err(|_| format!("invalid numeric operand `{text}`"))?;
    if value > Address24::MAX {
        return Err(format!(
            "address operand `{text}` is outside the 24-bit address space"
        ));
    }
    Ok(value)
}

fn assemble_file(options: &AssembleOptions) -> Result<(), String> {
    let source_path = options.path.clone();
    let target = resolve_target_profile(options.target.as_deref())?;
    let layout_path = options.layout_path.clone();
    let layout = load_layout(layout_path.as_deref(), &target.triple.value)?;
    if let Err(errors) = layout.validate() {
        let message = format_layout_errors(layout_path.as_deref(), errors);
        return Err(format!("layout is invalid:\n{message}"));
    }
    validate_layout_for_target_profile(&target, &layout)?;
    let output_format = target.output_format;
    let assembler_cpu = options
        .assembler_cpu
        .unwrap_or_else(|| AssemblerCpu::from(target.triple.cpu));
    validate_assembler_cpu_for_target(&target, assembler_cpu)?;
    let settings = BuildSettings {
        sdk: SdkResolver {
            target: Some(target.triple.value.clone()),
            sdk_roots: Vec::new(),
        },
        target,
        output_format,
        input_kind: Some(InputKind::Assembly),
        assembler_cpu,
        layout,
        layout_path,
        asset_config: AssetConfig::default(),
        project_root: source_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf(),
        gameboy: None,
        sega: None,
        gameboy_banking: None,
        arduboy: None,
        zxspectrum: None,
        default_sdk_symbols: true,
        optimization: OptimizationOptions::default(),
        output_root: source_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("target"),
        executable_name: None,
        size_budgets: ezra::api::SizeBudgets::default(),
    };
    if let Some(base_addr) = options.base_addr {
        let max_addr = max_address_for_target(&settings.target);
        if base_addr > max_addr {
            return Err(format!(
                "base address 0x{base_addr:X} is outside the {}-bit address space for target `{}`",
                settings.target.memory.address_width_bits, settings.target.triple.value
            ));
        }
        if settings.output_format == OutputFormat::GameBoyGb && base_addr != 0x0150 {
            return Err("Game Boy assembly must use base address 0x0150".to_owned());
        }
    }
    let preprocessed = preprocess_assembly_file(
        &source_path,
        AssemblyPreprocessOptions::for_compiled_features(
            &settings.target.triple.value,
            settings.assembler_cpu.as_str(),
        ),
    )
    .map_err(|error| error.to_string())?;
    let output_path = options
        .output
        .clone()
        .unwrap_or_else(|| source_path.with_extension(executable_extension(&settings)));
    let mut build_request = shared_build_request(&settings, &source_path)?;
    if settings.executable_name.is_none()
        && let Some(name) = output_path.file_stem().and_then(|name| name.to_str())
    {
        build_request.executable_name = Some(name.to_owned());
        build_request.package_context.executable_name = Some(name.to_owned());
    }
    let linked = if let Some(base_addr) = options.base_addr {
        ezra::api::link_assembly_program_at(
            &source_path,
            &preprocessed.program,
            base_addr,
            &build_request,
        )
    } else {
        build_request.package_context.image_kind = ezra::package::PackageImageKind::LoadImage;
        ezra::api::link_assembly_program(&source_path, &preprocessed.program, &build_request)
    }
    .map_err(|error| error.to_string())?;
    create_parent_directory(&output_path)?;
    fs::write(&output_path, linked.executable)
        .map_err(|error| format!("failed to write {}: {error}", output_path.display()))?;
    println!("wrote {}", output_path.display());
    if let Some(map_path) = options.map_path.as_ref() {
        create_parent_directory(map_path)?;
        fs::write(map_path, linked.map)
            .map_err(|error| format!("failed to write {}: {error}", map_path.display()))?;
        println!("wrote {}", map_path.display());
    }
    Ok(())
}

fn create_parent_directory(path: &Path) -> Result<(), String> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
    }
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct BuildSettings {
    sdk: SdkResolver,
    target: TargetProfile,
    output_format: OutputFormat,
    input_kind: Option<InputKind>,
    assembler_cpu: AssemblerCpu,
    layout: Layout,
    layout_path: Option<PathBuf>,
    asset_config: AssetConfig,
    project_root: PathBuf,
    gameboy: Option<GameBoyConfig>,
    sega: Option<SegaConfig>,
    gameboy_banking: Option<GameBoyBankingOptions>,
    arduboy: Option<ArduboyConfig>,
    zxspectrum: Option<ZxSpectrumConfig>,
    default_sdk_symbols: bool,
    optimization: OptimizationOptions,
    output_root: PathBuf,
    executable_name: Option<String>,
    size_budgets: ezra::api::SizeBudgets,
}

fn configured_assembly_options(
    settings: &BuildSettings,
    program: &Program,
    debug_comments: bool,
) -> Result<AssemblyOptions, String> {
    let mut options = ezra::api::assembly_options_for_layout_and_program(
        &settings.layout,
        program,
        settings.target.triple.cpu,
        &settings.target.triple.value,
        debug_comments,
        settings.default_sdk_symbols,
        settings.gameboy_banking,
    )
    .map_err(|error| error.to_string())?;
    options.optimization = settings.optimization.clone();
    Ok(options)
}

fn shared_build_request(
    settings: &BuildSettings,
    source_path: &Path,
) -> Result<ezra::api::BuildRequest, String> {
    let executable_name = settings.executable_name.clone().or_else(|| {
        source_path
            .file_stem()
            .and_then(|name| name.to_str())
            .map(str::to_owned)
    });
    let mut package_context = ezra::package::PackageContext::new();
    package_context.executable_name = executable_name.clone();
    if let Some(config) = &settings.arduboy {
        package_context.arduboy = Some(ezra::package::ArduboyPackageOptions {
            title: config.title.clone(),
            author: config.author.clone(),
            version: config.version.clone(),
            description: config.description.clone(),
            date: config.date.clone(),
            genre: config.genre.clone(),
            source_url: config.source_url.clone(),
        });
    }
    if let Some(config) = &settings.sega {
        let bank_payloads = config
            .bank_files
            .iter()
            .map(|path| {
                fs::read(path).map_err(|error| {
                    format!(
                        "failed to read Sega ROM bank file `{}`: {error}",
                        path.display()
                    )
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        package_context.sega = Some(ezra::package::SegaPackageOptions {
            rom_size_kib: config.rom_size_kib,
            bank_payloads,
        });
    }
    if let Some(config) = &settings.gameboy {
        let mapper = match config.mapper {
            GameBoyMapper::RomOnly => ezra::package::GameBoyMapper::RomOnly,
            GameBoyMapper::Mbc1 => ezra::package::GameBoyMapper::Mbc1,
            GameBoyMapper::Mbc5 => ezra::package::GameBoyMapper::Mbc5,
        };
        let bank_payloads = config
            .bank_files
            .iter()
            .map(|path| {
                fs::read(path).map_err(|error| {
                    format!(
                        "failed to read Game Boy ROM bank file `{}`: {error}",
                        path.display()
                    )
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        package_context.game_boy = Some(ezra::package::GameBoyPackageOptions {
            mapper,
            rom_banks: config.rom_banks,
            ram_banks: config.ram_banks,
            battery: config.battery,
            rumble: config.rumble,
            bank_payloads,
            generated_bank_payloads: Vec::new(),
            explicit_banking: settings.gameboy_banking.is_some(),
        });
    }
    if let Some(config) = &settings.zxspectrum {
        let banks = config
            .banks
            .iter()
            .map(|bank| {
                let bytes = fs::read(&bank.file).map_err(|error| {
                    format!(
                        "failed to read ZX Spectrum RAM page {} payload `{}`: {error}",
                        bank.page,
                        bank.file.display()
                    )
                })?;
                Ok(ezra::package::ZxSpectrumBankPayload {
                    page: bank.page,
                    name: bank.name.clone(),
                    bytes,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        package_context.zx_spectrum = Some(ezra::package::ZxSpectrumPackageOptions { banks });
    }
    Ok(ezra::api::BuildRequest {
        target: settings.target.clone(),
        output_format: settings.output_format,
        assembler_cpu: settings.assembler_cpu,
        layout: settings.layout.clone(),
        executable_name,
        gameboy_banking: settings.gameboy_banking,
        package_context,
        size_budgets: settings.size_budgets.clone(),
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InputKind {
    Ezra,
    Assembly,
}

impl InputKind {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "ezra" => Ok(Self::Ezra),
            "assembly" => Ok(Self::Assembly),
            _ => Err(format!(
                "unsupported input kind `{value}`; expected `ezra` or `assembly`"
            )),
        }
    }
}

fn resolve_build_settings(
    options: &impl BuildOptionsView,
    source_path: &Path,
) -> Result<BuildSettings, String> {
    resolve_build_settings_with_budgets(options, source_path, &ezra::api::SizeBudgets::default())
}

fn resolve_build_settings_with_budgets(
    options: &impl BuildOptionsView,
    source_path: &Path,
    size_budgets: &ezra::api::SizeBudgets,
) -> Result<BuildSettings, String> {
    let project = load_nearest_project_config(source_path).map_err(|error| error.to_string())?;
    let target_name = options.target().map(String::as_str).or_else(|| {
        project
            .as_ref()
            .and_then(|project| project.target.as_deref())
    });
    let target = resolve_target_profile(target_name)?;
    let output_format = project
        .as_ref()
        .and_then(|project| project.output.as_deref())
        .map(parse_output_format)
        .transpose()?
        .unwrap_or(target.output_format);
    let input_kind = match options.input_kind() {
        Some(input_kind) => Some(input_kind),
        None => project
            .as_ref()
            .and_then(|project| project.input_kind.as_deref())
            .map(InputKind::parse)
            .transpose()?,
    };
    let assembler_cpu = match options.assembler_cpu() {
        Some(cpu) => cpu,
        None => project
            .as_ref()
            .and_then(|project| project.assembler_cpu.as_deref())
            .map(AssemblerCpu::parse)
            .transpose()?
            .unwrap_or_else(|| AssemblerCpu::from(target.triple.cpu)),
    };
    validate_assembler_cpu_for_target(&target, assembler_cpu)?;
    if target.triple.value.starts_with("nes-") && output_format != OutputFormat::NesRom {
        return Err("NES targets require `.nes` output".to_owned());
    }
    let layout_path = options.layout_path().map(Path::to_path_buf).or_else(|| {
        project
            .as_ref()
            .and_then(|project| project.layout_file.clone())
    });
    let layout = match layout_path.as_deref() {
        Some(path) => load_layout(Some(path), &target.triple.value)?,
        None if output_format == OutputFormat::Commodore64Crt => Layout::commodore64_crt(),
        None => default_layout_for_target(&target.triple.value),
    };
    let default_sdk_symbols = options.default_sdk_symbols() && target.default_sdk_symbols;
    let mut optimization = project
        .as_ref()
        .map(|project| project.optimization.clone())
        .unwrap_or_default();
    if let Some(level) = options.optimization_level() {
        optimization.level = level;
    }
    for pass in options.enabled_optimizations() {
        optimization.enable(*pass);
    }
    for pass in options.disabled_optimizations() {
        optimization.disable(*pass);
    }
    let sdk = SdkResolver {
        target: Some(target.triple.value.clone()),
        sdk_roots: project
            .as_ref()
            .map(|project| project.sdk_paths.clone())
            .unwrap_or_default(),
    };
    let project_root = project
        .as_ref()
        .map(|project| project.root.clone())
        .unwrap_or_else(|| {
            source_path
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .to_path_buf()
        });
    let output_root = project_root.join("target");
    let executable_name = project
        .as_ref()
        .and_then(|project| project.executable.clone());
    let asset_config = project
        .as_ref()
        .map(|project| project.assets.clone())
        .unwrap_or_default();
    let gameboy = project.as_ref().and_then(|project| project.gameboy.clone());
    let sega = project.as_ref().and_then(|project| project.sega.clone());
    let is_sega_8bit = matches!(
        target.triple.value.as_str(),
        "sega-master-system-z80" | "sega-game-gear-z80"
    );
    if sega.is_some() && !is_sega_8bit {
        return Err(
            "project `[sega]` configuration requires `sega-master-system-z80` or `sega-game-gear-z80`"
                .to_owned(),
        );
    }
    if gameboy.is_some() && !target.triple.value.starts_with("gameboy-") {
        return Err("project `[gameboy]` configuration requires a `gameboy-*` target".to_owned());
    }
    let banking = project
        .as_ref()
        .map(|project| project.banking.clone())
        .unwrap_or_else(BankingConfig::default);
    let gameboy_banking = if banking.enabled && target.triple.value.starts_with("gameboy-") {
        let mapper = gameboy
            .as_ref()
            .map(|config| config.mapper)
            .unwrap_or_default();
        let mapper = match mapper {
            GameBoyMapper::Mbc1 => GameBoyBankingMapper::Mbc1,
            GameBoyMapper::Mbc5 => GameBoyBankingMapper::Mbc5,
            GameBoyMapper::RomOnly => {
                return Err(
                    "Game Boy `[banking] enabled = true` requires `[gameboy] mapper = \"mbc1\"` or `\"mbc5\"`; `rom-only` cannot select switchable ROM banks"
                        .to_owned(),
                );
            }
        };
        Some(GameBoyBankingOptions { mapper })
    } else {
        None
    };
    let arduboy = project.as_ref().and_then(|project| project.arduboy.clone());
    if arduboy.is_some() && !target.triple.value.starts_with("arduboy-") {
        return Err("project `[arduboy]` configuration requires an `arduboy-*` target".to_owned());
    }
    let zxspectrum = project
        .as_ref()
        .and_then(|project| project.zxspectrum.clone());
    if zxspectrum.is_some() && target.triple.value != "zxspectrum-z80-128k" {
        return Err(
            "project `[zxspectrum]` bank configuration requires the `zxspectrum-z80-128k` target"
                .to_owned(),
        );
    }
    if target.triple.value == "zxspectrum-z80-128k" {
        if output_format != OutputFormat::ZxSpectrumTap {
            return Err("the `zxspectrum-z80-128k` target requires `.tap` output".to_owned());
        }
        if layout_path.is_some() {
            return Err(
                "the `zxspectrum-z80-128k` target does not support custom layouts; its fixed-RAM stack and pageable window are required for safe banking"
                    .to_owned(),
            );
        }
    }

    Ok(BuildSettings {
        sdk,
        target,
        output_format,
        input_kind,
        assembler_cpu,
        layout,
        layout_path,
        asset_config,
        project_root,
        gameboy,
        sega,
        gameboy_banking,
        arduboy,
        zxspectrum,
        default_sdk_symbols,
        optimization,
        output_root,
        executable_name,
        size_budgets: size_budgets.clone(),
    })
}

fn ensure_source_codegen_supported(settings: &BuildSettings) -> Result<(), String> {
    if settings.target.triple.value.starts_with("nes-") {
        return Ok(());
    }
    if matches!(
        settings.target.triple.cpu,
        CpuFamily::Ez80
            | CpuFamily::Z80
            | CpuFamily::R800
            | CpuFamily::Z80N
            | CpuFamily::Z180
            | CpuFamily::I8080
            | CpuFamily::I8085
            | CpuFamily::I8086
            | CpuFamily::Lr35902
            | CpuFamily::Avr
            | CpuFamily::Pic18
            | CpuFamily::Mos6502
            | CpuFamily::Cmos65C02
            | CpuFamily::Wdc65C816
            | CpuFamily::Ricoh2A03
            | CpuFamily::Tms9900
    ) {
        return Ok(());
    }
    #[cfg(feature = "m6800")]
    if settings.target.triple.cpu == CpuFamily::M6800 {
        return Ok(());
    }
    #[cfg(feature = "m6809")]
    if settings.target.triple.cpu == CpuFamily::M6809 {
        return Ok(());
    }
    #[cfg(feature = "tms9900")]
    if settings.target.triple.cpu == CpuFamily::Tms9900 {
        return Ok(());
    }
    #[cfg(feature = "msp430")]
    if matches!(
        settings.target.triple.cpu,
        CpuFamily::Msp430 | CpuFamily::Msp430X | CpuFamily::Msp430X2
    ) {
        return Ok(());
    }
    #[cfg(feature = "dcpu")]
    if settings.target.triple.cpu == CpuFamily::Dcpu {
        return Ok(());
    }
    #[cfg(feature = "m68k")]
    if settings.target.triple.cpu == CpuFamily::M68k {
        return Ok(());
    }

    Err(format!(
        "source codegen is not implemented for target {} CPU {}",
        settings.target.triple.value,
        settings.target.triple.cpu.as_str()
    ))
}

fn load_program_for_cli(
    source_path: &Path,
    settings: &BuildSettings,
) -> Result<Program, Diagnostic> {
    let resolver = asset_pipeline::ConfiguredImageResolver::new(
        &settings.project_root,
        &settings.target.triple.value,
        &settings.asset_config,
    );
    load_program_with_sdk_and_embed_resolver(source_path, &settings.sdk, &resolver)
}

fn apply_asset_configuration(program: &mut Program, settings: &BuildSettings) {
    let placement = settings
        .asset_config
        .placement_for(&settings.target.triple.value);
    for declaration in &mut program.declarations {
        let ezra::ast::Declaration::Embed(embed) = declaration else {
            continue;
        };
        if embed.section.is_none() {
            embed.section.clone_from(&placement.section);
        }
        if embed.align.is_none()
            && let Some(align) = placement.align
        {
            embed.align = Some(ezra::ast::Expr::Int(i64::from(align)));
        }
    }
}

fn emit_source_assembly(
    program: &Program,
    options: AssemblyOptions,
) -> Result<String, ezra::diagnostic::Diagnostic> {
    ezra::tbir::diagnostics::validate_program(program, options.cpu)?;
    if options.cpu == CpuFamily::I8086 {
        #[cfg(feature = "i8086")]
        {
            emit_i8086_assembly_with_options(program, options)
        }
        #[cfg(not(feature = "i8086"))]
        {
            unreachable!("i8086 targets require the i8086 Cargo feature")
        }
    } else if options.cpu == CpuFamily::Lr35902 {
        emit_lr35902_assembly_with_options(program, options)
    } else if options.cpu == CpuFamily::Avr {
        #[cfg(feature = "avr")]
        {
            emit_avr_assembly_with_options(program, options)
        }
        #[cfg(not(feature = "avr"))]
        {
            unreachable!("AVR targets require the avr Cargo feature")
        }
    } else if options.cpu == CpuFamily::Pic18 {
        #[cfg(feature = "pic18")]
        {
            emit_pic18_assembly_with_options(program, options)
        }
        #[cfg(not(feature = "pic18"))]
        {
            unreachable!("PIC18 targets require the pic18 Cargo feature")
        }
    } else if matches!(
        options.cpu,
        CpuFamily::Mos6502 | CpuFamily::Cmos65C02 | CpuFamily::Wdc65C816 | CpuFamily::Ricoh2A03
    ) {
        emit_mos6502_assembly_with_options(program, options)
    } else if options.cpu == CpuFamily::Dcpu {
        #[cfg(feature = "dcpu")]
        {
            emit_dcpu_assembly_with_options(program, options)
        }
        #[cfg(not(feature = "dcpu"))]
        {
            unreachable!("DCPU-16 targets require the dcpu Cargo feature")
        }
    } else if options.cpu == CpuFamily::M6800 {
        #[cfg(feature = "m6800")]
        {
            emit_m6800_assembly_with_options(program, options)
        }
        #[cfg(not(feature = "m6800"))]
        {
            unreachable!("M6800 targets require the m6800 Cargo feature")
        }
    } else if options.cpu == CpuFamily::M6809 {
        #[cfg(feature = "m6809")]
        {
            emit_m6809_assembly_with_options(program, options)
        }
        #[cfg(not(feature = "m6809"))]
        {
            unreachable!("M6809 targets require the m6809 Cargo feature")
        }
    } else if options.cpu == CpuFamily::Tms9900 {
        #[cfg(feature = "tms9900")]
        {
            emit_tms9900_assembly_with_options(program, options)
        }
        #[cfg(not(feature = "tms9900"))]
        {
            unreachable!("TMS9900 targets require the tms9900 Cargo feature")
        }
    } else if matches!(
        options.cpu,
        CpuFamily::Msp430 | CpuFamily::Msp430X | CpuFamily::Msp430X2
    ) {
        #[cfg(feature = "msp430")]
        {
            emit_msp430_assembly_with_options(program, options)
        }
        #[cfg(not(feature = "msp430"))]
        {
            unreachable!("MSP430 targets require the msp430 Cargo feature")
        }
    } else if options.cpu == CpuFamily::M68k {
        #[cfg(feature = "m68k")]
        {
            emit_m68k_assembly_with_options(program, options)
        }
        #[cfg(not(feature = "m68k"))]
        {
            unreachable!("m68k targets require the m68k Cargo feature")
        }
    } else {
        emit_ez80_assembly_with_options(program, options)
    }
}

fn validate_layout_for_target(settings: &BuildSettings) -> Result<(), String> {
    validate_layout_for_target_profile(&settings.target, &settings.layout)
}

fn validate_assembler_cpu_for_target(
    target: &TargetProfile,
    assembler_cpu: AssemblerCpu,
) -> Result<(), String> {
    if target.triple.value.starts_with("nes-") && assembler_cpu != AssemblerCpu::Ricoh2A03 {
        return Err(format!(
            "target `{}` requires assembler CPU `2a03`, not `{}`",
            target.triple.value,
            assembler_cpu.as_str()
        ));
    }
    if target.triple.value.starts_with("snes-") && assembler_cpu != AssemblerCpu::Wdc65C816 {
        return Err(format!(
            "target `{}` requires assembler CPU `65c816`, not `{}`",
            target.triple.value,
            assembler_cpu.as_str()
        ));
    }
    Ok(())
}

fn max_address_for_target(target: &TargetProfile) -> u32 {
    if target.memory.address_width_bits >= 24 {
        Address24::MAX
    } else {
        (1u32 << target.memory.address_width_bits) - 1
    }
}

fn validate_layout_for_target_profile(
    target: &TargetProfile,
    layout: &Layout,
) -> Result<(), String> {
    let max_addr = max_address_for_target(target);
    let mut violations = Vec::new();
    if layout.load.get() > max_addr {
        violations.push(format!("load address {}", layout.load));
    }
    if layout.entry.get() > max_addr {
        violations.push(format!("entry address {}", layout.entry));
    }
    if layout.stack.get() > max_addr {
        violations.push(format!("stack address {}", layout.stack));
    }
    for region in &layout.regions {
        if region.start.get() > max_addr || region.end.get() > max_addr {
            violations.push(format!(
                "region `{}` range {}..{}",
                region.name, region.start, region.end
            ));
        }
    }
    for symbol in &layout.symbols {
        if symbol.value.get() > max_addr {
            violations.push(format!("symbol `{}` value {}", symbol.name, symbol.value));
        }
    }

    if violations.is_empty() {
        return Ok(());
    }

    Err(format!(
        "layout `{}` requires addresses outside the {}-bit address space for target `{}`: {}",
        layout.name,
        target.memory.address_width_bits,
        target.triple.value,
        violations.join(", ")
    ))
}

fn parse_size_budget_args<T: AsRef<OsStr>>(
    args: &[T],
) -> Result<(Vec<OsString>, ezra::api::SizeBudgets), String> {
    let mut remaining = Vec::new();
    let mut budgets = ezra::api::SizeBudgets::default();
    let mut iter = args.iter();
    while let Some(raw_arg) = iter.next() {
        let arg = raw_arg.as_ref();
        let specification = if arg == OsStr::new("--size-budget") {
            Some(cli_text(iter.next().ok_or_else(usage)?)?)
        } else {
            arg.to_str()
                .and_then(|value| value.strip_prefix("--size-budget=").map(str::to_owned))
        };
        if let Some(specification) = specification {
            add_size_budget(&mut budgets, &specification)?;
        } else {
            remaining.push(arg.to_os_string());
        }
    }
    Ok((remaining, budgets))
}

fn add_size_budget(
    budgets: &mut ezra::api::SizeBudgets,
    specification: &str,
) -> Result<(), String> {
    let (name, raw_limit) = specification.split_once('=').ok_or_else(|| {
        format!("invalid size budget `{specification}`; expected NAME=BYTES, such as `.text=4096`")
    })?;
    if name.is_empty() {
        return Err("size budget name cannot be empty".to_owned());
    }
    let limit = parse_size_bytes(raw_limit)?;
    match name {
        "target" | "package" | "final_package" => budgets.target = Some(limit),
        "runtime_helpers" | "runtime-helpers" | "helpers" => budgets.runtime_helpers = Some(limit),
        _ => {
            budgets.sections.insert(name.to_owned(), limit);
        }
    }
    Ok(())
}

fn parse_size_bytes(text: &str) -> Result<usize, String> {
    let value = text.replace('_', "");
    let parsed = if let Some(hex) = value.strip_prefix("0x") {
        usize::from_str_radix(hex, 16)
    } else if let Some(hex) = value.strip_suffix('h') {
        usize::from_str_radix(hex, 16)
    } else {
        value.parse()
    }
    .map_err(|_| format!("invalid size budget byte count `{text}`"))?;
    Ok(parsed)
}

fn build(options: &BuildCommandOptions) -> Result<(), String> {
    build_with_size_budgets(options, &ezra::api::SizeBudgets::default())
}

fn build_with_size_budgets(
    options: &BuildCommandOptions,
    size_budgets: &ezra::api::SizeBudgets,
) -> Result<(), String> {
    let source_path = resolve_build_source_path(options)?;
    let targets = if let Some(target) = &options.target {
        vec![Some(target.clone())]
    } else {
        load_nearest_project_config(&source_path)
            .map_err(|error| error.to_string())?
            .map(|project| project.targets.into_iter().map(Some).collect())
            .filter(|targets: &Vec<Option<String>>| !targets.is_empty())
            .unwrap_or_else(|| vec![None])
    };

    for target in targets {
        let mut target_options = options.clone();
        target_options.path = Some(source_path.clone());
        target_options.target = target;
        let outputs = build_source_with_build_options_and_budgets(&target_options, size_budgets)?;
        println!("wrote {}", outputs.asm.display());
        println!("wrote {}", outputs.map.display());
        println!("wrote {}", outputs.size.display());
        println!("wrote {}", outputs.executable.display());
    }
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct BuildOutputs {
    asm: PathBuf,
    map: PathBuf,
    size: PathBuf,
    executable: PathBuf,
}

#[cfg(test)]
fn build_source(path: &str) -> Result<BuildOutputs, String> {
    build_source_with_options(path, false)
}

#[cfg(test)]
fn build_source_with_options(path: &str, debug_comments: bool) -> Result<BuildOutputs, String> {
    build_source_with_build_options(&BuildCommandOptions::with_path(
        path.to_owned(),
        debug_comments,
    ))
}

#[cfg(test)]
fn build_source_with_command_options(options: &CommandOptions) -> Result<BuildOutputs, String> {
    build_source_with_build_options(&BuildCommandOptions {
        path: Some(options.path.clone()),
        debug_comments: options.debug_comments,
        default_sdk_symbols: options.default_sdk_symbols,
        input_kind: None,
        assembler_cpu: None,
        layout_path: options.layout_path.clone(),
        target: options.target.clone(),
        optimization_level: options.optimization_level,
        enable_optimizations: options.enable_optimizations.clone(),
        disable_optimizations: options.disable_optimizations.clone(),
    })
}

#[cfg(test)]
fn build_source_with_build_options(options: &BuildCommandOptions) -> Result<BuildOutputs, String> {
    build_source_with_build_options_and_budgets(options, &ezra::api::SizeBudgets::default())
}

fn build_source_with_build_options_and_budgets(
    options: &BuildCommandOptions,
    size_budgets: &ezra::api::SizeBudgets,
) -> Result<BuildOutputs, String> {
    let source_path = resolve_build_source_path(options)?;
    let source_location = command_source_start_location(&source_path);
    let settings = resolve_build_settings_with_budgets(options, &source_path, size_budgets)?;
    validate_build_layout(&settings)?;
    match detect_input_kind(&source_path, &settings)? {
        InputKind::Ezra => build_ezra_source(&source_path, source_location, &settings, options),
        InputKind::Assembly => build_assembly_source(&source_path, source_location, &settings),
    }
}

fn resolve_build_source_path(options: &BuildCommandOptions) -> Result<PathBuf, String> {
    if let Some(path) = &options.path {
        return Ok(path.clone());
    }

    let cwd =
        env::current_dir().map_err(|error| format!("failed to read current directory: {error}"))?;
    let project_probe = cwd.join("Ezra.toml");
    let project = load_nearest_project_config(&project_probe)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| {
            "build requires a source path or an ancestor `Ezra.toml` must define `build.input`"
                .to_owned()
        })?;
    project.input.ok_or_else(|| {
        format!(
            "build requires a source path or `{}` must define `build.input`",
            project.path.display()
        )
    })
}

fn validate_build_layout(settings: &BuildSettings) -> Result<(), String> {
    if let Err(errors) = settings.layout.validate() {
        let message = format_layout_errors(settings.layout_path.as_deref(), errors);
        return Err(format!("layout is invalid:\n{message}"));
    }
    validate_layout_for_target(settings)
}

fn detect_input_kind(source_path: &Path, settings: &BuildSettings) -> Result<InputKind, String> {
    if let Some(input_kind) = settings.input_kind {
        return Ok(input_kind);
    }
    match source_path.extension().and_then(|ext| ext.to_str()) {
        Some(ext) if ext.eq_ignore_ascii_case("ezra") => Ok(InputKind::Ezra),
        Some(ext)
            if ["asm", "s", "z80", "ez80", "i8080", "8080", "i8086", "8086"]
                .iter()
                .any(|known| ext.eq_ignore_ascii_case(known)) =>
        {
            Ok(InputKind::Assembly)
        }
        Some(ext) => Err(format!(
            "cannot infer input kind from extension `.{ext}`; use an `.ezra` source file or an assembly extension such as `.asm`"
        )),
        None => Err(format!(
            "cannot infer input kind for `{}`; use an `.ezra` source file or an assembly extension such as `.asm`",
            source_path.display()
        )),
    }
}

fn build_ezra_source(
    source_path: &Path,
    source_location: SourceLocation,
    settings: &BuildSettings,
    options: &BuildCommandOptions,
) -> Result<BuildOutputs, String> {
    let source = fs::read_to_string(source_path)
        .map_err(|error| format!("failed to read {}: {error}", source_path.display()))?;
    let mut program = load_program_for_cli(source_path, settings).map_err(|error| {
        error
            .with_location_if_missing(source_location.clone())
            .to_string()
    })?;
    apply_asset_configuration(&mut program, settings);
    ensure_source_codegen_supported(settings)?;
    let assembly = emit_source_assembly(
        &program,
        configured_assembly_options(settings, &program, options.debug_comments)?,
    )
    .map_err(|error| command_diagnostic(error, source_path, &source, &source_location))?;

    write_build_artifacts(source_path, source_location, settings, &program, &assembly)
}

fn build_assembly_source(
    source_path: &Path,
    source_location: SourceLocation,
    settings: &BuildSettings,
) -> Result<BuildOutputs, String> {
    let assembly = fs::read_to_string(source_path)
        .map_err(|error| format!("failed to read {}: {error}", source_path.display()))?;
    write_assembly_build_artifacts(source_path, source_location, settings, &assembly)
}

fn executable_extension(settings: &BuildSettings) -> &'static str {
    if settings.target.triple.value.starts_with("gameboy-color-") {
        "gbc"
    } else {
        settings.output_format.extension()
    }
}

fn write_assembly_build_artifacts(
    source_path: &Path,
    _source_location: SourceLocation,
    settings: &BuildSettings,
    assembly: &str,
) -> Result<BuildOutputs, String> {
    let output_base = build_output_base_path(settings, source_path)?;
    let asm_path = output_base.with_extension("asm");
    let map_path = output_base.with_extension("map");
    let size_path = output_base.with_extension("size");
    let executable_path = output_base.with_extension(executable_extension(settings));
    let preprocessed = preprocess_assembly_file(
        source_path,
        AssemblyPreprocessOptions::for_compiled_features(
            &settings.target.triple.value,
            settings.assembler_cpu.as_str(),
        ),
    )
    .map_err(|error| error.to_string())?;
    let mut build_request = shared_build_request(settings, source_path)?;
    build_request.package_context.image_kind = ezra::package::PackageImageKind::LoadImage;
    let linked =
        ezra::api::link_assembly_program(source_path, &preprocessed.program, &build_request)
            .map_err(|error| error.to_string())?;

    if let Some(parent) = output_base.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
    }
    fs::write(&asm_path, assembly)
        .map_err(|error| format!("failed to write {}: {error}", asm_path.display()))?;
    fs::write(&map_path, linked.map)
        .map_err(|error| format!("failed to write {}: {error}", map_path.display()))?;
    fs::write(&size_path, linked.size_report.to_stable_string())
        .map_err(|error| format!("failed to write {}: {error}", size_path.display()))?;
    fs::write(&executable_path, linked.executable)
        .map_err(|error| format!("failed to write {}: {error}", executable_path.display()))?;

    Ok(BuildOutputs {
        asm: asm_path,
        map: map_path,
        size: size_path,
        executable: executable_path,
    })
}

fn write_build_artifacts(
    source_path: &Path,
    _source_location: SourceLocation,
    settings: &BuildSettings,
    _program: &Program,
    assembly: &str,
) -> Result<BuildOutputs, String> {
    let output_base = build_output_base_path(settings, source_path)?;
    let asm_path = output_base.with_extension("asm");
    let map_path = output_base.with_extension("map");
    let size_path = output_base.with_extension("size");
    let executable_path = output_base.with_extension(executable_extension(settings));

    let build_request = shared_build_request(settings, source_path)?;
    let linked =
        ezra::api::link_generated_assembly(source_path, assembly, _program, &build_request)
            .map_err(|error| {
                error
                    .with_location_if_missing(_source_location.clone())
                    .to_string()
            })?;
    if let Some(parent) = output_base.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
    }
    fs::write(&asm_path, assembly)
        .map_err(|error| format!("failed to write {}: {error}", asm_path.display()))?;
    fs::write(&map_path, linked.map)
        .map_err(|error| format!("failed to write {}: {error}", map_path.display()))?;
    fs::write(&size_path, linked.size_report.to_stable_string())
        .map_err(|error| format!("failed to write {}: {error}", size_path.display()))?;
    fs::write(&executable_path, linked.executable)
        .map_err(|error| format!("failed to write {}: {error}", executable_path.display()))?;

    Ok(BuildOutputs {
        asm: asm_path,
        map: map_path,
        size: size_path,
        executable: executable_path,
    })
}

fn build_output_base_path(settings: &BuildSettings, source_path: &Path) -> Result<PathBuf, String> {
    let source_stem = match settings.executable_name.as_deref() {
        Some(name) => name,
        None => source_path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .ok_or_else(|| format!("source path `{}` has no file stem", source_path.display()))?,
    };
    validate_artifact_basename(source_stem)?;
    let mut output_directory = settings.output_root.join(&settings.target.triple.value);
    if settings.executable_name.is_none()
        && let Some(relative) = source_relative_directory(settings, source_path)?
    {
        output_directory.push(relative);
    }
    Ok(output_directory.join(source_stem))
}

fn validate_artifact_basename(name: &str) -> Result<(), String> {
    if name.is_empty() || name == "." || name == ".." || name.contains('/') || name.contains('\\') {
        return Err(format!(
            "artifact executable name `{name}` must be a file basename, not a path"
        ));
    }
    Ok(())
}

fn source_relative_directory(
    settings: &BuildSettings,
    source_path: &Path,
) -> Result<Option<PathBuf>, String> {
    let Some(project_root) = settings.output_root.parent() else {
        return Ok(None);
    };
    let source_dir = source_path.parent().unwrap_or_else(|| Path::new("."));
    let project_root = absolute_path(project_root)?;
    let source_dir = absolute_path(source_dir)?;
    let Ok(relative) = source_dir.strip_prefix(&project_root) else {
        return Ok(None);
    };
    if relative.as_os_str().is_empty()
        || relative
            .components()
            .any(|component| component == std::path::Component::ParentDir)
    {
        return Ok(None);
    }
    Ok(Some(relative.to_path_buf()))
}

fn absolute_path(path: &Path) -> Result<PathBuf, String> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(env::current_dir()
            .map_err(|error| format!("failed to read current directory: {error}"))?
            .join(path))
    }
}

#[cfg(test)]
fn test_source(path: &str) -> Result<(), String> {
    test_source_with_command_options(&CommandOptions {
        path: PathBuf::from(path),
        debug_comments: false,
        default_sdk_symbols: true,
        layout_path: None,
        target: None,
        optimization_level: None,
        enable_optimizations: Vec::new(),
        disable_optimizations: Vec::new(),
    })
}

fn test_project_with_command_options(options: &TestCommandOptions) -> Result<(), String> {
    let project_path = env::current_dir()
        .map_err(|error| format!("failed to determine current directory: {error}"))?
        .join("Ezra.toml");
    let project = load_project_config(&project_path).map_err(|error| error.to_string())?;
    let tests_root = project.root.join("tests");
    let mut sources = Vec::new();
    discover_ezra_test_sources(&tests_root, &mut sources)?;
    sources.sort();
    if sources.is_empty() {
        return Err(format!(
            "no EZRA test sources found under `{}`",
            tests_root.display()
        ));
    }

    let mut failures = Vec::new();
    for source in &sources {
        let target = options
            .target
            .clone()
            .or_else(|| project.test_target.clone());
        let command = CommandOptions {
            path: source.clone(),
            debug_comments: options.debug_comments,
            default_sdk_symbols: options.default_sdk_symbols,
            layout_path: options.layout_path.clone(),
            target,
            optimization_level: None,
            enable_optimizations: Vec::new(),
            disable_optimizations: Vec::new(),
        };
        let build_options = BuildCommandOptions {
            path: Some(command.path.clone()),
            debug_comments: command.debug_comments,
            default_sdk_symbols: command.default_sdk_symbols,
            input_kind: Some(InputKind::Ezra),
            assembler_cpu: None,
            layout_path: command.layout_path.clone(),
            target: command.target.clone(),
            optimization_level: None,
            enable_optimizations: Vec::new(),
            disable_optimizations: Vec::new(),
        };
        let name = source
            .strip_prefix(&tests_root)
            .unwrap_or(source)
            .display()
            .to_string();
        match build(&build_options).and_then(|_| run_source_with_command_options(&command)) {
            Ok(run) if run.halted && run.result_code == 0 => {
                println!("ok: {name} ({} instructions)", run.instructions);
            }
            Ok(run) if !run.halted => {
                failures.push(format!("{name}: {}", format_test_run_failure(&run)))
            }
            Ok(run) => failures.push(format!("{name}: test failed with code {}", run.result_code)),
            Err(error) => failures.push(format!("{name}: {error}")),
        }
    }
    let passed = sources.len() - failures.len();
    println!("test result: {passed} passed; {} failed", failures.len());
    if failures.is_empty() {
        Ok(())
    } else {
        Err(format!("project test failures:\n{}", failures.join("\n")))
    }
}

fn discover_ezra_test_sources(root: &Path, sources: &mut Vec<PathBuf>) -> Result<(), String> {
    let mut visited = HashSet::new();
    discover_ezra_test_sources_inner(root, sources, &mut visited)
}

fn discover_ezra_test_sources_inner(
    root: &Path,
    sources: &mut Vec<PathBuf>,
    visited: &mut HashSet<PathBuf>,
) -> Result<(), String> {
    let canonical_root = match fs::canonicalize(root) {
        Ok(path) => path,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(format!(
                "failed to resolve test directory `{}`: {error}",
                root.display()
            ));
        }
    };
    if !visited.insert(canonical_root) {
        return Ok(());
    }
    let entries = fs::read_dir(root).map_err(|error| {
        format!(
            "failed to read test directory `{}`: {error}",
            root.display()
        )
    })?;
    for entry in entries {
        let entry =
            entry.map_err(|error| format!("failed to read test directory entry: {error}"))?;
        let file_type = entry
            .file_type()
            .map_err(|error| format!("failed to inspect test directory entry: {error}"))?;
        if file_type.is_symlink() {
            continue;
        }
        let path = entry.path();
        if file_type.is_dir() {
            discover_ezra_test_sources_inner(&path, sources, visited)?;
        } else if path
            .extension()
            .is_some_and(|extension| extension.to_string_lossy().eq_ignore_ascii_case("ezra"))
        {
            sources.push(path);
        }
    }
    Ok(())
}

fn test_source_with_command_options(options: &CommandOptions) -> Result<(), String> {
    let run = run_source_with_command_options(options)?;
    if !run.halted {
        return Err(format_test_run_failure(&run));
    }
    if run.result_code != 0 {
        return Err(format!("test failed with code {}", run.result_code));
    }
    println!("ok: test passed in {} instructions", run.instructions);
    Ok(())
}

fn run_source_with_command_options(options: &CommandOptions) -> Result<ezra::vm::TestRun, String> {
    let source_path = options.path.clone();
    let source_location = command_source_start_location(&source_path);
    let source = fs::read_to_string(&source_path)
        .map_err(|error| format!("failed to read {}: {error}", source_path.display()))?;
    let metadata = parse_test_metadata(&source)?;
    let settings = resolve_build_settings(options, &source_path)?;
    let mut program = load_program_for_cli(&source_path, &settings).map_err(|error| {
        error
            .with_location_if_missing(source_location.clone())
            .to_string()
    })?;
    apply_asset_configuration(&mut program, &settings);
    if let Err(errors) = settings.layout.validate() {
        let message = format_layout_errors(settings.layout_path.as_deref(), errors);
        return Err(format!("layout is invalid:\n{message}"));
    }
    validate_layout_for_target(&settings)?;
    ensure_source_codegen_supported(&settings)?;
    let assembly = emit_source_assembly(
        &program,
        configured_assembly_options(&settings, &program, options.debug_comments)?,
    )
    .map_err(|error| {
        error
            .with_location_if_missing(source_location.clone())
            .to_string()
    })?;
    validate_generated_assembly_for_command(&source_path, &source_location, &settings, &assembly)?;
    let run = ezra::vm::run_assembly_test_with_cpu_options_at(
        settings.target.triple.cpu,
        &assembly,
        &TestRunOptions {
            instruction_budget: 1_000_000,
            initial_ports: metadata.initial_ports,
            initial_memory: metadata.initial_memory,
            stack_top: settings.layout.stack.get(),
        },
        settings.layout.entry.get(),
    )
    .map_err(|error| error.to_string())?;
    Ok(run)
}

fn format_test_run_failure(run: &ezra::vm::TestRun) -> String {
    match run.failure {
        Some(ezra::vm::TestRunFailure::Timeout) | None => {
            format!("test timed out after {} instructions", run.instructions)
        }
        Some(ezra::vm::TestRunFailure::ExecutionOutsideMappedMemory { pc }) => format!(
            "test executed outside mapped memory at 0x{pc:06X} after {} instructions",
            run.instructions
        ),
        Some(ezra::vm::TestRunFailure::IllegalInstruction { pc }) => format!(
            "test hit an illegal instruction at 0x{pc:06X} after {} instructions",
            run.instructions
        ),
        Some(ezra::vm::TestRunFailure::StackOverflow { sp }) => format!(
            "test stack overflowed into non-stack memory at SP=0x{sp:06X} after {} instructions",
            run.instructions
        ),
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct TestMetadata {
    initial_ports: Vec<(u8, u8)>,
    initial_memory: Vec<(u32, u8)>,
}

fn parse_test_metadata(source: &str) -> Result<TestMetadata, String> {
    let mut initial_ports = Vec::new();
    let mut initial_memory = Vec::new();
    for (index, line) in source.lines().enumerate() {
        let Some(comment) = line.trim_start().strip_prefix("//") else {
            continue;
        };
        let comment = comment.trim_start();
        let rest = if let Some(rest) = comment.strip_prefix("test:") {
            rest.trim()
        } else if comment.starts_with("port") || comment.starts_with("mem") {
            comment
        } else {
            continue;
        };
        if let Some(rest) = rest.strip_prefix("port") {
            let (port, value) = rest
                .trim()
                .split_once('=')
                .ok_or_else(|| format!("invalid test port metadata on line {}", index + 1))?;
            let port = parse_metadata_u8(port.trim())
                .map_err(|error| format!("invalid test port on line {}: {error}", index + 1))?;
            let value = parse_metadata_u8(value.trim()).map_err(|error| {
                format!("invalid test port value on line {}: {error}", index + 1)
            })?;
            initial_ports.push((port, value));
        } else if let Some(rest) = rest.strip_prefix("mem") {
            let (address, value) = rest
                .trim()
                .split_once('=')
                .ok_or_else(|| format!("invalid test memory metadata on line {}", index + 1))?;
            let address = parse_metadata_u24(address.trim()).map_err(|error| {
                format!("invalid test memory address on line {}: {error}", index + 1)
            })?;
            let value = parse_metadata_u8(value.trim()).map_err(|error| {
                format!("invalid test memory value on line {}: {error}", index + 1)
            })?;
            initial_memory.push((address, value));
        } else {
            return Err(format!("invalid test metadata on line {}", index + 1));
        }
    }
    Ok(TestMetadata {
        initial_ports,
        initial_memory,
    })
}

fn parse_metadata_u8(text: &str) -> Result<u8, String> {
    let value = if let Some(hex) = text.strip_prefix("0x") {
        u16::from_str_radix(hex, 16)
    } else if let Some(bin) = text.strip_prefix("0b") {
        u16::from_str_radix(bin, 2)
    } else {
        text.parse::<u16>()
    }
    .map_err(|_| format!("invalid u8 literal `{text}`"))?;
    u8::try_from(value).map_err(|_| format!("value {text} is outside u8 range"))
}

fn parse_metadata_u24(text: &str) -> Result<u32, String> {
    let value = if let Some(hex) = text.strip_prefix("0x") {
        u32::from_str_radix(hex, 16)
    } else if let Some(bin) = text.strip_prefix("0b") {
        u32::from_str_radix(bin, 2)
    } else {
        text.parse::<u32>()
    }
    .map_err(|_| format!("invalid u24 literal `{text}`"))?;
    if value <= 0xFF_FFFF {
        Ok(value)
    } else {
        Err(format!("value {text} is outside u24 range"))
    }
}

fn emit_asm(options: &CommandOptions) -> Result<(), String> {
    let assembly = emit_assembly_with_command_options(options)?;
    print!("{assembly}");
    Ok(())
}

fn emit_ir(options: &EmitIrOptions) -> Result<(), String> {
    let source_path = options.command.path.clone();
    let source_location = command_source_start_location(&source_path);
    let settings = resolve_build_settings(&options.command, &source_path)?;
    let program = load_program_for_cli(&source_path, &settings).map_err(|error| {
        error
            .with_location_if_missing(source_location.clone())
            .to_string()
    })?;
    if let Err(errors) = settings.layout.validate() {
        let message = format_layout_errors(settings.layout_path.as_deref(), errors);
        return Err(format!("layout is invalid:\n{message}"));
    }
    let hir = HirProgram::from_ast(&program).map_err(|error| {
        error
            .with_location_if_missing(source_location.clone())
            .to_string()
    })?;
    match options.stage {
        IrStage::Hir => print!("{}", hir.dump_text()),
        IrStage::Tbir => {
            validate_layout_for_target(&settings)?;
            ensure_source_codegen_supported(&settings)?;
            let tbir = TbirProgram::lower(
                &hir,
                &program,
                &configured_assembly_options(&settings, &program, options.command.debug_comments)?,
            )
            .map_err(|error| error.with_location_if_missing(source_location).to_string())?;
            print!("{}", tbir.dump_text());
        }
    }
    Ok(())
}

fn emit_assembly_with_command_options(options: &CommandOptions) -> Result<String, String> {
    let source_path = options.path.clone();
    let source = fs::read_to_string(&source_path)
        .map_err(|error| format!("failed to read {}: {error}", source_path.display()))?;
    let source_location = command_source_start_location(&source_path);
    let settings = resolve_build_settings(options, &source_path)?;
    let mut program = load_program_for_cli(&source_path, &settings).map_err(|error| {
        error
            .with_location_if_missing(source_location.clone())
            .to_string()
    })?;
    apply_asset_configuration(&mut program, &settings);
    if let Err(errors) = settings.layout.validate() {
        let message = format_layout_errors(settings.layout_path.as_deref(), errors);
        return Err(format!("layout is invalid:\n{message}"));
    }
    validate_layout_for_target(&settings)?;
    ensure_source_codegen_supported(&settings)?;
    let assembly = emit_source_assembly(
        &program,
        configured_assembly_options(&settings, &program, options.debug_comments)?,
    )
    .map_err(|error| command_diagnostic(error, &source_path, &source, &source_location))?;
    validate_generated_assembly_for_command(&source_path, &source_location, &settings, &assembly)?;
    Ok(assembly)
}

fn command_diagnostic(
    error: Diagnostic,
    source_path: &Path,
    source: &str,
    fallback: &SourceLocation,
) -> String {
    let error = if let Some(span) = diagnostic_span(source_path, source, &error.message) {
        error.with_span_if_missing(span)
    } else {
        error
    };
    error.with_location_if_missing(fallback.clone()).to_string()
}

fn validate_generated_assembly_for_command(
    source_path: &Path,
    source_location: &SourceLocation,
    settings: &BuildSettings,
    assembly: &str,
) -> Result<(), String> {
    let build_request = shared_build_request(settings, source_path)?;
    ezra::api::validate_generated_assembly_for_request(source_path, assembly, &build_request)
        .map_err(|error| {
            error
                .with_location_if_missing(source_location.clone())
                .to_string()
        })
}

fn check(options: &CommandOptions) -> Result<(), String> {
    let source_path = options.path.clone();
    let source = fs::read_to_string(&source_path)
        .map_err(|error| format!("failed to read {}: {error}", source_path.display()))?;
    check_source_with_layout(options, &source_path, &source)
}

fn check_source_with_layout(
    options: &CommandOptions,
    source_path: &std::path::Path,
    source: &str,
) -> Result<(), String> {
    let source_location = command_source_start_location(source_path);
    let root = parse_program(source_path, source).map_err(|error| error.to_string())?;
    let imports = root
        .declarations
        .iter()
        .filter(|decl| matches!(decl, ezra::ast::Declaration::Import(_)))
        .count();
    let settings = resolve_build_settings(options, source_path)?;
    let mut program = load_program_for_cli(source_path, &settings).map_err(|error| {
        error
            .with_location_if_missing(source_location.clone())
            .to_string()
    })?;
    apply_asset_configuration(&mut program, &settings);
    if let Err(errors) = settings.layout.validate() {
        let message = format_layout_errors(settings.layout_path.as_deref(), errors);
        return Err(format!("layout is invalid:\n{message}"));
    }
    validate_layout_for_target(&settings)?;
    ensure_source_codegen_supported(&settings)?;
    let assembly = emit_source_assembly(
        &program,
        configured_assembly_options(&settings, &program, options.debug_comments)?,
    )
    .map_err(|error| command_diagnostic(error, source_path, source, &source_location))?;
    validate_generated_assembly_for_command(source_path, &source_location, &settings, &assembly)?;

    println!(
        "ok: {} imports, {} declarations, main present",
        imports,
        program.declarations.len()
    );
    Ok(())
}

fn create_disk(options: &DiskCommandOptions) -> Result<(), String> {
    struct LoadedDiskFile {
        name: String,
        bytes: Vec<u8>,
    }

    let mut loaded = Vec::with_capacity(options.files.len());
    for file in &options.files {
        let bytes = fs::read(&file.path)
            .map_err(|error| format!("failed to read {}: {error}", file.path.display()))?;
        loaded.push(LoadedDiskFile {
            name: file.name.clone(),
            bytes,
        });
    }
    let files = loaded
        .iter()
        .map(|file| DiskFile::new(&file.name, &file.bytes))
        .collect::<Vec<_>>();
    let image = create_disk_image(&DiskRequest::new(options.format, &options.label, &files))
        .map_err(|error| error.to_string())?;

    if let Some(parent) = options
        .output
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
    }
    fs::write(&options.output, image)
        .map_err(|error| format!("failed to write {}: {error}", options.output.display()))?;
    println!("wrote {}", options.output.display());
    Ok(())
}

fn print_layout(layout_path: Option<&Path>) -> Result<(), String> {
    let layout_path = layout_path.map(Path::to_path_buf);
    let layout = load_layout(layout_path.as_deref(), ezra::target::DEFAULT_TARGET_TRIPLE)?;
    if let Err(errors) = layout.validate() {
        eprintln!(
            "error: {}",
            format_layout_errors(layout_path.as_deref(), errors)
        );
        return Err("layout is invalid".to_owned());
    }

    println!("layout {}", layout.name);
    println!("load  {}", layout.load);
    println!("entry {}", layout.entry);
    println!("stack {}", layout.stack);
    println!();
    print!("{}", layout.map_summary());
    Ok(())
}

fn load_layout(path: Option<&Path>, target: &str) -> Result<Layout, String> {
    let Some(path) = path else {
        return Ok(default_layout_for_target(target));
    };
    let source = fs::read_to_string(path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    parse_layout(&source).map_err(|error| {
        error
            .with_location_if_missing(command_source_start_location(path))
            .to_string()
    })
}

fn default_layout_for_target(target: &str) -> Layout {
    let layout = ezra::layout::default_layout_for_target(target);
    if parse_target_triple(target).is_ok_and(|triple| triple.cpu == CpuFamily::I8086)
        && layout_requires_more_than_16_bits(&layout)
    {
        Layout::bare_16(CpuFamily::I8086.as_str())
    } else {
        layout
    }
}

fn layout_requires_more_than_16_bits(layout: &Layout) -> bool {
    layout.load.get() > 0xFFFF
        || layout.entry.get() > 0xFFFF
        || layout.stack.get() > 0xFFFF
        || layout
            .regions
            .iter()
            .any(|region| region.start.get() > 0xFFFF || region.end.get() > 0xFFFF)
        || layout
            .symbols
            .iter()
            .any(|symbol| symbol.value.get() > 0xFFFF)
}

fn init_project(options: &InitOptions) -> Result<(), String> {
    let root = &options.path;
    let project_name = options
        .name
        .clone()
        .unwrap_or_else(|| default_project_name(root));
    validate_project_name(&project_name)?;
    let target = resolve_target_profile(Some(&options.target))?;
    let gitignore_path = root.join(".gitignore");
    let project_path = root.join("Ezra.toml");
    let readme_path = root.join("README.md");
    let assets_gitkeep_path = root.join("assets/.gitkeep");
    let source_path = root.join("src/main.ezra");
    let paths = [
        gitignore_path.clone(),
        project_path.clone(),
        readme_path.clone(),
        assets_gitkeep_path.clone(),
        source_path.clone(),
    ];
    preflight_init(root, &paths, options.force)?;

    fs::create_dir_all(root)
        .map_err(|error| format!("failed to create {}: {error}", root.display()))?;
    write_scaffold_file(
        &gitignore_path,
        options.force,
        "target/\n*.bin\n*.com\n*.gaem\n*.hex\n*.elf\n*.tap\n*.gb\n*.prg\n*.8xp\n*.8ek\n*.8xk\n*.map\n*.asm\n",
    )?;
    write_scaffold_file(
        &project_path,
        options.force,
        &format!(
            "[project]\nname = \"{project_name}\"\n\n[build]\ninput = \"src/main.ezra\"\ntarget = \"{}\"\noutput = \"{}\"\nexecutable = \"{project_name}\"\n",
            options.target,
            target.output_format.extension()
        ),
    )?;
    write_scaffold_file(
        &readme_path,
        options.force,
        &format!(
            "# {project_name}\n\nBuild with:\n\n```sh\nezrac build\n```\n\nOr from an ezrac checkout:\n\n```sh\ncargo run -- build\n```\n"
        ),
    )?;
    fs::create_dir_all(root.join("src"))
        .map_err(|error| format!("failed to create {}/src: {error}", root.display()))?;
    fs::create_dir_all(root.join("assets"))
        .map_err(|error| format!("failed to create {}/assets: {error}", root.display()))?;
    write_scaffold_file(&assets_gitkeep_path, options.force, "")?;
    write_scaffold_file(
        &source_path,
        options.force,
        &initial_main_source(&options.target),
    )?;
    println!("initialized {}", root.display());
    Ok(())
}

fn preflight_init(root: &Path, paths: &[PathBuf], force: bool) -> Result<(), String> {
    if root.exists() && !root.is_dir() {
        return Err(format!(
            "cannot initialize project: {} is not a directory",
            root.display()
        ));
    }
    for directory in [root.to_path_buf(), root.join("src"), root.join("assets")] {
        if directory.exists() && !directory.is_dir() {
            return Err(format!(
                "cannot initialize project: {} is not a directory",
                directory.display()
            ));
        }
    }
    for path in paths {
        if path.exists() && !force {
            return Err(format!(
                "refusing to overwrite {}; pass --force to replace existing scaffold files",
                path.display()
            ));
        }
    }
    Ok(())
}

fn default_project_name(root: &Path) -> String {
    root.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty() && *name != ".")
        .unwrap_or("ezra-game")
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
                ch
            } else {
                '-'
            }
        })
        .collect()
}

fn validate_project_name(name: &str) -> Result<(), String> {
    if name.is_empty()
        || !name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
    {
        return Err(
            "project name must contain only ASCII letters, digits, underscores, or hyphens"
                .to_owned(),
        );
    }
    Ok(())
}

fn initial_main_source(target: &str) -> String {
    if target.starts_with("agonlight-mos-ez80") {
        return "import agon.console\n\nfn main() {\n    console.print_line(\"Hello from EZRA\")\n}\n".to_owned();
    }
    if target.split('-').any(|part| part == "cpm") {
        return "import cpm.console\n\nfn main() {\n    console.write('H')\n    console.write('i')\n    console.newline()\n    console.exit()\n}\n".to_owned();
    }
    if target.starts_with("zxspectrum-z80") {
        return "import zx.rom\n\nfn main() {\n    zx.rom.print_char('H')\n    zx.rom.print_char('i')\n}\n".to_owned();
    }
    if target.starts_with("ti84plusce-ez80") || target.starts_with("ti83premiumce-ez80") {
        return "import tice.lcd\n\nfn main() {\n    tice.lcd.set_first_pixel(0xFF)\n}\n"
            .to_owned();
    }
    if target.starts_with("ti83-z80")
        || target.starts_with("ti83plus-z80")
        || target.starts_with("ti84-z80")
        || target.starts_with("ti84plus-z80")
    {
        return "import ti.lcd\n\nfn main() {\n    ti.lcd.set_first_byte(0xFF)\n}\n".to_owned();
    }
    "fn main() {\n    return\n}\n".to_owned()
}

fn write_scaffold_file(path: &Path, force: bool, contents: &str) -> Result<(), String> {
    if path.exists() && !force {
        return Err(format!(
            "refusing to overwrite {}; pass --force to replace existing scaffold files",
            path.display()
        ));
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
    }
    fs::write(path, contents)
        .map_err(|error| format!("failed to write {}: {error}", path.display()))
}

fn install_syntax(options: &InstallSyntaxOptions) -> Result<(), String> {
    let mut failures = Vec::new();
    for editor in &options.editors {
        match install_syntax_for_editor(*editor, options.dry_run) {
            Ok(paths) => {
                for path in paths {
                    if options.dry_run {
                        println!("would write {}", path.display());
                    } else {
                        println!("installed {} syntax at {}", editor.name(), path.display());
                    }
                }
            }
            Err(error) => failures.push(format!("{}: {error}", editor.name())),
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "syntax installation completed with errors:\n{}",
            failures.join("\n")
        ))
    }
}

fn install_syntax_for_editor(editor: SyntaxEditor, dry_run: bool) -> Result<Vec<PathBuf>, String> {
    match editor {
        SyntaxEditor::Vim => install_vim_syntax(config_home()?.join(".vim"), dry_run),
        SyntaxEditor::Neovim => install_vim_syntax(config_home()?.join(".config/nvim"), dry_run),
        SyntaxEditor::Nano => install_nano_syntax(dry_run),
        SyntaxEditor::Micro => install_single_syntax_file(
            micro_config_home()?.join("syntax/ezra.yaml"),
            include_str!("../assets/editors/micro/ezra.yaml"),
            dry_run,
        ),
        SyntaxEditor::Helix => install_helix_syntax(dry_run),
        SyntaxEditor::Vscode => install_vscode_syntax(dry_run),
        SyntaxEditor::Zed => install_zed_syntax(dry_run),
        SyntaxEditor::NotepadPlusPlus => install_notepadpp_syntax(dry_run),
    }
}

fn environment_path(name: &str) -> Option<PathBuf> {
    env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn config_home() -> Result<PathBuf, String> {
    if let Some(path) = environment_path("HOME") {
        return Ok(path);
    }
    #[cfg(windows)]
    if let Some(path) = environment_path("USERPROFILE") {
        return Ok(path);
    }
    Err("user home directory is not set".to_owned())
}

fn micro_config_home() -> Result<PathBuf, String> {
    resolve_micro_config_home(
        environment_path("MICRO_CONFIG_HOME"),
        environment_path("XDG_CONFIG_HOME"),
        config_home(),
    )
}

fn resolve_micro_config_home(
    micro_home: Option<PathBuf>,
    xdg_home: Option<PathBuf>,
    home: Result<PathBuf, String>,
) -> Result<PathBuf, String> {
    if let Some(path) = micro_home {
        return Ok(path);
    }
    if let Some(path) = xdg_home {
        return Ok(path.join("micro"));
    }
    Ok(home?.join(".config/micro"))
}

fn appdata_home() -> Result<PathBuf, String> {
    if let Some(path) = env::var_os("APPDATA") {
        return Ok(PathBuf::from(path));
    }
    Ok(config_home()?.join(".config"))
}

fn zed_data_home() -> Result<PathBuf, String> {
    if let Some(path) = environment_path("ZED_DATA_DIR") {
        return Ok(path);
    }
    #[cfg(windows)]
    if let Some(path) = environment_path("LOCALAPPDATA") {
        return Ok(path.join("Zed"));
    }
    #[cfg(target_os = "macos")]
    return Ok(config_home()?.join("Library/Application Support/Zed"));
    if let Some(path) = environment_path("XDG_DATA_HOME") {
        return Ok(path.join("zed"));
    }
    Ok(config_home()?.join(".local/share/zed"))
}

fn install_vim_syntax(root: PathBuf, dry_run: bool) -> Result<Vec<PathBuf>, String> {
    let files = [
        (
            "ftdetect/ezra.vim",
            include_str!("../assets/editors/vim/ftdetect/ezra.vim"),
        ),
        (
            "ftplugin/ezra.vim",
            include_str!("../assets/editors/vim/ftplugin/ezra.vim"),
        ),
        (
            "syntax/ezra.vim",
            include_str!("../assets/editors/vim/syntax/ezra.vim"),
        ),
    ];
    write_syntax_files(root, &files, dry_run)
}

fn install_nano_syntax(dry_run: bool) -> Result<Vec<PathBuf>, String> {
    let root = config_home()?;
    let nanorc_dir = root.join(".nano");
    let syntax_path = nanorc_dir.join("ezra.nanorc");
    let mut paths = install_single_syntax_file(
        syntax_path.clone(),
        include_str!("../assets/editors/nano/ezra.nanorc"),
        dry_run,
    )?;
    let include_line = format!("include {}", syntax_path.display());
    let nanorc = root.join(".nanorc");
    if dry_run {
        paths.push(nanorc);
        return Ok(paths);
    }
    let existing = fs::read_to_string(&nanorc).unwrap_or_default();
    if !existing.lines().any(|line| line.trim() == include_line) {
        let mut next = existing;
        if !next.is_empty() && !next.ends_with('\n') {
            next.push('\n');
        }
        next.push_str(&include_line);
        next.push('\n');
        fs::write(&nanorc, next)
            .map_err(|error| format!("failed to write {}: {error}", nanorc.display()))?;
    }
    paths.push(nanorc);
    Ok(paths)
}

fn install_helix_syntax(dry_run: bool) -> Result<Vec<PathBuf>, String> {
    let root = if cfg!(windows) {
        appdata_home()?.join("helix")
    } else {
        config_home()?.join(".config/helix")
    };
    let files = [
        (
            "languages.toml",
            include_str!("../assets/editors/helix/languages.toml"),
        ),
        (
            "runtime/queries/ezra/highlights.scm",
            include_str!("../assets/editors/helix/queries/highlights.scm"),
        ),
    ];
    let mut paths = write_syntax_files(root.clone(), &files, dry_run)?;
    let assembly_files = [(
        "runtime/queries/ezra-asm/highlights.scm",
        include_str!("../assets/editors/helix/queries/ezra-asm/highlights.scm"),
    )];
    paths.extend(write_syntax_files(root, &assembly_files, dry_run)?);
    Ok(paths)
}

fn install_vscode_syntax(dry_run: bool) -> Result<Vec<PathBuf>, String> {
    let root = config_home()?.join(".vscode/extensions/ezra-language");
    let files = [
        (
            "package.json",
            include_str!("../assets/editors/vscode/package.json"),
        ),
        (
            "language-configuration.json",
            include_str!("../assets/editors/vscode/language-configuration.json"),
        ),
        (
            "syntaxes/ezra.tmLanguage.json",
            include_str!("../assets/editors/vscode/syntaxes/ezra.tmLanguage.json"),
        ),
    ];
    write_syntax_files(root, &files, dry_run)
}

fn install_zed_syntax(dry_run: bool) -> Result<Vec<PathBuf>, String> {
    let root = zed_data_home()?.join("extensions/installed/ezra");
    let files = [
        (
            "extension.toml",
            include_str!("editor_assets/zed/extension.toml"),
        ),
        (
            "languages/ezra/config.toml",
            include_str!("editor_assets/zed/languages/ezra/config.toml"),
        ),
        (
            "languages/ezra/highlights.scm",
            include_str!("editor_assets/zed/languages/ezra/highlights.scm"),
        ),
        (
            "languages/ezra/brackets.scm",
            include_str!("editor_assets/zed/languages/ezra/brackets.scm"),
        ),
        (
            "languages/ezra/indents.scm",
            include_str!("editor_assets/zed/languages/ezra/indents.scm"),
        ),
        (
            "languages/ezra/outline.scm",
            include_str!("editor_assets/zed/languages/ezra/outline.scm"),
        ),
        (
            "languages/ezra/textobjects.scm",
            include_str!("editor_assets/zed/languages/ezra/textobjects.scm"),
        ),
        (
            "languages/ezra-asm/config.toml",
            include_str!("editor_assets/zed/languages/ezra-asm/config.toml"),
        ),
        (
            "languages/ezra-asm/highlights.scm",
            include_str!("editor_assets/zed/languages/ezra-asm/highlights.scm"),
        ),
    ];
    let mut paths = write_syntax_files(root.clone(), &files, dry_run)?;
    let binary_files = [
        (
            "extension.wasm",
            include_bytes!("editor_assets/zed/extension.wasm").as_slice(),
        ),
        (
            "grammars/ezra.wasm",
            include_bytes!("editor_assets/zed/grammars/ezra.wasm").as_slice(),
        ),
        (
            "grammars/ezra_asm.wasm",
            include_bytes!("editor_assets/zed/grammars/ezra_asm.wasm").as_slice(),
        ),
    ];
    paths.extend(write_syntax_binary_files(root, &binary_files, dry_run)?);
    Ok(paths)
}

fn install_notepadpp_syntax(dry_run: bool) -> Result<Vec<PathBuf>, String> {
    install_single_syntax_file(
        appdata_home()?.join("Notepad++/userDefineLangs/ezra.xml"),
        include_str!("../assets/editors/notepad++/ezra.xml"),
        dry_run,
    )
}

fn install_single_syntax_file(
    path: PathBuf,
    contents: &str,
    dry_run: bool,
) -> Result<Vec<PathBuf>, String> {
    if !dry_run {
        write_syntax_file(&path, contents)?;
    }
    Ok(vec![path])
}

fn write_syntax_files(
    root: PathBuf,
    files: &[(&str, &str)],
    dry_run: bool,
) -> Result<Vec<PathBuf>, String> {
    let mut paths = Vec::new();
    for (relative, contents) in files {
        let path = root.join(relative);
        if !dry_run {
            write_syntax_file(&path, contents)?;
        }
        paths.push(path);
    }
    Ok(paths)
}

fn write_syntax_binary_files(
    root: PathBuf,
    files: &[(&str, &[u8])],
    dry_run: bool,
) -> Result<Vec<PathBuf>, String> {
    let mut paths = Vec::new();
    for (relative, contents) in files {
        let path = root.join(relative);
        if !dry_run {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)
                    .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
            }
            fs::write(&path, contents)
                .map_err(|error| format!("failed to write {}: {error}", path.display()))?;
        }
        paths.push(path);
    }
    Ok(paths)
}

fn write_syntax_file(path: &Path, contents: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
    }
    fs::write(path, contents)
        .map_err(|error| format!("failed to write {}: {error}", path.display()))
}

fn format_layout_errors(path: Option<&Path>, errors: Vec<ezra::diagnostic::Diagnostic>) -> String {
    let location = path.map(command_source_start_location);
    errors
        .into_iter()
        .map(|error| {
            if let Some(location) = location.clone() {
                error.with_location_if_missing(location).to_string()
            } else {
                error.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn command_source_start_location(path: &std::path::Path) -> SourceLocation {
    SourceLocation {
        file: path.to_path_buf(),
        line: 1,
        column: 1,
    }
}

fn print_header() -> Result<(), String> {
    let header = CartridgeHeader::default();
    let bytes = header.serialize();

    for (index, byte) in bytes.iter().enumerate() {
        if index % 16 == 0 {
            print!("{index:04X}:");
        }
        print!(" {byte:02X}");
        if index % 16 == 15 {
            println!();
        }
    }

    Ok(())
}

fn print_usage() {
    println!("{}", usage());
}

fn print_targets() {
    struct TargetRow {
        triple: &'static str,
        cpu: &'static str,
        address_width_bits: u16,
        output: &'static str,
        sdk: &'static str,
        status: &'static str,
    }

    const TARGETS: &[TargetRow] = &[
        TargetRow {
            triple: "agonlight-mos-ez80",
            cpu: "ez80",
            address_width_bits: 24,
            output: "gaem",
            sdk: "agon.*",
            status: "main source target",
        },
        TargetRow {
            triple: "custom-unknown-ez80",
            cpu: "ez80",
            address_width_bits: 24,
            output: "bin",
            sdk: "none",
            status: "generic eZ80 source target",
        },
        TargetRow {
            triple: "ez180n-ez80",
            cpu: "ez80",
            address_width_bits: 24,
            output: "bin",
            sdk: "ez180n.*",
            status: "ez180N libretro console target",
        },
        TargetRow {
            triple: "ezra-test-flat-ez80",
            cpu: "ez80",
            address_width_bits: 24,
            output: "bin",
            sdk: "harness.*",
            status: "test harness target",
        },
        TargetRow {
            triple: "ezra-test-split-ez80",
            cpu: "ez80",
            address_width_bits: 24,
            output: "bin",
            sdk: "harness.*",
            status: "test harness target",
        },
        TargetRow {
            triple: "ti84plusce-ez80",
            cpu: "ez80",
            address_width_bits: 24,
            output: "8xp",
            sdk: "tice.*",
            status: "experimental TI CE target",
        },
        TargetRow {
            triple: "ti83premiumce-ez80",
            cpu: "ez80",
            address_width_bits: 24,
            output: "8xp",
            sdk: "tice.*",
            status: "experimental TI CE target",
        },
        TargetRow {
            triple: "zxspectrum-z80",
            cpu: "z80",
            address_width_bits: 16,
            output: "bin",
            sdk: "zx.*",
            status: "experimental Z80 target",
        },
        TargetRow {
            triple: "gameboy-dmg-lr35902",
            cpu: "lr35902",
            address_width_bits: 16,
            output: "gb",
            sdk: "vendored asm/gb",
            status: "EZRA source and assembly DMG target",
        },
        TargetRow {
            triple: "gameboy-color-lr35902",
            cpu: "lr35902",
            address_width_bits: 16,
            output: "gb",
            sdk: "vendored asm/gb",
            status: "EZRA source and assembly CGB target",
        },
        TargetRow {
            triple: "ti83-z80",
            cpu: "z80",
            address_width_bits: 16,
            output: "8xp",
            sdk: "ti.*",
            status: "experimental TI Z80 target",
        },
        TargetRow {
            triple: "ti83plus-z80",
            cpu: "z80",
            address_width_bits: 16,
            output: "8xp",
            sdk: "ti.*",
            status: "experimental TI Z80 target",
        },
        TargetRow {
            triple: "ti84-z80",
            cpu: "z80",
            address_width_bits: 16,
            output: "8xp",
            sdk: "ti.*",
            status: "experimental TI Z80 target",
        },
        TargetRow {
            triple: "ti84plus-z80",
            cpu: "z80",
            address_width_bits: 16,
            output: "8xp",
            sdk: "ti.*",
            status: "experimental TI Z80 target",
        },
        TargetRow {
            triple: "cpm-*-z80",
            cpu: "z80",
            address_width_bits: 16,
            output: "com",
            sdk: "cpm.*",
            status: "assembly examples; source backend maturing",
        },
        TargetRow {
            triple: "cpm-*-i8080",
            cpu: "i8080",
            address_width_bits: 16,
            output: "com",
            sdk: "cpm.*",
            status: "assembly/source scaffold",
        },
        TargetRow {
            triple: "cpm-*-i8085",
            cpu: "i8085",
            address_width_bits: 16,
            output: "com",
            sdk: "cpm.*",
            status: "assembly/source scaffold",
        },
        TargetRow {
            triple: "bare-z80",
            cpu: "z80",
            address_width_bits: 16,
            output: "bin",
            sdk: "none",
            status: "bare assembly/source scaffold",
        },
        TargetRow {
            triple: "bare-r800",
            cpu: "r800",
            address_width_bits: 16,
            output: "bin",
            sdk: "none",
            status: "bare R800 assembly/source target with VM execution",
        },
        TargetRow {
            triple: "bare-z80n",
            cpu: "z80n",
            address_width_bits: 16,
            output: "bin",
            sdk: "none",
            status: "bare assembly/source scaffold",
        },
        TargetRow {
            triple: "bare-z180",
            cpu: "z180",
            address_width_bits: 16,
            output: "bin",
            sdk: "none",
            status: "bare assembly/source scaffold",
        },
        TargetRow {
            triple: "bare-i8080",
            cpu: "i8080",
            address_width_bits: 16,
            output: "bin",
            sdk: "none",
            status: "bare assembly/source scaffold",
        },
        TargetRow {
            triple: "bare-i8085",
            cpu: "i8085",
            address_width_bits: 16,
            output: "bin",
            sdk: "none",
            status: "bare assembly/source scaffold",
        },
        #[cfg(feature = "i8086")]
        TargetRow {
            triple: "msdos-com-i8086",
            cpu: "i8086",
            address_width_bits: 16,
            output: "com",
            sdk: "dos.*",
            status: "MS-DOS .COM source/assembly target",
        },
        #[cfg(feature = "i8086")]
        TargetRow {
            triple: "bare-i8086",
            cpu: "i8086",
            address_width_bits: 16,
            output: "bin",
            sdk: "none",
            status: "generic source/assembly i8086 target",
        },
        TargetRow {
            triple: "bare-ez80",
            cpu: "ez80",
            address_width_bits: 24,
            output: "bin",
            sdk: "none",
            status: "bare eZ80 target",
        },
        TargetRow {
            triple: "nes-2a03",
            cpu: "2a03",
            address_width_bits: 16,
            output: "nes",
            sdk: "nes.*",
            status: "EZRA source and raw assembly NROM-128 target",
        },
        TargetRow {
            triple: "sega-master-system-z80",
            cpu: "z80",
            address_width_bits: 16,
            output: "sms",
            sdk: "sms.*",
            status: "fixed 32 KiB export-SMS source target",
        },
        TargetRow {
            triple: "sega-game-gear-z80",
            cpu: "z80",
            address_width_bits: 16,
            output: "gg",
            sdk: "sms.* + gg.*",
            status: "fixed 32 KiB export-Game-Gear source target",
        },
        #[cfg(feature = "tms9900")]
        TargetRow {
            triple: "ti99-4a-tms9900",
            cpu: "tms9900",
            address_width_bits: 16,
            output: "bin",
            sdk: "ti99.*",
            status: "TI-99/4A cartridge source target",
        },
        #[cfg(feature = "tms9900")]
        TargetRow {
            triple: "bare-tms9900",
            cpu: "tms9900",
            address_width_bits: 16,
            output: "bin",
            sdk: "none",
            status: "bare TMS9900 source/assembly target",
        },
        #[cfg(feature = "msp430")]
        TargetRow {
            triple: "msp430-none-elf",
            cpu: "msp430",
            address_width_bits: 16,
            output: "elf",
            sdk: "none",
            status: "MSP430 ELF32 source/assembly target",
        },
        #[cfg(feature = "msp430")]
        TargetRow {
            triple: "msp430x-none-elf",
            cpu: "msp430x",
            address_width_bits: 20,
            output: "elf",
            sdk: "none",
            status: "MSP430X ELF32 source/assembly target",
        },
        #[cfg(feature = "pic18")]
        TargetRow {
            triple: "generic-pic18-bare",
            cpu: "pic18",
            address_width_bits: 21,
            output: "hex",
            sdk: "none",
            status: "classic PIC18 source/assembly target",
        },
        #[cfg(feature = "dcpu")]
        TargetRow {
            triple: "generic-dcpu-bare",
            cpu: "dcpu",
            address_width_bits: 16,
            output: "bin",
            sdk: "dcpu.*",
            status: "DCPU-16 assembly, SDK, and limited scalar source target",
        },
        #[cfg(feature = "avr")]
        TargetRow {
            triple: "bare-avr",
            cpu: "avr",
            address_width_bits: 16,
            output: "bin",
            sdk: "none",
            status: "register-ABI AVR source/assembly target",
        },
        #[cfg(feature = "avr")]
        TargetRow {
            triple: "arduboy-avr",
            cpu: "avr",
            address_width_bits: 16,
            output: "hex",
            sdk: "arduboy.*",
            status: "ATmega32U4 register-ABI source/assembly target",
        },
        #[cfg(feature = "m68k")]
        TargetRow {
            triple: "generic-m68k-bare",
            cpu: "m68k",
            address_width_bits: 24,
            output: "bin",
            sdk: "none",
            status: "Motorola 68000 source/assembly target",
        },
    ];

    println!("supported target triples:\n");
    println!(
        "{:<24} {:<6} {:>5} {:<7} {:<10} status",
        "triple", "cpu", "addr", "output", "sdk"
    );
    for target in TARGETS {
        println!(
            "{:<24} {:<6} {:>4}b {:<7} {:<10} {}",
            target.triple,
            target.cpu,
            target.address_width_bits,
            target.output,
            target.sdk,
            target.status
        );
    }
    println!(
        "\nPatterns with `*` accept concrete versions, such as `cpm-2.2-z80`. Other triples may resolve if they contain a supported CPU family, but only listed triples have documented layouts/SDKs."
    );
}

fn usage() -> String {
    "usage: ezra <command>\n\ncommands:\n  init [--name <name>] [--target <triple>] [--force] [dir]\n                                       create a new EZRA project scaffold\n  install-syntax (--all | [--editor] <editor>...) [--dry-run]\n                                       install editor syntax files for selected editors\n  targets                              list documented target triples, outputs, and SDKs\n  lsp                                  start the language server; requires Cargo feature `lsp`\n  check [--target <triple>] [--debug-comments] [--no-default-sdk-symbols] [--layout <file.ezralayout>] <file.ezra>\n                                       parse and validate a source file\n  build [--target <triple>] [--cpu <mode>] [--input-kind ezra|assembly] [--size-budget NAME=BYTES]... [--debug-comments] [--no-default-sdk-symbols] [--layout <file.ezralayout>] [file.ezra|file.asm]\n                                       write .asm, .map, .size, and target executable artifacts\n  disk [--format <format>] [--label <label>] --output <image> [--file [NAME=]PATH]...
                                       create an emulator-ready disk image with named files
  emit-asm [--target <triple>] [--debug-comments] [--no-default-sdk-symbols] [--layout <file.ezralayout>] <file.ezra>\n                                       emit readable target assembly\n  emit-ir [--stage hir|tbir] [--target <triple>] [--debug-comments] [--no-default-sdk-symbols] [--layout <file.ezralayout>] <file.ezra>\n                                       emit inspectable HIR or TBIR text\n  test [--target <triple>] [--debug-comments] [--no-default-sdk-symbols] [--layout <file.ezralayout>] <file.ezra>\n                                       emit and run on the target VM\n  assemble [--target <triple>] [--cpu <mode>] [--layout <file.ezralayout>] [--map <file.map>] [--base <addr>] [--output <file.bin>] <file.asm>\n                                       assemble target assembly into a raw binary\n  layout [file.ezralayout]             print the default or custom EZRA layout summary\n  header                               print the default 64-byte cartridge header\n\nsource optimization options:\n  -O0|-O1|-O2|-O3                    select an optimization level (default: -O2)\n  --enable-optimization <pass>        enable one named optimization pass\n  --disable-optimization <pass>       disable one named optimization pass\n\neditors for install-syntax: vim, neovim, nano, micro, helix, vscode, zed, notepad++".to_owned()
}

#[cfg(all(test, feature = "i8086"))]
mod i8086_review_tests {
    use super::*;

    fn temp_source(name: &str, source: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "ezrac_i8086_review_{name}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("main.ezra");
        std::fs::write(&path, source).unwrap();
        path
    }

    #[test]
    fn cli_builds_msdos_target_as_a_raw_com_from_0100h() {
        let source_path = temp_source("msdos_com", "fn main() {}");
        let outputs = build_source_with_build_options(&BuildCommandOptions {
            path: Some(source_path.clone()),
            debug_comments: false,
            default_sdk_symbols: true,
            input_kind: None,
            assembler_cpu: None,
            layout_path: None,
            target: Some("msdos-com-i8086".to_owned()),
            optimization_level: None,
            enable_optimizations: Vec::new(),
            disable_optimizations: Vec::new(),
        })
        .unwrap();
        let assembly = std::fs::read_to_string(&outputs.asm).unwrap();
        let executable = std::fs::read(&outputs.executable).unwrap();
        let assembled =
            ezra::vm::assemble_subset_with_symbols_at(AssemblerCpu::I8086, &assembly, 0x0100)
                .unwrap();
        let start = assembled
            .symbols
            .iter()
            .find(|symbol| symbol.name == "__ezra_start")
            .unwrap();

        assert_eq!(
            outputs.executable.extension().and_then(|ext| ext.to_str()),
            Some("com")
        );
        assert_eq!(start.addr, 0x0100);
        assert_eq!(executable, assembled.bytes);
        assert!(assembly.contains("    mov ax,0x4c00\n    int 0x21\n"));
        assert!(!assembly.contains("    cli\n"));
        let _ = std::fs::remove_dir_all(source_path.parent().unwrap());
    }

    #[test]
    fn arbitrary_i8086_target_uses_a_16_bit_default_layout() {
        let source_path = temp_source("generic_layout", "fn main() {}");
        let options = CommandOptions {
            path: source_path.clone(),
            debug_comments: false,
            default_sdk_symbols: true,
            layout_path: None,
            target: Some("custom-board-i8086".to_owned()),
            optimization_level: None,
            enable_optimizations: Vec::new(),
            disable_optimizations: Vec::new(),
        };
        let settings = resolve_build_settings(&options, &source_path).unwrap();

        assert_eq!(settings.layout.name, "bare_i8086");
        assert_eq!(settings.layout.entry.get(), 0);
        assert_eq!(settings.layout.stack.get(), 0xFFFF);
        assert!(
            settings
                .layout
                .regions
                .iter()
                .all(|region| region.end.get() <= 0xFFFF)
        );
        check(&options).unwrap();
        let _ = std::fs::remove_dir_all(source_path.parent().unwrap());
    }

    #[test]
    fn cli_check_strictly_rejects_post_8086_inline_assembly() {
        let source_path = temp_source("strict_check", "fn main() { asm volatile { \"pusha\" } }");
        let error = check(&CommandOptions {
            path: source_path.clone(),
            debug_comments: false,
            default_sdk_symbols: true,
            layout_path: None,
            target: Some("bare-i8086".to_owned()),
            optimization_level: None,
            enable_optimizations: Vec::new(),
            disable_optimizations: Vec::new(),
        })
        .unwrap_err();

        assert!(
            error.contains("assembler does not support 8086 instruction `pusha`"),
            "{error}"
        );
        let _ = std::fs::remove_dir_all(source_path.parent().unwrap());
    }

    #[test]
    fn cli_emit_asm_strictly_rejects_post_8086_inline_assembly() {
        let source_path = temp_source("strict_emit", "fn main() { asm volatile { \"pusha\" } }");
        let error = emit_asm(&CommandOptions {
            path: source_path.clone(),
            debug_comments: false,
            default_sdk_symbols: true,
            layout_path: None,
            target: Some("bare-i8086".to_owned()),
            optimization_level: None,
            enable_optimizations: Vec::new(),
            disable_optimizations: Vec::new(),
        })
        .unwrap_err();

        assert!(
            error.contains("assembler does not support 8086 instruction `pusha`"),
            "{error}"
        );
        let _ = std::fs::remove_dir_all(source_path.parent().unwrap());
    }

    #[test]
    fn cli_emit_asm_rejects_generated_text_that_exceeds_its_region() {
        let source_path = temp_source("emit_text_fit", "fn main() {}");
        let layout_path = source_path.parent().unwrap().join("tiny.ezralayout");
        std::fs::write(
            &layout_path,
            r#"
                layout tiny_i8086 {
                    load 0x0000;
                    entry 0x0000;
                    stack 0xFFFF;

                    region code 0x0000..0x0000 read execute;
                    region rodata 0x0001..0x1FFF read;
                    region ram 0x2000..0x7FFF read write;
                    region assets 0x8000..0x9FFF read;
                    region scratch 0xA000..0xAFFF read write;
                    region stack 0xB000..0xFFFF read write reserved;
                    section .header -> code align 1;
                    section .text -> code align 1;
                    section .rodata -> rodata align 1;
                    section .data -> ram align 1;
                    section .bss -> ram align 1;
                    section .assets -> assets align 1;
                    section .scratch -> scratch align 1;

                    symbol EZRA_LOAD_ADDR = 0x0000;
                    symbol EZRA_ENTRY_ADDR = 0x0000;
                    symbol EZRA_CODE_BASE = 0x0000;
                    symbol EZRA_STACK_TOP = 0xFFFF;
                    symbol EZRA_RAM_BASE = 0x2000;
                    symbol EZRA_RODATA_BASE = 0x0001;
                    symbol EZRA_ASSET_BASE = 0x8000;
                }
            "#,
        )
        .unwrap();

        let error = emit_asm(&CommandOptions {
            path: source_path.clone(),
            debug_comments: false,
            default_sdk_symbols: true,
            layout_path: Some(layout_path.clone()),
            target: Some("bare-i8086".to_owned()),
            optimization_level: None,
            enable_optimizations: Vec::new(),
            disable_optimizations: Vec::new(),
        })
        .unwrap_err();

        assert!(
            error.contains("assembly section `.text`")
                && error.contains("does not fit in region `code`"),
            "{error}"
        );
        let _ = std::fs::remove_dir_all(source_path.parent().unwrap());
    }
}

#[cfg(test)]
mod arduboy_package_tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn builds_a_schema_v2_arduboy_zip_with_stored_hex() {
        let config = ArduboyConfig {
            title: "A \"quoted\" game".to_owned(),
            author: "EZRA".to_owned(),
            version: "1.0.0".to_owned(),
            description: Some("Line one\nLine two".to_owned()),
            date: Some("2026-07-17".to_owned()),
            genre: Some("Puzzle".to_owned()),
            source_url: Some("https://example.com/game".to_owned()),
        };
        let request = ezra::package::PackageRequest {
            target: "arduboy-avr".to_owned(),
            output_format: OutputFormat::Arduboy,
            load_addr: 0,
            entry_addr: 0,
            executable_name: Some("pocket-game".to_owned()),
        };
        let context = ezra::package::PackageContext {
            executable_name: Some("pocket-game".to_owned()),
            arduboy: Some(ezra::package::ArduboyPackageOptions {
                title: config.title,
                author: config.author,
                version: config.version,
                description: config.description,
                date: config.date,
                genre: config.genre,
                source_url: config.source_url,
            }),
            ..ezra::package::PackageContext::new()
        };
        let zip = ezra::package::package_executable_with_context(&request, &context, &[]).unwrap();
        let entries = read_stored_zip_entries(&zip);

        assert_eq!(entries.len(), 2);
        assert_eq!(entries["pocket-game.hex"], b":00000001FF\n");
        let info = std::str::from_utf8(&entries["info.json"]).unwrap();
        assert!(info.contains("\"schemaVersion\":2"), "{info}");
        assert!(
            info.contains("\"title\":\"A \\\"quoted\\\" game\""),
            "{info}"
        );
        assert!(
            info.contains("\"description\":\"Line one\\nLine two\""),
            "{info}"
        );
        assert!(info.contains("\"date\":\"2026-07-17\""), "{info}");
        assert!(info.contains("\"genre\":\"Puzzle\""), "{info}");
        assert!(
            info.contains("\"sourceUrl\":\"https://example.com/game\""),
            "{info}"
        );
        assert!(
            info.contains(
                "\"binaries\":[{\"filename\":\"pocket-game.hex\",\"device\":\"Arduboy\"}]"
            ),
            "{info}"
        );
    }

    fn read_stored_zip_entries(zip: &[u8]) -> BTreeMap<String, Vec<u8>> {
        const LOCAL_FILE_HEADER: u32 = 0x0403_4B50;
        const CENTRAL_DIRECTORY_HEADER: u32 = 0x0201_4B50;
        const END_OF_CENTRAL_DIRECTORY: u32 = 0x0605_4B50;

        let mut entries = BTreeMap::new();
        let mut offset = 0;
        while read_u32(zip, offset) == LOCAL_FILE_HEADER {
            assert_eq!(read_u16(zip, offset + 6), 1 << 11);
            assert_eq!(read_u16(zip, offset + 8), 0, "entry must be stored");
            let size = read_u32(zip, offset + 18) as usize;
            assert_eq!(read_u32(zip, offset + 22) as usize, size);
            let name_len = read_u16(zip, offset + 26) as usize;
            let extra_len = read_u16(zip, offset + 28) as usize;
            let name_start = offset + 30;
            let data_start = name_start + name_len + extra_len;
            let name = std::str::from_utf8(&zip[name_start..name_start + name_len])
                .unwrap()
                .to_owned();
            let data = zip[data_start..data_start + size].to_vec();
            assert_eq!(read_u32(zip, offset + 14), crc32(&data));
            entries.insert(name, data);
            offset = data_start + size;
        }
        assert_eq!(read_u32(zip, offset), CENTRAL_DIRECTORY_HEADER);
        let end_offset = zip.len() - 22;
        assert_eq!(read_u32(zip, end_offset), END_OF_CENTRAL_DIRECTORY);
        assert_eq!(read_u16(zip, end_offset + 10) as usize, entries.len());
        entries
    }

    fn crc32(data: &[u8]) -> u32 {
        let mut crc = !0u32;
        for byte in data {
            crc ^= u32::from(*byte);
            for _ in 0..8 {
                crc = (crc >> 1) ^ (0xEDB8_8320 & (0u32.wrapping_sub(crc & 1)));
            }
        }
        !crc
    }

    fn read_u16(bytes: &[u8], offset: usize) -> u16 {
        u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
    }

    fn read_u32(bytes: &[u8], offset: usize) -> u32 {
        u32::from_le_bytes([
            bytes[offset],
            bytes[offset + 1],
            bytes[offset + 2],
            bytes[offset + 3],
        ])
    }
}

#[cfg(test)]
mod tests;
