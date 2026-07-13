use std::{
    collections::{BTreeMap, HashMap},
    env, fs,
    path::{Path, PathBuf},
    process::ExitCode,
};

use ezra::{
    asm::{
        AssemblyOptions, emit_ez80_assembly_with_options, emit_lr35902_assembly_with_options,
        emit_mos6502_assembly_with_options,
    },
    ast::Program,
    cart::{CartridgeHeader, build_cartridge_map, layout_section_bases},
    compile::{SdkResolver, load_program_with_sdk},
    diagnostic::SourceLocation,
    hir::HirProgram,
    layout::{Layout, parse_layout},
    parser::parse_program,
    project::{AssetConfig, load_nearest_project_config, load_project_config},
    target::{
        Address24, AssemblerCpu, CpuFamily, EZRA_ASSET_BASE, EZRA_AUDIO_BASE, EZRA_RAM_BASE,
        EZRA_RODATA_BASE, EZRA_VRAM_BASE, OutputFormat, TargetProfile, parse_output_format,
        resolve_target_profile,
    },
    tbir::TbirProgram,
    vm::TestRunOptions,
};

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
    let args = env::args().skip(1).collect::<Vec<_>>();
    match args.first().map(String::as_str) {
        Some("check") => {
            let options = CommandOptions::parse(&args[1..])?;
            check(&options)
        }
        Some("build") => {
            let options = BuildCommandOptions::parse(&args[1..])?;
            build(&options)
        }
        Some("emit-asm") => {
            let options = CommandOptions::parse(&args[1..])?;
            emit_asm(&options)
        }
        Some("emit-ir") => {
            let options = EmitIrOptions::parse(&args[1..])?;
            emit_ir(&options)
        }
        Some("test") => {
            let options = CommandOptions::parse(&args[1..])?;
            test_source_with_command_options(&options)
        }
        Some("assemble") => {
            let options = AssembleOptions::parse(&args[1..])?;
            assemble_file(&options)
        }
        Some("init") => {
            let options = InitOptions::parse(&args[1..])?;
            init_project(&options)
        }
        Some("install-syntax") => {
            let options = InstallSyntaxOptions::parse(&args[1..])?;
            install_syntax(&options)
        }
        Some("targets") => {
            print_targets();
            Ok(())
        }
        Some("lsp") => run_lsp(),
        Some("layout") => print_layout(args.get(1).map(String::as_str)),
        Some("header") => print_header(),
        Some("-h" | "--help") | None => {
            print_usage();
            Ok(())
        }
        Some(command) => Err(format!("unknown command `{command}`\n{}", usage())),
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

#[derive(Clone, Debug, Eq, PartialEq)]
struct InitOptions {
    path: PathBuf,
    name: Option<String>,
    target: String,
    force: bool,
}

impl InitOptions {
    fn parse(args: &[String]) -> Result<Self, String> {
        let mut path = None;
        let mut name = None;
        let mut target = "agonlight-mos-ez80".to_owned();
        let mut force = false;
        let mut iter = args.iter();
        while let Some(arg) = iter.next() {
            match arg.as_str() {
                "--name" => {
                    let value = iter.next().ok_or_else(usage)?;
                    name = Some(value.clone());
                }
                "--target" => {
                    let value = iter.next().ok_or_else(usage)?;
                    resolve_target_profile(Some(value))?;
                    target = value.clone();
                }
                "--force" => force = true,
                _ if path.is_none() => path = Some(PathBuf::from(arg)),
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
struct InstallSyntaxOptions {
    editors: Vec<SyntaxEditor>,
    all: bool,
    dry_run: bool,
}

impl InstallSyntaxOptions {
    fn parse(args: &[String]) -> Result<Self, String> {
        let mut editors = Vec::new();
        let mut all = false;
        let mut dry_run = false;
        let mut iter = args.iter();
        while let Some(arg) = iter.next() {
            match arg.as_str() {
                "--all" => all = true,
                "--dry-run" => dry_run = true,
                "--editor" => {
                    let value = iter.next().ok_or_else(usage)?;
                    editors.push(SyntaxEditor::parse(value)?);
                }
                value if !value.starts_with('-') => editors.push(SyntaxEditor::parse(value)?),
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

#[derive(Clone, Debug, Eq, PartialEq)]
struct BuildCommandOptions {
    path: Option<String>,
    debug_comments: bool,
    default_sdk_symbols: bool,
    input_kind: Option<InputKind>,
    assembler_cpu: Option<AssemblerCpu>,
    layout_path: Option<String>,
    target: Option<String>,
}

impl BuildCommandOptions {
    fn parse(args: &[String]) -> Result<Self, String> {
        let mut path = None;
        let mut debug_comments = false;
        let mut default_sdk_symbols = true;
        let mut input_kind = None;
        let mut assembler_cpu = None;
        let mut layout_path = None;
        let mut target = None;
        let mut iter = args.iter();
        while let Some(arg) = iter.next() {
            match arg.as_str() {
                "--debug-comments" => debug_comments = true,
                "--no-default-sdk-symbols" => default_sdk_symbols = false,
                "--input-kind" => {
                    let value = iter.next().ok_or_else(usage)?;
                    input_kind = Some(InputKind::parse(value)?);
                }
                "--cpu" => {
                    let value = iter.next().ok_or_else(usage)?;
                    assembler_cpu = Some(AssemblerCpu::parse(value)?);
                }
                "--layout" => {
                    let value = iter.next().ok_or_else(usage)?;
                    layout_path = Some(value.clone());
                }
                "--target" => {
                    let value = iter.next().ok_or_else(usage)?;
                    target = Some(value.clone());
                }
                _ if path.is_none() => path = Some(arg.clone()),
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
        })
    }

    #[cfg(test)]
    fn with_path(path: String, debug_comments: bool) -> Self {
        Self {
            path: Some(path),
            debug_comments,
            default_sdk_symbols: true,
            input_kind: None,
            assembler_cpu: None,
            layout_path: None,
            target: None,
        }
    }
}

trait BuildOptionsView {
    fn default_sdk_symbols(&self) -> bool;
    fn input_kind(&self) -> Option<InputKind>;
    fn assembler_cpu(&self) -> Option<AssemblerCpu>;
    fn layout_path(&self) -> Option<&String>;
    fn target(&self) -> Option<&String>;
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

    fn layout_path(&self) -> Option<&String> {
        self.layout_path.as_ref()
    }

    fn target(&self) -> Option<&String> {
        self.target.as_ref()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CommandOptions {
    path: String,
    debug_comments: bool,
    default_sdk_symbols: bool,
    layout_path: Option<String>,
    target: Option<String>,
}

impl CommandOptions {
    fn parse(args: &[String]) -> Result<Self, String> {
        let mut path = None;
        let mut debug_comments = false;
        let mut default_sdk_symbols = true;
        let mut layout_path = None;
        let mut target = None;
        let mut iter = args.iter();
        while let Some(arg) = iter.next() {
            match arg.as_str() {
                "--debug-comments" => debug_comments = true,
                "--no-default-sdk-symbols" => default_sdk_symbols = false,
                "--layout" => {
                    let value = iter.next().ok_or_else(usage)?;
                    layout_path = Some(value.clone());
                }
                "--target" => {
                    let value = iter.next().ok_or_else(usage)?;
                    target = Some(value.clone());
                }
                _ if path.is_none() => path = Some(arg.clone()),
                _ => return Err(usage()),
            }
        }
        Ok(Self {
            path: path.ok_or_else(usage)?,
            debug_comments,
            default_sdk_symbols,
            layout_path,
            target,
        })
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

    fn layout_path(&self) -> Option<&String> {
        self.layout_path.as_ref()
    }

    fn target(&self) -> Option<&String> {
        self.target.as_ref()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct AssembleOptions {
    path: String,
    output: Option<String>,
    base_addr: Option<u32>,
    assembler_cpu: Option<AssemblerCpu>,
    layout_path: Option<String>,
    map_path: Option<String>,
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
    fn parse(args: &[String]) -> Result<Self, String> {
        let mut rest = Vec::new();
        let mut stage = IrStage::Tbir;
        let mut iter = args.iter();
        while let Some(arg) = iter.next() {
            if arg == "--stage" {
                let value = iter.next().ok_or_else(usage)?;
                stage = IrStage::parse(value)?;
            } else {
                rest.push(arg.clone());
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
    fn parse(args: &[String]) -> Result<Self, String> {
        let mut path = None;
        let mut output = None;
        let mut base_addr = None;
        let mut assembler_cpu = None;
        let mut layout_path = None;
        let mut map_path = None;
        let mut target = None;
        let mut iter = args.iter();
        while let Some(arg) = iter.next() {
            match arg.as_str() {
                "--output" | "-o" => {
                    let value = iter.next().ok_or_else(usage)?;
                    output = Some(value.clone());
                }
                "--base" => {
                    let value = iter.next().ok_or_else(usage)?;
                    base_addr = Some(parse_cli_u24(value)?);
                }
                "--cpu" => {
                    let value = iter.next().ok_or_else(usage)?;
                    assembler_cpu = Some(AssemblerCpu::parse(value)?);
                }
                "--layout" => {
                    let value = iter.next().ok_or_else(usage)?;
                    layout_path = Some(value.clone());
                }
                "--map" => {
                    let value = iter.next().ok_or_else(usage)?;
                    map_path = Some(value.clone());
                }
                "--target" => {
                    let value = iter.next().ok_or_else(usage)?;
                    target = Some(value.clone());
                }
                _ if path.is_none() => path = Some(arg.clone()),
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
    let source_path = PathBuf::from(&options.path);
    let source = fs::read_to_string(&source_path)
        .map_err(|error| format!("failed to read {}: {error}", source_path.display()))?;
    let target = resolve_target_profile(options.target.as_deref())?;
    let layout_path = options.layout_path.as_ref().map(PathBuf::from);
    let layout = load_layout(layout_path.as_deref(), &target.triple.value)?;
    if let Err(errors) = layout.validate() {
        let message = format_layout_errors(layout_path.as_deref(), errors);
        return Err(format!("layout is invalid:\n{message}"));
    }
    let output_format = target.output_format;
    let assembler_cpu = options
        .assembler_cpu
        .unwrap_or_else(|| AssemblerCpu::from(target.triple.cpu));
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
        default_sdk_symbols: true,
        output_root: source_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("target"),
        executable_name: None,
    };
    let image = if let Some(base_addr) = options.base_addr {
        if settings.output_format == OutputFormat::GameBoyGb && base_addr != 0x0150 {
            return Err("Game Boy assembly must use base address 0x0150".to_owned());
        }
        let assembly = preprocess_assembly(
            &source_path,
            &source,
            &settings.target.triple.value,
            settings.assembler_cpu,
        )?;
        let mut source_options = assembly_source_options(&source_path, &settings.layout);
        source_options.line_origins = assembly.line_origins;
        let assembled = ezra::vm::assemble_subset_with_options_at(
            settings.assembler_cpu,
            &assembly.text,
            base_addr,
            &source_options,
        )
        .map_err(|error| error.to_string())?;
        AssemblyBuildImage {
            map: flat_assembly_map(&settings.layout, assembled.bytes.len(), &assembled.symbols)?,
            bytes: assembled.bytes,
            symbols: assembled.symbols,
        }
    } else {
        build_assembly_image(&source_path, &source, &settings)?
    };
    let output_path = options
        .output
        .as_ref()
        .map(PathBuf::from)
        .unwrap_or_else(|| source_path.with_extension(executable_extension(&settings)));
    let executable = build_executable_bytes(&settings, &image.bytes, Some(&output_path))?;
    fs::write(&output_path, executable)
        .map_err(|error| format!("failed to write {}: {error}", output_path.display()))?;
    println!("wrote {}", output_path.display());
    if let Some(map_path) = options.map_path.as_ref().map(PathBuf::from) {
        fs::write(&map_path, image.map)
            .map_err(|error| format!("failed to write {}: {error}", map_path.display()))?;
        println!("wrote {}", map_path.display());
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
    default_sdk_symbols: bool,
    output_root: PathBuf,
    executable_name: Option<String>,
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
    let layout_path = options.layout_path().map(PathBuf::from).or_else(|| {
        project
            .as_ref()
            .and_then(|project| project.layout_file.clone())
    });
    let layout = load_layout(layout_path.as_deref(), &target.triple.value)?;
    let default_sdk_symbols = options.default_sdk_symbols() && target.default_sdk_symbols;
    let sdk = SdkResolver {
        target: Some(target.triple.value.clone()),
        sdk_roots: project
            .as_ref()
            .map(|project| project.sdk_paths.clone())
            .unwrap_or_default(),
    };
    let output_root = project
        .as_ref()
        .map(|project| project.root.join("target"))
        .unwrap_or_else(|| {
            source_path
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join("target")
        });
    let executable_name = project
        .as_ref()
        .and_then(|project| project.executable.clone());
    let asset_config = project
        .as_ref()
        .map(|project| project.assets.clone())
        .unwrap_or_default();

    Ok(BuildSettings {
        sdk,
        target,
        output_format,
        input_kind,
        assembler_cpu,
        layout,
        layout_path,
        asset_config,
        default_sdk_symbols,
        output_root,
        executable_name,
    })
}

fn ensure_source_codegen_supported(settings: &BuildSettings) -> Result<(), String> {
    if matches!(
        settings.target.triple.cpu,
        CpuFamily::Ez80
            | CpuFamily::Z80
            | CpuFamily::Z80N
            | CpuFamily::Z180
            | CpuFamily::I8080
            | CpuFamily::I8085
            | CpuFamily::Lr35902
            | CpuFamily::Mos6502
    ) {
        return Ok(());
    }

    Err(format!(
        "target `{}` uses CPU `{}`, but EZRA source codegen is not implemented for that CPU; use `assemble` for hand-written assembly or another supported source target",
        settings.target.triple.value,
        settings.target.triple.cpu.as_str()
    ))
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
    if options.cpu == CpuFamily::Lr35902 {
        emit_lr35902_assembly_with_options(program, options)
    } else if options.cpu == CpuFamily::Mos6502 {
        emit_mos6502_assembly_with_options(program, options)
    } else {
        emit_ez80_assembly_with_options(program, options)
    }
}

fn validate_layout_for_target(settings: &BuildSettings) -> Result<(), String> {
    let max_addr = if settings.target.memory.address_width_bits >= 24 {
        Address24::MAX
    } else {
        (1u32 << settings.target.memory.address_width_bits) - 1
    };
    let mut violations = Vec::new();
    if settings.layout.load.get() > max_addr {
        violations.push(format!("load address {}", settings.layout.load));
    }
    if settings.layout.entry.get() > max_addr {
        violations.push(format!("entry address {}", settings.layout.entry));
    }
    if settings.layout.stack.get() > max_addr {
        violations.push(format!("stack address {}", settings.layout.stack));
    }
    for region in &settings.layout.regions {
        if region.start.get() > max_addr || region.end.get() > max_addr {
            violations.push(format!(
                "region `{}` range {}..{}",
                region.name, region.start, region.end
            ));
        }
    }
    for symbol in &settings.layout.symbols {
        if symbol.value.get() > max_addr {
            violations.push(format!("symbol `{}` value {}", symbol.name, symbol.value));
        }
    }

    if violations.is_empty() {
        return Ok(());
    }

    Err(format!(
        "layout `{}` requires addresses outside the {}-bit address space for target `{}`: {}",
        settings.layout.name,
        settings.target.memory.address_width_bits,
        settings.target.triple.value,
        violations.join(", ")
    ))
}

fn build(options: &BuildCommandOptions) -> Result<(), String> {
    let outputs = build_source_with_build_options(options)?;
    println!("wrote {}", outputs.asm.display());
    println!("wrote {}", outputs.map.display());
    println!("wrote {}", outputs.executable.display());
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct BuildOutputs {
    asm: PathBuf,
    map: PathBuf,
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
    })
}

fn build_source_with_build_options(options: &BuildCommandOptions) -> Result<BuildOutputs, String> {
    let source_path = resolve_build_source_path(options)?;
    let source_location = command_source_start_location(&source_path);
    let settings = resolve_build_settings(options, &source_path)?;
    validate_build_layout(&settings)?;
    match detect_input_kind(&source_path, &settings)? {
        InputKind::Ezra => build_ezra_source(&source_path, source_location, &settings, options),
        InputKind::Assembly => build_assembly_source(&source_path, source_location, &settings),
    }
}

fn resolve_build_source_path(options: &BuildCommandOptions) -> Result<PathBuf, String> {
    if let Some(path) = &options.path {
        return Ok(PathBuf::from(path));
    }

    let cwd =
        env::current_dir().map_err(|error| format!("failed to read current directory: {error}"))?;
    let project_path = cwd.join("Ezra.toml");
    let project = load_project_config(&project_path).map_err(|error| error.to_string())?;
    project.input.ok_or_else(|| {
        format!(
            "build requires a source path or `{}` must define `build.input`",
            project_path.display()
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
        Some("ezra") => Ok(InputKind::Ezra),
        Some("asm" | "s" | "z80" | "ez80" | "i8080" | "8080") => Ok(InputKind::Assembly),
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
    let mut program = load_program_with_sdk(source_path, &settings.sdk).map_err(|error| {
        error
            .with_location_if_missing(source_location.clone())
            .to_string()
    })?;
    apply_asset_configuration(&mut program, settings);
    ensure_source_codegen_supported(settings)?;
    let assembly = emit_source_assembly(
        &program,
        assembly_options_from_layout_and_program(
            &settings.layout,
            &program,
            settings.target.triple.cpu,
            options.debug_comments,
            settings.default_sdk_symbols,
        )?,
    )
    .map_err(|error| {
        error
            .with_location_if_missing(source_location.clone())
            .to_string()
    })?;

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
    let executable_path = output_base.with_extension(executable_extension(settings));
    let image = build_assembly_image(source_path, assembly, settings)?;

    if let Some(parent) = output_base.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
    }
    fs::write(&asm_path, assembly)
        .map_err(|error| format!("failed to write {}: {error}", asm_path.display()))?;
    fs::write(&map_path, image.map)
        .map_err(|error| format!("failed to write {}: {error}", map_path.display()))?;
    let executable = if settings
        .target
        .triple
        .value
        .starts_with("agonlight-mos-ez80")
    {
        apply_agon_mos_header(settings.layout.entry.get(), image.bytes)?
    } else {
        build_executable_bytes(settings, &image.bytes, Some(&executable_path))?
    };
    fs::write(&executable_path, executable)
        .map_err(|error| format!("failed to write {}: {error}", executable_path.display()))?;

    Ok(BuildOutputs {
        asm: asm_path,
        map: map_path,
        executable: executable_path,
    })
}

fn write_build_artifacts(
    source_path: &Path,
    source_location: SourceLocation,
    settings: &BuildSettings,
    program: &Program,
    assembly: &str,
) -> Result<BuildOutputs, String> {
    let output_base = build_output_base_path(settings, source_path)?;
    let asm_path = output_base.with_extension("asm");
    let map_path = output_base.with_extension("map");
    let executable_path = output_base.with_extension(executable_extension(settings));

    let assembled = ezra::vm::assemble_subset_with_options_at(
        AssemblerCpu::from(settings.target.triple.cpu),
        assembly,
        settings.layout.entry.get(),
        &assembly_source_options(source_path, &settings.layout),
    )
    .map_err(|error| error.to_string())?;
    let map = build_output_map(settings, program, assembled.bytes.len(), &assembled.symbols)
        .map_err(|error| {
            error
                .with_location_if_missing(source_location.clone())
                .to_string()
        })?;
    if let Some(parent) = output_base.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
    }
    fs::write(&asm_path, assembly)
        .map_err(|error| format!("failed to write {}: {error}", asm_path.display()))?;
    fs::write(&map_path, map)
        .map_err(|error| format!("failed to write {}: {error}", map_path.display()))?;
    let executable = build_executable_bytes(settings, &assembled.bytes, Some(&executable_path))?;
    fs::write(&executable_path, executable)
        .map_err(|error| format!("failed to write {}: {error}", executable_path.display()))?;

    Ok(BuildOutputs {
        asm: asm_path,
        map: map_path,
        executable: executable_path,
    })
}

fn build_output_map(
    settings: &BuildSettings,
    program: &ezra::ast::Program,
    code_len: usize,
    symbols: &[ezra::vm::AssemblySymbol],
) -> Result<String, ezra::diagnostic::Diagnostic> {
    if uses_flat_output_map(settings) {
        let code_len = u32::try_from(code_len).map_err(|_| {
            ezra::diagnostic::Diagnostic::new("program code exceeds 24-bit address space")
        })?;
        let end = settings
            .layout
            .entry
            .get()
            .checked_add(code_len.saturating_sub(1))
            .ok_or_else(|| {
                ezra::diagnostic::Diagnostic::new("program code exceeds 24-bit address space")
            })?;
        return Ok(format!(
            "section      start      end        size\n{:<12} {} 0x{:06X} 0x{:06X}\n",
            ".text", settings.layout.entry, end, code_len
        ));
    }

    build_cartridge_map(program, &settings.layout, code_len, symbols)
}

fn flat_assembly_map(
    layout: &Layout,
    code_len: usize,
    symbols: &[ezra::vm::AssemblySymbol],
) -> Result<String, String> {
    let code_len = u32::try_from(code_len)
        .map_err(|_| "assembled program exceeds the 24-bit address space".to_owned())?;
    let end = layout
        .entry
        .get()
        .checked_add(code_len.saturating_sub(1))
        .ok_or_else(|| "assembled program exceeds the 24-bit address space".to_owned())?;
    let mut out = format!(
        "section      start      end        size\n{:<12} {} 0x{:06X} 0x{:06X}\n",
        ".text", layout.entry, end, code_len
    );
    if !symbols.is_empty() {
        out.push_str("\nsymbol       address\n");
        for symbol in symbols {
            out.push_str(&format!("{:<12} 0x{:06X}\n", symbol.name, symbol.addr));
        }
    }
    Ok(out)
}

fn uses_flat_output_map(settings: &BuildSettings) -> bool {
    settings.output_format == OutputFormat::CpmCom
        || bare_target_cpu(&settings.target.triple.value).is_some()
        || settings.target.triple.value.starts_with("zxspectrum-z80")
        || settings.target.triple.value.starts_with("gameboy-")
        || matches!(
            settings.target.triple.cpu,
            CpuFamily::Chip8 | CpuFamily::SuperChip | CpuFamily::XoChip
        )
        || is_ti_ce_target(&settings.target.triple.value)
        || is_ti_z80_target(&settings.target.triple.value)
}

fn is_ti_ce_target(target: &str) -> bool {
    target.starts_with("ti84plusce-ez80") || target.starts_with("ti83premiumce-ez80")
}

fn is_ti_z80_target(target: &str) -> bool {
    target.starts_with("ti83-z80")
        || target.starts_with("ti83plus-z80")
        || target.starts_with("ti84-z80")
        || target.starts_with("ti84plus-z80")
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct AssemblyBuildImage {
    bytes: Vec<u8>,
    map: String,
    symbols: Vec<ezra::vm::AssemblySymbol>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct AssemblySectionSource {
    name: String,
    source: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ExpandedAssembly {
    text: String,
    line_origins: Vec<SourceLocation>,
}

#[derive(Clone, Debug)]
struct AssemblyMacro {
    parameters: Vec<String>,
    body: Vec<(String, SourceLocation)>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PlacedAssemblySection {
    name: String,
    start: u32,
    bytes: Vec<u8>,
}

fn build_assembly_image(
    source_path: &Path,
    assembly: &str,
    settings: &BuildSettings,
) -> Result<AssemblyBuildImage, String> {
    let assembly = preprocess_assembly(
        source_path,
        assembly,
        &settings.target.triple.value,
        settings.assembler_cpu,
    )?;
    let sections = split_assembly_sections(&assembly.text);
    let section_bases =
        placed_assembly_section_bases(source_path, settings, &sections, &assembly.line_origins)?;
    let mut options = assembly_source_options(source_path, &settings.layout);
    options.line_origins = assembly.line_origins;
    options.section_bases = section_bases
        .iter()
        .map(|(name, start, _)| ezra::vm::AssemblySymbol {
            name: name.clone(),
            addr: *start,
        })
        .collect();
    let assembled = ezra::vm::assemble_subset_with_options_at(
        settings.assembler_cpu,
        &assembly.text,
        settings.layout.load.get(),
        &options,
    )
    .map_err(|error| error.to_string())?;
    let mut placed = Vec::new();
    for (name, start, len) in section_bases {
        validate_assembled_section_fit(&settings.layout, &name, start, len)?;
        let offset = usize::try_from(start.saturating_sub(settings.layout.load.get()))
            .map_err(|_| "assembly image exceeds host addressable memory".to_owned())?;
        let end = offset
            .checked_add(len)
            .ok_or_else(|| "assembly image exceeds host addressable memory".to_owned())?;
        if end > assembled.bytes.len() {
            return Err(format!(
                "assembled section `{name}` extends beyond the linked image"
            ));
        }
        placed.push(PlacedAssemblySection {
            name,
            start,
            bytes: assembled.bytes[offset..end].to_vec(),
        });
    }

    let bytes = assembly_image_bytes(settings, &placed)?;
    let map = assembly_section_map(&placed, &assembled.symbols);
    Ok(AssemblyBuildImage {
        bytes,
        map,
        symbols: assembled.symbols,
    })
}

fn placed_assembly_section_bases(
    source_path: &Path,
    settings: &BuildSettings,
    sections: &[AssemblySectionSource],
    line_origins: &[SourceLocation],
) -> Result<Vec<(String, u32, usize)>, String> {
    let mut lengths = BTreeMap::new();
    for section in sections {
        let len = ezra::vm::measure_assembly_with_options(
            settings.assembler_cpu,
            &section.source,
            &ezra::vm::AssemblerSourceOptions {
                source_path: Some(source_path.to_path_buf()),
                line_origins: line_origins.to_vec(),
                ..ezra::vm::AssemblerSourceOptions::default()
            },
        )
        .map_err(|error| error.to_string())?;
        lengths.insert(section.name.clone(), len);
    }
    for name in lengths.keys() {
        if !settings
            .layout
            .sections
            .iter()
            .any(|section| &section.name == name)
        {
            return Err(format!(
                "assembly section `{name}` is not defined by layout `{}`",
                settings.layout.name
            ));
        }
    }

    let mut cursors = BTreeMap::<String, u32>::new();
    let mut placed = Vec::new();
    for section in &settings.layout.sections {
        let Some(len) = lengths.get(&section.name).copied() else {
            continue;
        };
        let region = settings
            .layout
            .regions
            .iter()
            .find(|region| region.name == section.region)
            .ok_or_else(|| {
                format!(
                    "layout section `{}` targets unknown region `{}`",
                    section.name, section.region
                )
            })?;
        let cursor = cursors
            .entry(region.name.clone())
            .or_insert(region.start.get());
        let start = if section.name == ".text" {
            settings.layout.entry.get()
        } else {
            align_u32(*cursor, section.align)?
        };
        let len_u32 = u32::try_from(len)
            .map_err(|_| format!("section `{}` exceeds 24-bit address space", section.name))?;
        *cursor = start
            .checked_add(len_u32)
            .ok_or_else(|| format!("section `{}` exceeds 24-bit address space", section.name))?;
        placed.push((section.name.clone(), start, len));
    }
    Ok(placed)
}

fn split_assembly_sections(assembly: &str) -> Vec<AssemblySectionSource> {
    let mut sections = BTreeMap::<String, Vec<String>>::new();
    let mut current = ".text".to_owned();
    sections.entry(current.clone()).or_default();
    for (line_index, line) in assembly.lines().enumerate() {
        let trimmed = line.split(';').next().unwrap_or("").trim();
        if let Some(section) = trimmed.strip_prefix("section ") {
            current = section.trim().to_owned();
            let lines = sections.entry(current.clone()).or_default();
            lines.resize(line_index, String::new());
            lines.push(String::new());
        } else {
            let lines = sections.entry(current.clone()).or_default();
            lines.resize(line_index, String::new());
            lines.push(line.to_owned());
        }
    }
    sections
        .into_iter()
        .map(|(name, lines)| AssemblySectionSource {
            name,
            source: lines.join("\n"),
        })
        .collect()
}

fn expand_assembly_includes(
    source_path: &Path,
    assembly: &str,
) -> Result<ExpandedAssembly, String> {
    let mut expanded = ExpandedAssembly {
        text: String::new(),
        line_origins: Vec::new(),
    };
    let root = normalize_include_path(source_path);
    expand_assembly_file(&root, assembly, &mut vec![root.clone()], &mut expanded)?;
    Ok(expanded)
}

/// Expand EZRA assembly directives after recursive includes and before any
/// CPU-specific parsing. Macro sets are ordinary vendored include files.
fn preprocess_assembly(
    source_path: &Path,
    assembly: &str,
    target: &str,
    cpu: AssemblerCpu,
) -> Result<ExpandedAssembly, String> {
    let included = expand_assembly_includes(source_path, assembly)?;
    let lines = included
        .text
        .lines()
        .zip(included.line_origins)
        .map(|(line, origin)| (line.to_owned(), origin))
        .collect::<Vec<_>>();
    expand_assembly_macros(
        lines,
        target,
        cpu,
        &mut HashMap::new(),
        &mut HashMap::new(),
        0,
    )
}

fn expand_assembly_macros(
    lines: Vec<(String, SourceLocation)>,
    target: &str,
    cpu: AssemblerCpu,
    defines: &mut HashMap<String, String>,
    macros: &mut HashMap<String, AssemblyMacro>,
    depth: usize,
) -> Result<ExpandedAssembly, String> {
    if depth > 32 {
        return Err("assembly macro expansion exceeded 32 nested invocations".to_owned());
    }
    let mut output = ExpandedAssembly {
        text: String::new(),
        line_origins: Vec::new(),
    };
    let mut conditions = Vec::<bool>::new();
    let mut active = true;
    let mut index = 0;
    while index < lines.len() {
        let (line, origin) = &lines[index];
        let trimmed = line.split(';').next().unwrap_or_default().trim();
        if let Some(condition) = trimmed.strip_prefix("%if ") {
            let value = evaluate_assembly_condition(condition.trim(), target, cpu, defines)?;
            conditions.push(active);
            active &= value;
            index += 1;
            continue;
        }
        if trimmed == "%else" {
            let Some(parent_active) = conditions.last().copied() else {
                return Err(format!(
                    "{}:{}: `%else` without `%if`",
                    origin.file.display(),
                    origin.line
                ));
            };
            active = parent_active && !active;
            index += 1;
            continue;
        }
        if trimmed == "%endif" {
            active = conditions.pop().ok_or_else(|| {
                format!(
                    "{}:{}: `%endif` without `%if`",
                    origin.file.display(),
                    origin.line
                )
            })?;
            index += 1;
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("%macro ") {
            let (name, parameters) = parse_assembly_macro_signature(rest, origin)?;
            let mut body = Vec::new();
            index += 1;
            while index < lines.len()
                && lines[index].0.split(';').next().unwrap_or_default().trim() != "%endmacro"
            {
                body.push(lines[index].clone());
                index += 1;
            }
            if index == lines.len() {
                return Err(format!(
                    "{}:{}: unterminated macro `{name}`",
                    origin.file.display(),
                    origin.line
                ));
            }
            if active {
                macros.insert(name, AssemblyMacro { parameters, body });
            }
            index += 1;
            continue;
        }
        if !active {
            index += 1;
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("%define ") {
            let (name, value) = rest.split_once(char::is_whitespace).ok_or_else(|| {
                format!(
                    "{}:{}: expected `%define NAME value`",
                    origin.file.display(),
                    origin.line
                )
            })?;
            defines.insert(name.to_owned(), value.trim().to_owned());
            index += 1;
            continue;
        }
        if let Some(name) = trimmed
            .strip_prefix('%')
            .and_then(|rest| rest.split_whitespace().next())
            && let Some(definition) = macros.get(name).cloned()
        {
            let args = trimmed[name.len() + 1..].trim();
            let args = if args.is_empty() {
                Vec::new()
            } else {
                args.split(',').map(|arg| arg.trim().to_owned()).collect()
            };
            if args.len() != definition.parameters.len() {
                return Err(format!(
                    "{}:{}: macro `{name}` expects {} arguments, got {}",
                    origin.file.display(),
                    origin.line,
                    definition.parameters.len(),
                    args.len()
                ));
            }
            let expansion_id = output.line_origins.len();
            let expanded = definition
                .body
                .into_iter()
                .map(|(body, _)| {
                    let mut body = substitute_assembly_defines(&body, defines);
                    for (parameter, value) in definition.parameters.iter().zip(&args) {
                        body = body.replace(&format!("${parameter}"), value);
                    }
                    (
                        body.replace("%%", &format!("__ezra_macro_{expansion_id}_")),
                        origin.clone(),
                    )
                })
                .collect::<Vec<_>>();
            let expanded =
                expand_assembly_macros(expanded, target, cpu, defines, macros, depth + 1)?;
            output.text.push_str(&expanded.text);
            output.line_origins.extend(expanded.line_origins);
            index += 1;
            continue;
        }
        output
            .text
            .push_str(&substitute_assembly_defines(line, defines));
        output.text.push('\n');
        output.line_origins.push(origin.clone());
        index += 1;
    }
    if !conditions.is_empty() {
        return Err("unterminated `%if` in assembly source".to_owned());
    }
    Ok(output)
}

fn parse_assembly_macro_signature(
    text: &str,
    origin: &SourceLocation,
) -> Result<(String, Vec<String>), String> {
    let text = text.trim();
    let (name, parameters) = if let Some((name, parameters)) = text.split_once('(') {
        let parameters = parameters.strip_suffix(')').ok_or_else(|| {
            format!(
                "{}:{}: macro parameter list is missing `)`",
                origin.file.display(),
                origin.line
            )
        })?;
        (name.trim().to_owned(), parameters.trim().to_owned())
    } else {
        let mut parts = text.split_whitespace();
        let name = parts.next().ok_or_else(|| {
            format!(
                "{}:{}: missing macro name",
                origin.file.display(),
                origin.line
            )
        })?;
        let parameters = parts.collect::<Vec<_>>().join(" ");
        (
            name.to_owned(),
            parameters
                .trim()
                .trim_start_matches('(')
                .trim_end_matches(')')
                .to_owned(),
        )
    };
    if name.is_empty() {
        return Err(format!(
            "{}:{}: missing macro name",
            origin.file.display(),
            origin.line
        ));
    }
    Ok((
        name,
        if parameters.is_empty() {
            Vec::new()
        } else {
            parameters
                .split(',')
                .map(|parameter| parameter.trim().to_owned())
                .collect()
        },
    ))
}

fn evaluate_assembly_condition(
    condition: &str,
    target: &str,
    cpu: AssemblerCpu,
    defines: &HashMap<String, String>,
) -> Result<bool, String> {
    if let Some(value) = condition
        .strip_prefix("cpu(")
        .and_then(|value| value.strip_suffix(')'))
    {
        return Ok(cpu.as_str() == value.trim_matches('"'));
    }
    if let Some(value) = condition
        .strip_prefix("target(")
        .and_then(|value| value.strip_suffix(')'))
    {
        return Ok(target == value.trim_matches('"'));
    }
    if let Some(name) = condition
        .strip_prefix("defined(")
        .and_then(|name| name.strip_suffix(')'))
    {
        return Ok(defines.contains_key(name.trim()));
    }
    Err(format!("unsupported assembly condition `{condition}`"))
}

fn substitute_assembly_defines(line: &str, defines: &HashMap<String, String>) -> String {
    defines.iter().fold(line.to_owned(), |line, (name, value)| {
        line.replace(&format!("${{{name}}}"), value)
    })
}

fn expand_assembly_file(
    source_path: &Path,
    assembly: &str,
    stack: &mut Vec<PathBuf>,
    expanded: &mut ExpandedAssembly,
) -> Result<(), String> {
    let base = source_path.parent().unwrap_or_else(|| Path::new("."));
    for (line_index, line) in assembly.lines().enumerate() {
        let trimmed = line.split(';').next().unwrap_or("").trim();
        if let Some(include) = trimmed.strip_prefix("include ") {
            let include = include.trim();
            let Some(include) = include
                .strip_prefix('"')
                .and_then(|include| include.strip_suffix('"'))
            else {
                return Err(format!(
                    "{}:{}: invalid include syntax; expected include \"path\"",
                    source_path.display(),
                    line_index + 1
                ));
            };
            let include_path = normalize_include_path(&base.join(include));
            if let Some(cycle_start) = stack.iter().position(|path| path == &include_path) {
                let mut cycle = stack[cycle_start..]
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>();
                cycle.push(include_path.display().to_string());
                return Err(format!(
                    "{}:{}: assembly include cycle: {}",
                    source_path.display(),
                    line_index + 1,
                    cycle.join(" -> ")
                ));
            }
            let included = fs::read_to_string(&include_path).map_err(|error| {
                format!(
                    "{}:{}: failed to read include {}: {error}",
                    source_path.display(),
                    line_index + 1,
                    include_path.display()
                )
            })?;
            stack.push(include_path.clone());
            expand_assembly_file(&include_path, &included, stack, expanded)?;
            stack.pop();
        } else {
            expanded.text.push_str(line);
            expanded.text.push('\n');
            expanded.line_origins.push(SourceLocation {
                file: source_path.to_path_buf(),
                line: line_index + 1,
                column: 1,
            });
        }
    }
    Ok(())
}

fn normalize_include_path(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn validate_assembled_section_fit(
    layout: &Layout,
    name: &str,
    start: u32,
    len: usize,
) -> Result<(), String> {
    if len == 0 {
        return Ok(());
    }
    let section = layout
        .sections
        .iter()
        .find(|section| section.name == name)
        .ok_or_else(|| {
            format!(
                "assembly section `{name}` is not defined by layout `{}`",
                layout.name
            )
        })?;
    let region = layout
        .regions
        .iter()
        .find(|region| region.name == section.region)
        .ok_or_else(|| {
            format!(
                "layout section `{name}` targets unknown region `{}`",
                section.region
            )
        })?;
    let end = start
        .checked_add(
            u32::try_from(len)
                .map_err(|_| format!("section `{name}` exceeds 24-bit address space"))?
                - 1,
        )
        .ok_or_else(|| format!("section `{name}` exceeds 24-bit address space"))?;
    if start < region.start.get() || end > region.end.get() {
        return Err(format!(
            "assembly section `{name}` range 0x{start:06X}..0x{end:06X} does not fit in region `{}`",
            region.name
        ));
    }
    Ok(())
}

fn assembly_image_bytes(
    settings: &BuildSettings,
    sections: &[PlacedAssemblySection],
) -> Result<Vec<u8>, String> {
    if settings.output_format == OutputFormat::CpmCom {
        return Ok(sections
            .iter()
            .find(|section| section.name == ".text")
            .map(|section| section.bytes.clone())
            .unwrap_or_default());
    }
    let max_end = sections
        .iter()
        .filter(|section| !section.bytes.is_empty())
        .map(|section| section.start + section.bytes.len() as u32)
        .max()
        .unwrap_or(settings.layout.load.get());
    let len = usize::try_from(max_end.saturating_sub(settings.layout.load.get()))
        .map_err(|_| "assembly image exceeds host addressable memory".to_owned())?;
    let mut image = vec![0; len];
    for section in sections {
        let offset = section
            .start
            .checked_sub(settings.layout.load.get())
            .ok_or_else(|| {
                format!(
                    "section `{}` starts before layout load address",
                    section.name
                )
            })?;
        let offset = usize::try_from(offset)
            .map_err(|_| "assembly image exceeds host addressable memory".to_owned())?;
        image[offset..offset + section.bytes.len()].copy_from_slice(&section.bytes);
    }
    Ok(image)
}

fn assembly_section_map(
    sections: &[PlacedAssemblySection],
    symbols: &[ezra::vm::AssemblySymbol],
) -> String {
    let mut out = String::from("section      start      end        size\n");
    for section in sections {
        let len = section.bytes.len() as u32;
        let end = section.start + len.saturating_sub(1);
        out.push_str(&format!(
            "{:<12} 0x{:06X} 0x{:06X} 0x{:06X}\n",
            section.name, section.start, end, len
        ));
    }
    if !symbols.is_empty() {
        out.push_str("\nsymbol       address\n");
        for symbol in symbols {
            out.push_str(&format!("{:<12} 0x{:06X}\n", symbol.name, symbol.addr));
        }
    }
    out
}

fn align_u32(value: u32, align: u32) -> Result<u32, String> {
    if align <= 1 {
        return Ok(value);
    }
    let mask = align - 1;
    value
        .checked_add(mask)
        .map(|value| value & !mask)
        .ok_or_else(|| "aligned address exceeds 24-bit address space".to_owned())
}

fn assembly_source_options(
    source_path: &Path,
    layout: &Layout,
) -> ezra::vm::AssemblerSourceOptions {
    ezra::vm::AssemblerSourceOptions {
        source_path: Some(source_path.to_path_buf()),
        symbols: layout
            .symbols
            .iter()
            .map(|symbol| ezra::vm::AssemblySymbol {
                name: symbol.name.clone(),
                addr: symbol.value.get(),
            })
            .collect(),
        section_bases: Vec::new(),
        line_origins: Vec::new(),
    }
}

fn build_output_base_path(settings: &BuildSettings, source_path: &Path) -> Result<PathBuf, String> {
    let source_parent = source_path.parent().unwrap_or_else(|| Path::new("."));
    let source_stem = match settings.executable_name.as_deref() {
        Some(name) => name,
        None => source_path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .ok_or_else(|| format!("source path `{}` has no file stem", source_path.display()))?,
    };
    let relative_parent = source_parent
        .strip_prefix(
            settings
                .output_root
                .parent()
                .unwrap_or_else(|| Path::new(".")),
        )
        .unwrap_or(source_parent);
    Ok(settings
        .output_root
        .join(&settings.target.triple.value)
        .join(relative_parent)
        .join(source_stem))
}

fn build_executable_bytes(
    settings: &BuildSettings,
    code: &[u8],
    output_path: Option<&Path>,
) -> Result<Vec<u8>, String> {
    if matches!(
        settings.output_format,
        OutputFormat::IntelHex | OutputFormat::ArduinoHex
    ) {
        return Ok(intel_hex_bytes(settings.layout.load.get(), code));
    }
    if settings.output_format == OutputFormat::Ti8xp {
        return ti8xp_bytes(settings, output_path, code);
    }
    if settings.output_format == OutputFormat::ZxSpectrumTap {
        return zx_spectrum_tap_bytes(settings, output_path, code);
    }
    if settings.output_format == OutputFormat::GameBoyGb {
        return game_boy_rom_bytes(settings, output_path, code);
    }
    if matches!(
        settings.output_format,
        OutputFormat::Ti8ek | OutputFormat::Ti8xk
    ) {
        return ti_app_bytes(settings, output_path, code);
    }
    if settings
        .target
        .triple
        .value
        .starts_with("agonlight-mos-ez80")
    {
        return build_agon_mos_executable(settings.layout.entry.get(), code);
    }
    Ok(code.to_vec())
}

fn game_boy_rom_bytes(
    settings: &BuildSettings,
    output_path: Option<&Path>,
    code: &[u8],
) -> Result<Vec<u8>, String> {
    if !settings.target.triple.value.starts_with("gameboy-") {
        return Err(format!(
            "target `{}` does not support Game Boy .gb output",
            settings.target.triple.value
        ));
    }
    if settings.layout.load.get() != 0x0150 || settings.layout.entry.get() != 0x0150 {
        return Err("Game Boy ROM layouts must load and enter at 0x0150".to_owned());
    }
    const ROM_SIZE: usize = 0x8000;
    const CODE_OFFSET: usize = 0x0150;
    if code.len() > ROM_SIZE - CODE_OFFSET {
        return Err("Game Boy ROM-only code exceeds 32 KiB cartridge capacity".to_owned());
    }
    let mut rom = vec![0xFF; ROM_SIZE];
    rom[0x0100..0x0104].copy_from_slice(&[0xC3, 0x50, 0x01, 0x00]);
    rom[0x0104..0x0134].copy_from_slice(&[
        0xCE, 0xED, 0x66, 0x66, 0xCC, 0x0D, 0x00, 0x0B, 0x03, 0x73, 0x00, 0x83, 0x00, 0x0C, 0x00,
        0x0D, 0x00, 0x08, 0x11, 0x1F, 0x88, 0x89, 0x00, 0x0E, 0xDC, 0xCC, 0x6E, 0xE6, 0xDD, 0xDD,
        0xD9, 0x99, 0xBB, 0xBB, 0x67, 0x63, 0x6E, 0x0E, 0xEC, 0xCC, 0xDD, 0xDC, 0x99, 0x9F, 0xBB,
        0xB9, 0x33, 0x3E,
    ]);
    let title = settings
        .executable_name
        .as_deref()
        .or_else(|| {
            output_path
                .and_then(|path| path.file_stem())
                .and_then(|name| name.to_str())
        })
        .unwrap_or("EZRA");
    rom[0x0134..0x0144].fill(0);
    for (slot, ch) in rom[0x0134..0x0143].iter_mut().zip(title.bytes()) {
        *slot = if ch.is_ascii_alphanumeric() || ch == b' ' {
            ch
        } else {
            b'_'
        };
    }
    rom[0x0143] = if settings.target.triple.value.starts_with("gameboy-color-") {
        0xC0
    } else {
        0x00
    };
    rom[0x0144..0x0146].copy_from_slice(b"00");
    rom[0x0146] = 0x00;
    rom[0x0147] = 0x00;
    rom[0x0148] = 0x00;
    rom[0x0149] = 0x00;
    rom[0x014A] = 0x01;
    rom[0x014B] = 0x33;
    rom[0x014C] = 0x00;
    rom[CODE_OFFSET..CODE_OFFSET + code.len()].copy_from_slice(code);
    rom[0x014D] = rom[0x0134..=0x014C].iter().fold(0u8, |checksum, byte| {
        checksum.wrapping_sub(*byte).wrapping_sub(1)
    });
    let checksum = rom
        .iter()
        .enumerate()
        .filter(|(index, _)| !matches!(*index, 0x014E | 0x014F))
        .fold(0u16, |sum, (_, byte)| sum.wrapping_add(u16::from(*byte)));
    rom[0x014E..0x0150].copy_from_slice(&checksum.to_be_bytes());
    Ok(rom)
}

fn ti8xp_bytes(
    settings: &BuildSettings,
    output_path: Option<&Path>,
    code: &[u8],
) -> Result<Vec<u8>, String> {
    let name = ti8xp_variable_name(settings, output_path)?;
    let mut payload = ti8xp_payload_prefix(settings)?.to_vec();
    payload.extend_from_slice(code);
    let payload_len = u16::try_from(payload.len())
        .map_err(|_| "TI .8xp payload exceeds 65535 bytes".to_owned())?;
    let entry_len = payload_len
        .checked_add(13)
        .ok_or_else(|| "TI .8xp entry exceeds 65535 bytes".to_owned())?;

    let mut data = Vec::new();
    push16_le(&mut data, entry_len);
    data.push(0x06); // protected program
    data.extend_from_slice(&name);
    data.push(0x00); // version
    data.push(0x00); // RAM/unarchived flag
    push16_le(&mut data, payload_len);
    data.extend_from_slice(&payload);

    let data_len = u16::try_from(data.len())
        .map_err(|_| "TI .8xp data section exceeds 65535 bytes".to_owned())?;
    let checksum = data
        .iter()
        .fold(0u16, |sum, byte| sum.wrapping_add(u16::from(*byte)));

    let mut out = Vec::with_capacity(11 + 42 + 2 + data.len() + 2);
    out.extend_from_slice(b"**TI83F*\x1A\x0A\x00");
    let mut comment = [0u8; 42];
    let text = b"Generated by ezrac";
    comment[..text.len()].copy_from_slice(text);
    out.extend_from_slice(&comment);
    push16_le(&mut out, data_len);
    out.extend_from_slice(&data);
    push16_le(&mut out, checksum);
    Ok(out)
}

fn zx_spectrum_tap_bytes(
    settings: &BuildSettings,
    output_path: Option<&Path>,
    code: &[u8],
) -> Result<Vec<u8>, String> {
    if !settings.target.triple.value.starts_with("zxspectrum-z80") {
        return Err(format!(
            "target `{}` does not support ZX Spectrum .tap output",
            settings.target.triple.value
        ));
    }
    let load = u16::try_from(settings.layout.load.get())
        .map_err(|_| "ZX Spectrum load address exceeds 16-bit address space".to_owned())?;
    let length = u16::try_from(code.len())
        .map_err(|_| "ZX Spectrum CODE block exceeds 65535 bytes".to_owned())?;

    let mut header = Vec::with_capacity(17);
    header.push(3); // CODE header
    header.extend_from_slice(&zx_tap_name(settings, output_path));
    header.extend_from_slice(&length.to_le_bytes());
    header.extend_from_slice(&load.to_le_bytes());
    header.extend_from_slice(&0u16.to_le_bytes());

    let mut out = Vec::with_capacity(4 + header.len() + code.len());
    push_zx_tap_block(&mut out, 0x00, &header)?;
    push_zx_tap_block(&mut out, 0xFF, code)?;
    Ok(out)
}

fn zx_tap_name(settings: &BuildSettings, output_path: Option<&Path>) -> [u8; 10] {
    let raw = settings
        .executable_name
        .as_deref()
        .or_else(|| {
            output_path
                .and_then(|path| path.file_stem())
                .and_then(|stem| stem.to_str())
        })
        .unwrap_or("EZRA");
    let mut name = [b' '; 10];
    for (slot, ch) in name.iter_mut().zip(raw.chars()) {
        *slot = if ch.is_ascii_alphanumeric() || ch == '_' {
            ch.to_ascii_uppercase() as u8
        } else {
            b'_'
        };
    }
    name
}

fn push_zx_tap_block(out: &mut Vec<u8>, flag: u8, data: &[u8]) -> Result<(), String> {
    let block_len = data
        .len()
        .checked_add(2)
        .ok_or_else(|| "ZX Spectrum TAP block is too large".to_owned())?;
    let block_len = u16::try_from(block_len)
        .map_err(|_| "ZX Spectrum TAP block exceeds 65535 bytes".to_owned())?;
    out.extend_from_slice(&block_len.to_le_bytes());
    out.push(flag);
    out.extend_from_slice(data);
    let checksum = data.iter().fold(flag, |checksum, byte| checksum ^ byte);
    out.push(checksum);
    Ok(())
}

fn ti_app_bytes(
    settings: &BuildSettings,
    output_path: Option<&Path>,
    code: &[u8],
) -> Result<Vec<u8>, String> {
    match settings.output_format {
        OutputFormat::Ti8ek if !is_ti_ce_target(&settings.target.triple.value) => {
            return Err(format!(
                "target `{}` does not support TI CE .8ek app output",
                settings.target.triple.value
            ));
        }
        OutputFormat::Ti8xk if !is_ti_z80_target(&settings.target.triple.value) => {
            return Err(format!(
                "target `{}` does not support classic TI .8xk app output",
                settings.target.triple.value
            ));
        }
        OutputFormat::Ti8ek | OutputFormat::Ti8xk => {}
        _ => unreachable!("non-app output format"),
    }

    let name = ti8xp_variable_name(settings, output_path)?;
    let payload_len = u32::try_from(code.len())
        .map_err(|_| "TI app payload exceeds 32-bit length range".to_owned())?;
    let checksum = code
        .iter()
        .fold(0u16, |sum, byte| sum.wrapping_add(u16::from(*byte)));

    let mut out = Vec::with_capacity(64 + code.len());
    out.extend_from_slice(b"**TIFL**\x1A\x0A\x00");
    out.push(match settings.output_format {
        OutputFormat::Ti8ek => b'E',
        OutputFormat::Ti8xk => b'X',
        _ => unreachable!(),
    });
    out.extend_from_slice(&name);
    out.extend_from_slice(&settings.layout.entry.get().to_le_bytes());
    out.extend_from_slice(&payload_len.to_le_bytes());
    push16_le(&mut out, checksum);
    out.resize(64, 0);
    out.extend_from_slice(code);
    Ok(out)
}

fn ti8xp_payload_prefix(settings: &BuildSettings) -> Result<&'static [u8], String> {
    if is_ti_ce_target(&settings.target.triple.value) {
        Ok(&[0xEF, 0x7B])
    } else if is_ti_z80_target(&settings.target.triple.value) {
        Ok(&[0xBB, 0x6D])
    } else {
        Err(format!(
            "target `{}` does not support TI .8xp output",
            settings.target.triple.value
        ))
    }
}

fn ti8xp_variable_name(
    settings: &BuildSettings,
    output_path: Option<&Path>,
) -> Result<[u8; 8], String> {
    let raw = settings
        .executable_name
        .as_deref()
        .or_else(|| {
            output_path
                .and_then(|path| path.file_stem())
                .and_then(|stem| stem.to_str())
        })
        .unwrap_or("EZRA");
    let mut out = [0u8; 8];
    let mut len = 0;
    for ch in raw.chars() {
        if len == out.len() {
            break;
        }
        if ch.is_ascii_alphanumeric() || ch == '_' {
            out[len] = ch.to_ascii_uppercase() as u8;
            len += 1;
        }
    }
    if len == 0 {
        return Err(format!(
            "TI .8xp variable name `{raw}` does not contain any ASCII letters, digits, or underscores"
        ));
    }
    Ok(out)
}

fn push16_le(out: &mut Vec<u8>, value: u16) {
    out.push(value as u8);
    out.push((value >> 8) as u8);
}

fn intel_hex_bytes(base_addr: u32, code: &[u8]) -> Vec<u8> {
    let mut out = String::new();
    let mut current_upper = None;
    for (offset, chunk) in code.chunks(16).enumerate() {
        let addr = base_addr + (offset * 16) as u32;
        let upper = (addr >> 16) as u16;
        if current_upper != Some(upper) {
            current_upper = Some(upper);
            push_ihex_record(&mut out, 0, 0x04, &upper.to_be_bytes());
        }
        push_ihex_record(&mut out, (addr & 0xFFFF) as u16, 0x00, chunk);
    }
    push_ihex_record(&mut out, 0, 0x01, &[]);
    out.into_bytes()
}

fn push_ihex_record(out: &mut String, address: u16, kind: u8, data: &[u8]) {
    let len = data.len() as u8;
    let mut sum = len
        .wrapping_add((address >> 8) as u8)
        .wrapping_add(address as u8)
        .wrapping_add(kind);
    out.push_str(&format!(":{len:02X}{address:04X}{kind:02X}"));
    for byte in data {
        sum = sum.wrapping_add(*byte);
        out.push_str(&format!("{byte:02X}"));
    }
    out.push_str(&format!("{:02X}\n", (!sum).wrapping_add(1)));
}

fn build_agon_mos_executable(entry: u32, code: &[u8]) -> Result<Vec<u8>, String> {
    if entry > Address24::MAX {
        return Err(format!(
            "Agon MOS entry address 0x{entry:X} is outside the 24-bit address space"
        ));
    }
    let mut out = Vec::with_capacity(69 + code.len());
    out.push(0xC3);
    out.push((entry & 0xFF) as u8);
    out.push(((entry >> 8) & 0xFF) as u8);
    out.push(((entry >> 16) & 0xFF) as u8);
    out.resize(64, 0);
    out.extend_from_slice(b"MOS");
    out.push(0);
    out.push(1);
    out.extend_from_slice(code);
    Ok(out)
}

fn apply_agon_mos_header(entry: u32, mut image: Vec<u8>) -> Result<Vec<u8>, String> {
    if entry > Address24::MAX {
        return Err(format!(
            "Agon MOS entry address 0x{entry:X} is outside the 24-bit address space"
        ));
    }
    image.resize(image.len().max(69), 0);
    image[0..4].copy_from_slice(&[
        0xC3,
        (entry & 0xFF) as u8,
        ((entry >> 8) & 0xFF) as u8,
        ((entry >> 16) & 0xFF) as u8,
    ]);
    image[64..69].copy_from_slice(b"MOS\0\x01");
    Ok(image)
}

#[cfg(test)]
fn test_source(path: &str) -> Result<(), String> {
    test_source_with_command_options(&CommandOptions {
        path: path.to_owned(),
        debug_comments: false,
        default_sdk_symbols: true,
        layout_path: None,
        target: None,
    })
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
    let source_path = PathBuf::from(&options.path);
    let source_location = command_source_start_location(&source_path);
    let source = fs::read_to_string(&source_path)
        .map_err(|error| format!("failed to read {}: {error}", source_path.display()))?;
    let metadata = parse_test_metadata(&source)?;
    let settings = resolve_build_settings(options, &source_path)?;
    let mut program = load_program_with_sdk(&source_path, &settings.sdk).map_err(|error| {
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
        assembly_options_from_layout_and_program(
            &settings.layout,
            &program,
            settings.target.triple.cpu,
            options.debug_comments,
            settings.default_sdk_symbols,
        )?,
    )
    .map_err(|error| error.with_location_if_missing(source_location).to_string())?;
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
    let source_path = PathBuf::from(&options.command.path);
    let source_location = command_source_start_location(&source_path);
    let settings = resolve_build_settings(&options.command, &source_path)?;
    let program = load_program_with_sdk(&source_path, &settings.sdk).map_err(|error| {
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
                &assembly_options_from_layout_and_program(
                    &settings.layout,
                    &program,
                    settings.target.triple.cpu,
                    options.command.debug_comments,
                    settings.default_sdk_symbols,
                )?,
            )
            .map_err(|error| error.with_location_if_missing(source_location).to_string())?;
            print!("{}", tbir.dump_text());
        }
    }
    Ok(())
}

fn emit_assembly_with_command_options(options: &CommandOptions) -> Result<String, String> {
    let source_path = PathBuf::from(&options.path);
    let source_location = command_source_start_location(&source_path);
    let settings = resolve_build_settings(options, &source_path)?;
    let mut program = load_program_with_sdk(&source_path, &settings.sdk).map_err(|error| {
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
    emit_source_assembly(
        &program,
        assembly_options_from_layout_and_program(
            &settings.layout,
            &program,
            settings.target.triple.cpu,
            options.debug_comments,
            settings.default_sdk_symbols,
        )?,
    )
    .map_err(|error| error.with_location_if_missing(source_location).to_string())
}

fn check(options: &CommandOptions) -> Result<(), String> {
    let source_path = PathBuf::from(&options.path);
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
    let mut program = load_program_with_sdk(source_path, &settings.sdk).map_err(|error| {
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
    emit_source_assembly(
        &program,
        assembly_options_from_layout_and_program(
            &settings.layout,
            &program,
            settings.target.triple.cpu,
            options.debug_comments,
            settings.default_sdk_symbols,
        )?,
    )
    .map_err(|error| error.with_location_if_missing(source_location).to_string())?;

    println!(
        "ok: {} imports, {} declarations, main present",
        imports,
        program.declarations.len()
    );
    Ok(())
}

fn print_layout(path: Option<&str>) -> Result<(), String> {
    let layout_path = path.map(PathBuf::from);
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
    if target == "generic-6502-bare" {
        Layout::bare_6502()
    } else if target.starts_with("chip8-") || target == "vm-chip8" {
        Layout::chip8("chip8")
    } else if target.starts_with("schip-") || target.starts_with("superchip-") {
        Layout::chip8("schip")
    } else if target.starts_with("xochip-") {
        Layout::chip8("xochip")
    } else if let Some(cpu) = bare_target_cpu(target) {
        match cpu {
            AssemblerCpu::Ez80 => Layout::bare_ez80(),
            AssemblerCpu::Mos6502 => Layout::bare_6502(),
            _ => Layout::bare_16(cpu.as_str()),
        }
    } else if target.starts_with("zxspectrum-z80") {
        Layout::zx_spectrum_z80()
    } else if target.starts_with("gameboy-") {
        Layout::game_boy_lr35902()
    } else if target.starts_with("arduboy-") {
        Layout::bare_16("arduboy_avr")
    } else if is_ti_ce_target(target) {
        Layout::ti_ce_ez80(target)
    } else if is_ti_z80_target(target) {
        Layout::ti_z80(target)
    } else if target.starts_with("agonlight-mos-ez80") {
        Layout::agon_light_mos()
    } else if target.starts_with("ez180n-ez80") {
        Layout::ez180n()
    } else if target.starts_with("ezra-test-flat-ez80") {
        Layout::ez80_test_flat()
    } else if target.starts_with("ezra-test-split-ez80") {
        Layout::ez80_test_split()
    } else if target.split('-').any(|part| part == "cpm") {
        Layout::cpm_z80_com()
    } else if target.split('-').any(|part| part == "z80") {
        Layout::z80_default()
    } else {
        Layout::ezra_default()
    }
}

fn init_project(options: &InitOptions) -> Result<(), String> {
    let root = &options.path;
    let project_name = options
        .name
        .clone()
        .unwrap_or_else(|| default_project_name(root));
    validate_project_name(&project_name)?;

    fs::create_dir_all(root)
        .map_err(|error| format!("failed to create {}: {error}", root.display()))?;
    write_scaffold_file(
        &root.join(".gitignore"),
        options.force,
        "target/\n*.bin\n*.com\n*.gaem\n*.hex\n*.tap\n*.gb\n*.8xp\n*.8ek\n*.8xk\n*.map\n*.asm\n",
    )?;
    write_scaffold_file(
        &root.join("Ezra.toml"),
        options.force,
        &format!(
            "[project]\nname = \"{project_name}\"\n\n[build]\ninput = \"src/main.ezra\"\ntarget = \"{}\"\noutput = \"{}\"\nexecutable = \"{project_name}\"\n\n[sdk]\npaths = [\"sdk\"]\n",
            options.target,
            resolve_target_profile(Some(&options.target))?
                .output_format
                .extension()
        ),
    )?;
    write_scaffold_file(
        &root.join("README.md"),
        options.force,
        &format!(
            "# {project_name}\n\nBuild with:\n\n```sh\nezrac build\n```\n\nOr from an ezrac checkout:\n\n```sh\ncargo run -- build\n```\n"
        ),
    )?;
    fs::create_dir_all(root.join("src"))
        .map_err(|error| format!("failed to create {}/src: {error}", root.display()))?;
    fs::create_dir_all(root.join("sdk"))
        .map_err(|error| format!("failed to create {}/sdk: {error}", root.display()))?;
    fs::create_dir_all(root.join("assets"))
        .map_err(|error| format!("failed to create {}/assets: {error}", root.display()))?;
    write_scaffold_file(&root.join("sdk/.gitkeep"), options.force, "")?;
    write_scaffold_file(&root.join("assets/.gitkeep"), options.force, "")?;
    write_scaffold_file(
        &root.join("src/main.ezra"),
        options.force,
        &initial_main_source(&options.target),
    )?;
    println!("initialized {}", root.display());
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
            config_home()?.join(".config/micro/syntax/ezra.yaml"),
            include_str!("../editors/micro/ezra.yaml"),
            dry_run,
        ),
        SyntaxEditor::Helix => install_helix_syntax(dry_run),
        SyntaxEditor::Vscode => install_vscode_syntax(dry_run),
        SyntaxEditor::Zed => install_zed_syntax(dry_run),
        SyntaxEditor::NotepadPlusPlus => install_notepadpp_syntax(dry_run),
    }
}

fn config_home() -> Result<PathBuf, String> {
    env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| "HOME is not set".to_owned())
}

fn appdata_home() -> Result<PathBuf, String> {
    if let Some(path) = env::var_os("APPDATA") {
        return Ok(PathBuf::from(path));
    }
    Ok(config_home()?.join(".config"))
}

fn install_vim_syntax(root: PathBuf, dry_run: bool) -> Result<Vec<PathBuf>, String> {
    let files = [
        (
            "ftdetect/ezra.vim",
            include_str!("../editors/vim/ftdetect/ezra.vim"),
        ),
        (
            "ftplugin/ezra.vim",
            include_str!("../editors/vim/ftplugin/ezra.vim"),
        ),
        (
            "syntax/ezra.vim",
            include_str!("../editors/vim/syntax/ezra.vim"),
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
        include_str!("../editors/nano/ezra.nanorc"),
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
    let root = config_home()?.join(".config/helix");
    let files = [
        (
            "languages.toml",
            include_str!("../editors/helix/languages.toml"),
        ),
        (
            "runtime/queries/ezra/highlights.scm",
            include_str!("../editors/helix/queries/highlights.scm"),
        ),
    ];
    write_syntax_files(root, &files, dry_run)
}

fn install_vscode_syntax(dry_run: bool) -> Result<Vec<PathBuf>, String> {
    let root = config_home()?.join(".vscode/extensions/ezra-language");
    let files = [
        (
            "package.json",
            include_str!("../editors/vscode/package.json"),
        ),
        (
            "language-configuration.json",
            include_str!("../editors/vscode/language-configuration.json"),
        ),
        (
            "syntaxes/ezra.tmLanguage.json",
            include_str!("../editors/vscode/syntaxes/ezra.tmLanguage.json"),
        ),
    ];
    write_syntax_files(root, &files, dry_run)
}

fn install_zed_syntax(dry_run: bool) -> Result<Vec<PathBuf>, String> {
    let root = config_home()?.join(".config/zed/extensions/ezra");
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
    ];
    write_syntax_files(root, &files, dry_run)
}

fn install_notepadpp_syntax(dry_run: bool) -> Result<Vec<PathBuf>, String> {
    install_single_syntax_file(
        appdata_home()?.join("Notepad++/userDefineLangs/ezra.xml"),
        include_str!("../editors/notepad++/ezra.xml"),
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

fn write_syntax_file(path: &Path, contents: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
    }
    fs::write(path, contents)
        .map_err(|error| format!("failed to write {}: {error}", path.display()))
}

fn bare_target_cpu(target: &str) -> Option<AssemblerCpu> {
    let parts = target.split('-').collect::<Vec<_>>();
    if !parts.contains(&"bare") {
        return None;
    }
    parts
        .into_iter()
        .find_map(|part| AssemblerCpu::parse(part).ok())
}

fn assembly_options_from_layout(
    layout: &Layout,
    cpu: CpuFamily,
    debug_comments: bool,
    default_sdk_symbols: bool,
) -> AssemblyOptions {
    AssemblyOptions {
        cpu,
        debug_comments,
        default_sdk_symbols,
        mos_executable: layout.name == "agon_light_mos",
        load_addr: layout_symbol(layout, "EZRA_LOAD_ADDR").unwrap_or(layout.load),
        entry_addr: layout_symbol(layout, "EZRA_ENTRY_ADDR").unwrap_or(layout.entry),
        code_base: layout_symbol(layout, "EZRA_CODE_BASE").unwrap_or(layout.entry),
        stack_top: layout_symbol(layout, "EZRA_STACK_TOP").unwrap_or(layout.stack),
        ram_base: layout_symbol(layout, "EZRA_RAM_BASE").unwrap_or(EZRA_RAM_BASE),
        vram_base: layout_symbol(layout, "EZRA_VRAM_BASE").unwrap_or(EZRA_VRAM_BASE),
        audio_base: layout_symbol(layout, "EZRA_AUDIO_BASE").unwrap_or(EZRA_AUDIO_BASE),
        asset_base: layout_symbol(layout, "EZRA_ASSET_BASE").unwrap_or(EZRA_ASSET_BASE),
        rodata_base: layout_symbol(layout, "EZRA_RODATA_BASE").unwrap_or(EZRA_RODATA_BASE),
        section_bases: Vec::new(),
    }
}

fn assembly_options_from_layout_and_program(
    layout: &Layout,
    program: &ezra::ast::Program,
    cpu: CpuFamily,
    debug_comments: bool,
    default_sdk_symbols: bool,
) -> Result<AssemblyOptions, String> {
    let mut options =
        assembly_options_from_layout(layout, cpu, debug_comments, default_sdk_symbols);
    options.section_bases =
        layout_section_bases(program, layout).map_err(|error| error.to_string())?;
    Ok(options)
}

fn layout_symbol(layout: &Layout, name: &str) -> Option<Address24> {
    layout
        .symbols
        .iter()
        .find(|symbol| symbol.name == name)
        .map(|symbol| symbol.value)
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
            triple: "chip8-vm-chip8",
            cpu: "chip8",
            address_width_bits: 12,
            output: "bin",
            sdk: "none",
            status: "assembly-only CHIP-8 target",
        },
        TargetRow {
            triple: "schip-vm-schip",
            cpu: "schip",
            address_width_bits: 12,
            output: "bin",
            sdk: "none",
            status: "assembly-only SUPER-CHIP target",
        },
        TargetRow {
            triple: "xochip-vm-xochip",
            cpu: "xochip",
            address_width_bits: 16,
            output: "bin",
            sdk: "none",
            status: "assembly-only XO-CHIP target",
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
        TargetRow {
            triple: "bare-ez80",
            cpu: "ez80",
            address_width_bits: 24,
            output: "bin",
            sdk: "none",
            status: "bare eZ80 target",
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
    "usage: ezra <command>\n\ncommands:\n  init [--name <name>] [--target <triple>] [--force] [dir]\n                                       create a new EZRA project scaffold\n  install-syntax (--all | [--editor] <editor>...) [--dry-run]\n                                       install editor syntax files for selected editors\n  targets                              list documented target triples, outputs, and SDKs\n  lsp                                  start the language server; requires Cargo feature `lsp`\n  check [--target <triple>] [--debug-comments] [--no-default-sdk-symbols] [--layout <file.ezralayout>] <file.ezra>\n                                       parse and validate a source file\n  build [--target <triple>] [--cpu <mode>] [--input-kind ezra|assembly] [--debug-comments] [--no-default-sdk-symbols] [--layout <file.ezralayout>] [file.ezra|file.asm]\n                                       write .asm, .map, and target executable artifacts\n  emit-asm [--target <triple>] [--debug-comments] [--no-default-sdk-symbols] [--layout <file.ezralayout>] <file.ezra>\n                                       emit readable target assembly\n  emit-ir [--stage hir|tbir] [--target <triple>] [--debug-comments] [--no-default-sdk-symbols] [--layout <file.ezralayout>] <file.ezra>\n                                       emit inspectable HIR or TBIR text\n  test [--target <triple>] [--debug-comments] [--no-default-sdk-symbols] [--layout <file.ezralayout>] <file.ezra>\n                                       emit and run on the target VM\n  assemble [--target <triple>] [--cpu <mode>] [--layout <file.ezralayout>] [--map <file.map>] [--base <addr>] [--output <file.bin>] <file.asm>\n                                       assemble target assembly into a raw binary\n  layout [file.ezralayout]             print the default or custom EZRA layout summary\n  header                               print the default 64-byte cartridge header\n\neditors for install-syntax: vim, neovim, nano, micro, helix, vscode, zed, notepad++".to_owned()
}

#[cfg(test)]
mod tests;
