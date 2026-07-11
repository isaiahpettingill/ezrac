use std::{
    collections::{BTreeMap, HashMap},
    env, fs,
    path::{Path, PathBuf},
    process::ExitCode,
};

use ezra::{
    asm::{AssemblyOptions, emit_ez80_assembly_with_options, emit_lr35902_assembly_with_options},
    ast::Program,
    cart::{CartridgeHeader, build_cartridge_map, layout_section_bases},
    compile::{SdkResolver, load_program_with_sdk},
    diagnostic::SourceLocation,
    hir::HirProgram,
    layout::{Layout, parse_layout},
    parser::parse_program,
    project::{load_nearest_project_config, load_project_config},
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

    Ok(BuildSettings {
        sdk,
        target,
        output_format,
        input_kind,
        assembler_cpu,
        layout,
        layout_path,
        default_sdk_symbols,
        output_root,
        executable_name,
    })
}

fn ensure_ez80_codegen_supported(settings: &BuildSettings) -> Result<(), String> {
    if matches!(
        settings.target.triple.cpu,
        CpuFamily::Ez80
            | CpuFamily::Z80
            | CpuFamily::Z80N
            | CpuFamily::Z180
            | CpuFamily::I8080
            | CpuFamily::I8085
            | CpuFamily::Lr35902
    ) {
        return Ok(());
    }

    Err(format!(
        "target `{}` uses CPU `{}`, but EZRA source codegen is only implemented for eZ80 ADL, Z80-family, 8080-family, and LR35902 targets; use `assemble` for hand-written assembly or a supported source target",
        settings.target.triple.value,
        settings.target.triple.cpu.as_str()
    ))
}

fn emit_source_assembly(
    program: &Program,
    options: AssemblyOptions,
) -> Result<String, ezra::diagnostic::Diagnostic> {
    if options.cpu == CpuFamily::Lr35902 {
        emit_lr35902_assembly_with_options(program, options)
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
    let program = load_program_with_sdk(source_path, &settings.sdk).map_err(|error| {
        error
            .with_location_if_missing(source_location.clone())
            .to_string()
    })?;
    ensure_ez80_codegen_supported(settings)?;
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
    if settings.output_format == OutputFormat::IntelHex {
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
    let program = load_program_with_sdk(&source_path, &settings.sdk).map_err(|error| {
        error
            .with_location_if_missing(source_location.clone())
            .to_string()
    })?;
    if let Err(errors) = settings.layout.validate() {
        let message = format_layout_errors(settings.layout_path.as_deref(), errors);
        return Err(format!("layout is invalid:\n{message}"));
    }
    validate_layout_for_target(&settings)?;
    ensure_ez80_codegen_supported(&settings)?;
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
            ensure_ez80_codegen_supported(&settings)?;
            let tbir = TbirProgram::for_ez80(
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
    let program = load_program_with_sdk(&source_path, &settings.sdk).map_err(|error| {
        error
            .with_location_if_missing(source_location.clone())
            .to_string()
    })?;
    if let Err(errors) = settings.layout.validate() {
        let message = format_layout_errors(settings.layout_path.as_deref(), errors);
        return Err(format!("layout is invalid:\n{message}"));
    }
    validate_layout_for_target(&settings)?;
    ensure_ez80_codegen_supported(&settings)?;
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
    let program = load_program_with_sdk(source_path, &settings.sdk).map_err(|error| {
        error
            .with_location_if_missing(source_location.clone())
            .to_string()
    })?;
    if let Err(errors) = settings.layout.validate() {
        let message = format_layout_errors(settings.layout_path.as_deref(), errors);
        return Err(format!("layout is invalid:\n{message}"));
    }
    validate_layout_for_target(&settings)?;
    ensure_ez80_codegen_supported(&settings)?;
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
    if let Some(cpu) = bare_target_cpu(target) {
        match cpu {
            AssemblerCpu::Ez80 => Layout::bare_ez80(),
            _ => Layout::bare_16(cpu.as_str()),
        }
    } else if target.starts_with("zxspectrum-z80") {
        Layout::zx_spectrum_z80()
    } else if target.starts_with("gameboy-") {
        Layout::game_boy_lr35902()
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
    let mut parts = target.split('-');
    if parts.next()? != "bare" {
        return None;
    }
    parts.find_map(|part| AssemblerCpu::parse(part).ok())
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
mod tests {
    use super::*;

    static CWD_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct CurrentDirGuard {
        previous: PathBuf,
    }

    impl CurrentDirGuard {
        fn switch_to(path: &Path) -> Self {
            let previous = std::env::current_dir().unwrap();
            std::env::set_current_dir(path).unwrap();
            Self { previous }
        }
    }

    impl Drop for CurrentDirGuard {
        fn drop(&mut self) {
            std::env::set_current_dir(&self.previous).unwrap();
        }
    }

    fn temp_root(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "ezra_{name}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    fn assert_ti8xp(bytes: &[u8], name: &[u8; 8], payload_prefix: &[u8]) {
        assert!(bytes.starts_with(b"**TI83F*\x1A\x0A\x00"), "{bytes:02X?}");
        assert_eq!(&bytes[58..66], name);
        let payload_len = u16::from_le_bytes([bytes[68], bytes[69]]) as usize;
        let payload_start = 70;
        assert!(
            bytes[payload_start..payload_start + payload_len].starts_with(payload_prefix),
            "{bytes:02X?}"
        );
        let checksum_offset = payload_start + payload_len;
        let expected = bytes[55..checksum_offset]
            .iter()
            .fold(0u16, |sum, byte| sum.wrapping_add(u16::from(*byte)));
        let actual = u16::from_le_bytes([bytes[checksum_offset], bytes[checksum_offset + 1]]);
        assert_eq!(actual, expected);
    }

    fn assert_ti_app(bytes: &[u8], kind: u8, name: &[u8; 8], entry: u32, payload_prefix: &[u8]) {
        assert!(bytes.starts_with(b"**TIFL**\x1A\x0A\x00"), "{bytes:02X?}");
        assert_eq!(bytes[11], kind);
        assert_eq!(&bytes[12..20], name);
        assert_eq!(
            u32::from_le_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]),
            entry
        );
        assert!(bytes[64..].starts_with(payload_prefix), "{bytes:02X?}");
    }

    fn copy_fixture(root: &Path, name: &str) -> PathBuf {
        let source = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("harness")
            .join(name);
        let destination = root.join(name);
        std::fs::copy(&source, &destination).unwrap_or_else(|error| {
            panic!(
                "failed to copy fixture {} to {}: {error}",
                source.display(),
                destination.display()
            )
        });
        destination
    }

    #[test]
    fn assemble_options_parse_base_and_output() {
        let options = AssembleOptions::parse(&[
            "--base".to_owned(),
            "040000h".to_owned(),
            "-o".to_owned(),
            "out.bin".to_owned(),
            "main.asm".to_owned(),
        ])
        .unwrap();

        assert_eq!(options.path, "main.asm");
        assert_eq!(options.output, Some("out.bin".to_owned()));
        assert_eq!(options.base_addr, Some(0x04_0000));
        assert_eq!(options.target, None);
    }

    #[test]
    fn assemble_options_parse_target() {
        let options = AssembleOptions::parse(&[
            "--target".to_owned(),
            "cpm-2.2-z80".to_owned(),
            "main.asm".to_owned(),
        ])
        .unwrap();

        assert_eq!(options.path, "main.asm");
        assert_eq!(options.target, Some("cpm-2.2-z80".to_owned()));
        assert_eq!(options.output, None);
        assert_eq!(options.base_addr, None);
    }

    #[test]
    fn build_options_parse_input_kind() {
        let options = BuildCommandOptions::parse(&[
            "--input-kind".to_owned(),
            "assembly".to_owned(),
            "--cpu".to_owned(),
            "z180".to_owned(),
            "main.txt".to_owned(),
        ])
        .unwrap();

        assert_eq!(options.path.as_deref(), Some("main.txt"));
        assert_eq!(options.input_kind, Some(InputKind::Assembly));
        assert_eq!(options.assembler_cpu, Some(AssemblerCpu::Z180));
    }

    #[test]
    fn assemble_options_parse_cpu() {
        let options =
            AssembleOptions::parse(&["--cpu".to_owned(), "z80n".to_owned(), "main.asm".to_owned()])
                .unwrap();

        assert_eq!(options.path, "main.asm");
        assert_eq!(options.assembler_cpu, Some(AssemblerCpu::Z80N));
    }

    #[test]
    fn emit_ir_options_parse_stage() {
        let options = EmitIrOptions::parse(&[
            "--stage".to_owned(),
            "hir".to_owned(),
            "game.ezra".to_owned(),
        ])
        .unwrap();

        assert_eq!(options.stage, IrStage::Hir);
        assert_eq!(options.command.path, "game.ezra");
    }

    #[test]
    fn init_options_parse_path_name_target_and_force() {
        let options = InitOptions::parse(&[
            "--name".to_owned(),
            "cafe".to_owned(),
            "--target".to_owned(),
            "agonlight-mos-ez80".to_owned(),
            "--force".to_owned(),
            "game".to_owned(),
        ])
        .unwrap();

        assert_eq!(options.path, PathBuf::from("game"));
        assert_eq!(options.name.as_deref(), Some("cafe"));
        assert_eq!(options.target, "agonlight-mos-ez80");
        assert!(options.force);
    }

    #[test]
    fn install_syntax_options_require_editor_selection() {
        let error = InstallSyntaxOptions::parse(&[]).unwrap_err();

        assert!(error.contains("requires `--all`"), "{error}");
    }

    #[test]
    fn install_syntax_options_parse_selected_editors() {
        let options = InstallSyntaxOptions::parse(&[
            "--editor".to_owned(),
            "vim".to_owned(),
            "nvim".to_owned(),
            "--dry-run".to_owned(),
        ])
        .unwrap();

        assert_eq!(options.editors, [SyntaxEditor::Vim, SyntaxEditor::Neovim]);
        assert!(options.dry_run);
        assert!(!options.all);
    }

    #[test]
    fn init_project_writes_default_scaffold() {
        let root = temp_root("init_project");
        init_project(&InitOptions {
            path: root.clone(),
            name: Some("demo".to_owned()),
            target: "agonlight-mos-ez80".to_owned(),
            force: false,
        })
        .unwrap();

        let config = std::fs::read_to_string(root.join("Ezra.toml")).unwrap();
        let main = std::fs::read_to_string(root.join("src/main.ezra")).unwrap();
        let gitignore = std::fs::read_to_string(root.join(".gitignore")).unwrap();

        assert!(config.contains("name = \"demo\""), "{config}");
        assert!(
            config.contains("target = \"agonlight-mos-ez80\""),
            "{config}"
        );
        assert!(main.contains("import agon.console"), "{main}");
        assert!(main.contains("console.print_line"), "{main}");
        assert!(gitignore.contains("target/"), "{gitignore}");

        let error = init_project(&InitOptions {
            path: root.clone(),
            name: Some("demo".to_owned()),
            target: "agonlight-mos-ez80".to_owned(),
            force: false,
        })
        .unwrap_err();
        assert!(error.contains("refusing to overwrite"), "{error}");

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn assemble_file_writes_raw_binary() {
        let root = temp_root("assemble_file");
        std::fs::create_dir_all(&root).unwrap();
        let source_path = root.join("main.asm");
        let output_path = root.join("main.bin");
        std::fs::write(
            &source_path,
            r#"
                start:
                    ld a, 42h
                    rst.lis 10h
                    ret
            "#,
        )
        .unwrap();

        assemble_file(&AssembleOptions {
            path: source_path.to_string_lossy().into_owned(),
            output: Some(output_path.to_string_lossy().into_owned()),
            base_addr: Some(0x04_0000),
            assembler_cpu: None,
            layout_path: None,
            map_path: None,
            target: None,
        })
        .unwrap();

        assert_eq!(
            std::fs::read(&output_path).unwrap(),
            [0x3E, 0x42, 0x49, 0xD7, 0xC9]
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn assemble_file_writes_cpm_com_for_cpm_z80_target() {
        let root = temp_root("assemble_cpm_file");
        std::fs::create_dir_all(&root).unwrap();
        let source_path = root.join("hello.asm");
        let output_path = source_path.with_extension("com");
        std::fs::write(
            &source_path,
            r#"
                start:
                    ld c, 02h
                    ld e, 48h
                    call 0005h
                    ld c, 00h
                    call 0005h
            "#,
        )
        .unwrap();

        assemble_file(&AssembleOptions {
            path: source_path.to_string_lossy().into_owned(),
            output: None,
            base_addr: None,
            assembler_cpu: None,
            layout_path: None,
            map_path: None,
            target: Some("cpm-2.2-z80".to_owned()),
        })
        .unwrap();

        assert_eq!(
            std::fs::read(&output_path).unwrap(),
            [
                0x0E, 0x02, 0x1E, 0x48, 0xCD, 0x05, 0x00, 0x0E, 0x00, 0xCD, 0x05, 0x00
            ]
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn cpm_z80_examples_assemble_as_com_programs() {
        let root = temp_root("assemble_cpm_examples");
        std::fs::create_dir_all(&root).unwrap();
        let examples = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/cpm-z80");

        for name in ["exit", "hello-char", "hello-line"] {
            let output = root.join(format!("{name}.com"));
            assemble_file(&AssembleOptions {
                path: examples
                    .join(format!("{name}.asm"))
                    .to_string_lossy()
                    .into_owned(),
                output: Some(output.to_string_lossy().into_owned()),
                base_addr: None,
                assembler_cpu: None,
                layout_path: None,
                map_path: None,
                target: Some("cpm-2.2-z80".to_owned()),
            })
            .unwrap();

            assert!(!std::fs::read(output).unwrap().is_empty());
        }

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn game_boy_targets_write_valid_dmg_and_cgb_roms() {
        use ez80::{Cpu, Machine, PlainMachine};

        let root = temp_root("game_boy_roms");
        std::fs::create_dir_all(&root).unwrap();
        let source_path = root.join("main.asm");
        std::fs::write(
            &source_path,
            "di\nld sp, 0FFFEh\nld a, 42h\nldh (80h), a\nhalt\n",
        )
        .unwrap();
        for (target, cgb_flag) in [
            ("gameboy-dmg-lr35902", 0x00),
            ("gameboy-color-lr35902", 0xC0),
        ] {
            let output = root.join(format!("{target}.gb"));
            assemble_file(&AssembleOptions {
                path: source_path.to_string_lossy().into_owned(),
                output: Some(output.to_string_lossy().into_owned()),
                base_addr: None,
                assembler_cpu: None,
                layout_path: None,
                map_path: None,
                target: Some(target.to_owned()),
            })
            .unwrap();
            let rom = std::fs::read(output).unwrap();
            assert_eq!(rom.len(), 0x8000);
            assert_eq!(&rom[0x0100..0x0104], &[0xC3, 0x50, 0x01, 0x00]);
            assert_eq!(rom[0x0143], cgb_flag);
            assert_eq!(&rom[0x0150..0x0155], &[0xF3, 0x31, 0xFE, 0xFF, 0x3E]);

            let mut machine = PlainMachine::new();
            for (address, byte) in rom.iter().copied().enumerate() {
                machine.poke(address as u32, byte);
            }
            let mut cpu = Cpu::new_gameboy();
            cpu.state.set_pc(0x0100);
            for _ in 0..16 {
                if cpu.is_halted() {
                    break;
                }
                cpu.fast_execute_instruction(&mut machine);
            }
            assert!(
                cpu.is_halted(),
                "{target} ROM did not halt in Game Boy CPU mode"
            );
            assert_eq!(
                machine.peek(0xFF80),
                0x42,
                "{target} ROM did not execute LR35902 LDH semantics"
            );

            let header = rom[0x0134..=0x014C]
                .iter()
                .fold(0u8, |sum, byte| sum.wrapping_sub(*byte).wrapping_sub(1));
            assert_eq!(rom[0x014D], header);
            let global = rom
                .iter()
                .enumerate()
                .filter(|(index, _)| !matches!(*index, 0x014E | 0x014F))
                .fold(0u16, |sum, (_, byte)| sum.wrapping_add(u16::from(*byte)));
            assert_eq!(&rom[0x014E..0x0150], &global.to_be_bytes());
        }
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn game_boy_targets_compile_ezra_source_with_embedded_assets() {
        use ez80::{Cpu, Machine, PlainMachine};

        let root = temp_root("game_boy_ezra_source");
        std::fs::create_dir_all(&root).unwrap();
        let source_path = root.join("main.ezra");
        std::fs::write(
            &source_path,
            r#"
                embed tile: bytes = bytes [0x42, 0x18, 0x24, 0x42]

                fn main() {
                    asm volatile {
                        "ld hl, _tile"
                        "ld a, (hl)"
                        "ldh (80h), a"
                    }
                }
            "#,
        )
        .unwrap();

        for target in ["gameboy-dmg-lr35902", "gameboy-color-lr35902"] {
            let outputs = build_source_with_build_options(&BuildCommandOptions {
                path: Some(source_path.to_string_lossy().into_owned()),
                debug_comments: false,
                default_sdk_symbols: true,
                input_kind: Some(InputKind::Ezra),
                assembler_cpu: None,
                layout_path: None,
                target: Some(target.to_owned()),
            })
            .unwrap();
            let rom = std::fs::read(outputs.executable).unwrap();
            assert_eq!(rom.len(), 0x8000);

            let mut machine = PlainMachine::new();
            for (address, byte) in rom.iter().copied().enumerate() {
                machine.poke(address as u32, byte);
            }
            let mut cpu = Cpu::new_gameboy();
            cpu.state.set_pc(0x0100);
            for _ in 0..32 {
                if cpu.is_halted() {
                    break;
                }
                cpu.fast_execute_instruction(&mut machine);
            }
            assert!(cpu.is_halted(), "{target} source ROM did not halt");
            assert_eq!(machine.peek(0xFF80), 0x42);
        }

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn game_boy_source_examples_build_as_roms() {
        let examples = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/gameboy");
        for name in ["serial-hello", "background", "sprite"] {
            let source = examples.join(name).join("src/main.ezra");
            for (target, cgb_flag) in [
                ("gameboy-dmg-lr35902", 0x00),
                ("gameboy-color-lr35902", 0xC0),
            ] {
                let outputs = build_source_with_build_options(&BuildCommandOptions {
                    path: Some(source.to_string_lossy().into_owned()),
                    debug_comments: false,
                    default_sdk_symbols: true,
                    input_kind: Some(InputKind::Ezra),
                    assembler_cpu: None,
                    layout_path: None,
                    target: Some(target.to_owned()),
                })
                .unwrap_or_else(|error| {
                    panic!("failed to build Game Boy example `{name}` for `{target}`: {error}")
                });
                let expected_extension = if target.starts_with("gameboy-color-") {
                    "gbc"
                } else {
                    "gb"
                };
                assert_eq!(
                    outputs
                        .executable
                        .extension()
                        .and_then(|value| value.to_str()),
                    Some(expected_extension)
                );
                let rom = std::fs::read(outputs.executable).unwrap();
                assert_eq!(rom.len(), 0x8000);
                assert_eq!(
                    rom[0x0143], cgb_flag,
                    "wrong compatibility byte for {target}"
                );
            }
        }
    }

    #[test]
    fn game_boy_vendored_sdk_macros_preprocess_and_assemble() {
        let sdk =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("toolchains/gameboy-lr35902/sdk/asm/gb");
        let source_path = sdk.join("fixture.asm");
        let source = r#"
            include "color.inc"
            %GB_AUDIO_ENABLE
            ld hl, GB_OAM
            %GB_SPRITE_SET 32, 40, 1, OAMF_XFLIP
            %GB_SPRITE_HIDE
            ld hl, GB_TILE_DATA_0
            ld de, GB_TILE_DATA_0
            ld b, 1
            %GB_TILE_UPLOAD
            ld hl, GB_BG_MAP_0
            xor a
            %GB_TILEMAP_FILL
            %GB_WAVE_LOAD
            %GB_WAVE_PLAY 0, 20h, 0, 80h
            %GB_TIMER_START 0, 0, TAC_ENABLE + TAC_4096_HZ
            %GB_JOYPAD_READ_DPAD
            %GB_SERIAL_START 65
            %GBC_VRAM_BANK 1
            %GBC_WRAM_BANK 2
            %GBC_BG_COLOR_LOW 80h, 1Fh
            halt
        "#;
        let expanded = preprocess_assembly(
            &source_path,
            source,
            "gameboy-color-lr35902",
            AssemblerCpu::Lr35902,
        )
        .unwrap();
        let bytes =
            ezra::vm::assemble_subset_at(CpuFamily::Lr35902, &expanded.text, 0x0150).unwrap();
        assert!(!bytes.is_empty());
        assert_eq!(bytes.last(), Some(&0x76));
    }

    #[test]
    fn assemble_file_can_write_layout_map() {
        let root = temp_root("assemble_layout_map");
        std::fs::create_dir_all(&root).unwrap();
        let source_path = root.join("main.asm");
        let output_path = root.join("main.com");
        let map_path = root.join("main.map");
        std::fs::write(
            &source_path,
            r#"
            section .text
            start:
                ld c, 00h
                call CPM_BDOS
            "#,
        )
        .unwrap();

        assemble_file(&AssembleOptions {
            path: source_path.to_string_lossy().into_owned(),
            output: Some(output_path.to_string_lossy().into_owned()),
            base_addr: None,
            assembler_cpu: None,
            layout_path: None,
            map_path: Some(map_path.to_string_lossy().into_owned()),
            target: Some("cpm-2.2-z80".to_owned()),
        })
        .unwrap();
        let map = std::fs::read_to_string(map_path).unwrap();

        assert_eq!(
            std::fs::read(output_path).unwrap(),
            [0x0E, 0x00, 0xCD, 0x05, 0x00]
        );
        assert!(map.contains(".text        0x000100"), "{map}");
        assert!(map.contains("start        0x000100"), "{map}");

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn build_writes_artifacts() {
        let root = temp_root("build");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(root.join("lib")).unwrap();
        let source_path = root.join("game.ezra");
        std::fs::write(
            root.join("lib/math.ezra"),
            "pub fn add_one(v: u8) -> u8 { return v + 1 }\n",
        )
        .unwrap();
        std::fs::write(
            root.join("lib/assets.ezra"),
            r#"
            pub const BASE: u8 = 2
            pub const LEN: u8 = BASE + 1
            pub const BYTE: u8 = 0x5A
            "#,
        )
        .unwrap();
        std::fs::write(
            &source_path,
            r#"
            import lib.math
            import lib.assets

            embed palette: bytes = bytes [0x11, 0x22]
            embed blob: bytes = repeat(assets.BYTE, assets.LEN)

            fn main() {
                let x: u8 = add_one(4)
                test.pass()
            }
            "#,
        )
        .unwrap();

        let outputs = build_source(source_path.to_str().unwrap()).unwrap();
        let asm = std::fs::read_to_string(&outputs.asm).unwrap();
        let map = std::fs::read_to_string(&outputs.map).unwrap();
        let bin = std::fs::read(&outputs.executable).unwrap();

        assert!(asm.contains("__ezra_start:"));
        assert!(asm.contains("_add_one:"));
        assert!(
            map.starts_with("section      start      end        size\n"),
            "{map}"
        );
        assert!(
            map.contains(".header      0x010000 0x01003F 0x000040"),
            "{map}"
        );
        assert!(map.contains(".text        0x010040"), "{map}");
        assert!(
            map.contains(".assets:palette 0x100000 0x100001 0x000002"),
            "{map}"
        );
        assert!(
            map.contains(".assets:blob 0x100100 0x100102 0x000003"),
            "{map}"
        );
        assert_eq!(
            outputs.executable.extension().and_then(|ext| ext.to_str()),
            Some("bin")
        );
        assert_eq!(&bin[0..5], &[0xF3, 0x31, 0x00, 0x00, 0xF0]);
        assert!(bin.len() > 5);

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn build_accepts_assembly_input_by_extension() {
        let root = temp_root("build_asm_extension");
        std::fs::create_dir_all(&root).unwrap();
        let source_path = root.join("exit.asm");
        std::fs::write(
            &source_path,
            r#"
            start:
                ld c, 00h
                call 0005h
            "#,
        )
        .unwrap();

        let outputs = build_source_with_command_options(&CommandOptions {
            path: source_path.to_string_lossy().into_owned(),
            debug_comments: false,
            default_sdk_symbols: true,
            layout_path: None,
            target: Some("cpm-2.2-z80".to_owned()),
        })
        .unwrap();

        let asm = std::fs::read_to_string(&outputs.asm).unwrap();
        let map = std::fs::read_to_string(&outputs.map).unwrap();
        let executable = std::fs::read(&outputs.executable).unwrap();

        assert!(asm.contains("start:"), "{asm}");
        assert!(map.contains(".text        0x000100"), "{map}");
        assert_eq!(
            outputs.executable.extension().and_then(|ext| ext.to_str()),
            Some("com")
        );
        assert_eq!(executable, [0x0E, 0x00, 0xCD, 0x05, 0x00]);

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn assembly_build_can_reference_layout_symbols() {
        let root = temp_root("build_asm_layout_symbols");
        std::fs::create_dir_all(&root).unwrap();
        let source_path = root.join("exit.asm");
        std::fs::write(
            &source_path,
            r#"
            start:
                ld c, 00h
                call CPM_BDOS
            "#,
        )
        .unwrap();

        let outputs = build_source_with_build_options(&BuildCommandOptions {
            path: Some(source_path.to_string_lossy().into_owned()),
            debug_comments: false,
            default_sdk_symbols: true,
            input_kind: Some(InputKind::Assembly),
            assembler_cpu: None,
            layout_path: None,
            target: Some("cpm-2.2-z80".to_owned()),
        })
        .unwrap();

        assert_eq!(
            std::fs::read(outputs.executable).unwrap(),
            [0x0E, 0x00, 0xCD, 0x05, 0x00]
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn assembly_build_reports_source_location_for_assembler_errors() {
        let root = temp_root("build_asm_diagnostics");
        std::fs::create_dir_all(&root).unwrap();
        let source_path = root.join("bad.asm");
        std::fs::write(
            &source_path,
            r#"
            start:
                not_an_instruction
            "#,
        )
        .unwrap();

        let error = build_source_with_build_options(&BuildCommandOptions {
            path: Some(source_path.to_string_lossy().into_owned()),
            debug_comments: false,
            default_sdk_symbols: true,
            input_kind: Some(InputKind::Assembly),
            assembler_cpu: None,
            layout_path: None,
            target: Some("cpm-2.2-z80".to_owned()),
        })
        .unwrap_err();

        assert!(error.contains("bad.asm:3:17"), "{error}");
        assert!(
            error.contains("test assembler does not support instruction `not_an_instruction`"),
            "{error}"
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn assembly_build_maps_layout_sections_and_includes() {
        let root = temp_root("build_asm_sections");
        std::fs::create_dir_all(&root).unwrap();
        let source_path = root.join("main.asm");
        std::fs::write(root.join("message.inc"), "db \"OK\"\n").unwrap();
        std::fs::write(
            &source_path,
            r#"
            section .text
            start:
                ld c, 00h
                call CPM_BDOS
            section .rodata
            include "message.inc"
            "#,
        )
        .unwrap();

        let outputs = build_source_with_build_options(&BuildCommandOptions {
            path: Some(source_path.to_string_lossy().into_owned()),
            debug_comments: false,
            default_sdk_symbols: true,
            input_kind: Some(InputKind::Assembly),
            assembler_cpu: None,
            layout_path: None,
            target: Some("cpm-2.2-z80".to_owned()),
        })
        .unwrap();
        let map = std::fs::read_to_string(outputs.map).unwrap();

        assert!(map.contains(".text        0x000100"), "{map}");
        assert!(map.contains(".rodata      0x008000"), "{map}");
        assert!(map.contains("start        0x000100"), "{map}");

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn assembly_includes_expand_recursively_with_origins_and_cycles() {
        let root = temp_root("nested_assembly_includes");
        let lib = root.join("lib");
        std::fs::create_dir_all(&lib).unwrap();
        let source_path = root.join("main.asm");
        let outer_path = lib.join("outer.inc");
        let inner_path = lib.join("inner.inc");
        std::fs::write(&source_path, "include \"lib/outer.inc\"\n").unwrap();
        std::fs::write(&outer_path, "include \"inner.inc\"\n").unwrap();
        std::fs::write(&inner_path, "section .text\nret\n").unwrap();

        let source = std::fs::read_to_string(&source_path).unwrap();
        let expanded = expand_assembly_includes(&source_path, &source).unwrap();
        assert_eq!(expanded.text, "section .text\nret\n");
        assert_eq!(
            expanded.line_origins[1].file,
            inner_path.canonicalize().unwrap()
        );
        assert_eq!(expanded.line_origins[1].line, 2);

        std::fs::write(&inner_path, "include \"outer.inc\"\n").unwrap();
        let error = expand_assembly_includes(&source_path, &source).unwrap_err();
        assert!(error.contains("assembly include cycle"), "{error}");
        assert!(error.contains("outer.inc"), "{error}");
        assert!(error.contains("inner.inc"), "{error}");

        std::fs::write(&outer_path, "include \"missing.inc\"\n").unwrap();
        let error = expand_assembly_includes(&source_path, &source).unwrap_err();
        assert!(error.contains("outer.inc:1"), "{error}");
        assert!(error.contains("missing.inc"), "{error}");

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn assembly_preprocessor_expands_vendored_macros_and_target_conditionals() {
        let root = temp_root("assembly_macros");
        std::fs::create_dir_all(root.join("macros")).unwrap();
        let source_path = root.join("main.asm");
        std::fs::write(
            root.join("macros/test.inc"),
            r#"
                %define EXIT_PORT 0Eh
                %macro finish(code)
                    mvi a, $code
                    out ${EXIT_PORT}
                %endmacro
            "#,
        )
        .unwrap();
        let source = r#"
            include "macros/test.inc"
            %if cpu("i8080")
                %finish 1
            %else
                db 0
            %endif
            %macro twice()
            %%loop:
                nop
                jp %%loop
            %endmacro
            %twice
        "#;
        std::fs::write(&source_path, source).unwrap();

        let expanded =
            preprocess_assembly(&source_path, source, "bare-i8080", AssemblerCpu::I8080).unwrap();
        assert!(expanded.text.contains("mvi a, 1"), "{}", expanded.text);
        assert!(expanded.text.contains("out 0Eh"), "{}", expanded.text);
        assert!(!expanded.text.contains("db 0"), "{}", expanded.text);
        assert!(expanded.text.contains("__ezra_macro_"), "{}", expanded.text);
        assert_eq!(
            expanded.line_origins[0].file,
            normalize_include_path(&source_path)
        );

        let base_source = root.join("base.asm");
        let base_output = root.join("base.com");
        std::fs::write(
            &base_source,
            "%macro exit()\nmvi a, 0\nout 0Eh\n%endmacro\n%exit\n",
        )
        .unwrap();
        assemble_file(&AssembleOptions {
            path: base_source.to_string_lossy().into_owned(),
            output: Some(base_output.to_string_lossy().into_owned()),
            base_addr: Some(0x0100),
            assembler_cpu: None,
            layout_path: None,
            map_path: None,
            target: Some("cpm-2.2-i8080".to_owned()),
        })
        .unwrap();
        assert_eq!(
            std::fs::read(base_output).unwrap(),
            [0x3E, 0x00, 0xD3, 0x0E]
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn assembly_errors_in_nested_includes_report_included_file() {
        let root = temp_root("nested_assembly_diagnostic");
        let lib = root.join("lib");
        std::fs::create_dir_all(&lib).unwrap();
        let source_path = root.join("main.asm");
        std::fs::write(&source_path, "include \"lib/outer.inc\"\n").unwrap();
        std::fs::write(lib.join("outer.inc"), "include \"bad.inc\"\n").unwrap();
        std::fs::write(lib.join("bad.inc"), "; first line\nnot_an_instruction\n").unwrap();

        let error = build_source_with_build_options(&BuildCommandOptions {
            path: Some(source_path.to_string_lossy().into_owned()),
            debug_comments: false,
            default_sdk_symbols: true,
            input_kind: Some(InputKind::Assembly),
            assembler_cpu: None,
            layout_path: None,
            target: Some("cpm-2.2-z80".to_owned()),
        })
        .unwrap_err();

        assert!(error.contains("bad.inc:2:1"), "{error}");
        assert!(error.contains("not_an_instruction"), "{error}");

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn assembly_build_respects_custom_layout_entry() {
        let root = temp_root("build_asm_custom_layout");
        std::fs::create_dir_all(&root).unwrap();
        let source_path = root.join("main.asm");
        let layout_path = root.join("custom.ezralayout");
        std::fs::write(&source_path, "ret\n").unwrap();
        std::fs::write(
            &layout_path,
            r#"
            layout custom_z80 {
                load  0x000200;
                entry 0x000220;
                stack 0x00FF00;

                region code    0x000200..0x007FFF read execute;
                region rodata  0x008000..0x009FFF read;
                region ram     0x00A000..0x00BFFF read write;
                region assets  0x00C000..0x00DFFF read;
                region scratch 0x00E000..0x00EFFF read write;
                region stack   0x00F000..0x00FFFF read write reserved;

                section .header  -> code    align 1;
                section .text    -> code    align 16;
                section .rodata  -> rodata  align 16;
                section .data    -> ram     align 16;
                section .bss     -> ram     align 16;
                section .assets  -> assets  align 256;
                section .scratch -> scratch align 16;
            }
            "#,
        )
        .unwrap();

        let outputs = build_source_with_build_options(&BuildCommandOptions {
            path: Some(source_path.to_string_lossy().into_owned()),
            debug_comments: false,
            default_sdk_symbols: true,
            input_kind: Some(InputKind::Assembly),
            assembler_cpu: None,
            layout_path: Some(layout_path.to_string_lossy().into_owned()),
            target: Some("zxspectrum-z80".to_owned()),
        })
        .unwrap();
        let map = std::fs::read_to_string(outputs.map).unwrap();
        let tape = std::fs::read(outputs.executable).unwrap();

        assert!(map.contains(".text        0x000220"), "{map}");
        assert_eq!(u16::from_le_bytes([tape[16], tape[17]]), 0x0200);
        assert_eq!(tape[21 + 3 + 0x20], 0xC9);

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn agon_mos_assembly_build_emits_mos_wrapper() {
        let root = temp_root("build_asm_agon_mos");
        std::fs::create_dir_all(&root).unwrap();
        let source_path = root.join("main.asm");
        std::fs::write(&source_path, "ret\n").unwrap();

        let outputs = build_source_with_build_options(&BuildCommandOptions {
            path: Some(source_path.to_string_lossy().into_owned()),
            debug_comments: false,
            default_sdk_symbols: true,
            input_kind: Some(InputKind::Assembly),
            assembler_cpu: None,
            layout_path: None,
            target: Some("agonlight-mos-ez80".to_owned()),
        })
        .unwrap();
        let bin = std::fs::read(outputs.executable).unwrap();

        assert_eq!(bin[0], 0xC3);
        assert_eq!(&bin[64..67], b"MOS");
        assert_eq!(bin[69], 0xC9);

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn agon_assembly_links_cross_section_symbols_and_preserves_sections() {
        let root = temp_root("build_asm_agon_sections");
        std::fs::create_dir_all(&root).unwrap();
        let source_path = root.join("main.asm");
        std::fs::write(
            &source_path,
            r#"section .header
                db 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0A5h
            section .text
            TEXT_DATA equ rodata_value
            start:
                ld hl, TEXT_DATA
                ld de, data_value
                ret
            section .rodata
            rodata_value:
                db 0AAh, 0BBh
            section .data
            data_value:
                db 0CCh, 0DDh
            section .bss
            bss_value:
                db 0EEh
            section .assets
            asset_value:
                db 0F0h
            "#,
        )
        .unwrap();

        let outputs = build_source_with_build_options(&BuildCommandOptions {
            path: Some(source_path.to_string_lossy().into_owned()),
            debug_comments: false,
            default_sdk_symbols: true,
            input_kind: Some(InputKind::Assembly),
            assembler_cpu: None,
            layout_path: None,
            target: Some("agonlight-mos-ez80".to_owned()),
        })
        .unwrap();
        let bin = std::fs::read(outputs.executable).unwrap();
        let map = std::fs::read_to_string(outputs.map).unwrap();

        assert_eq!(&bin[64..69], b"MOS\0\x01");
        assert_eq!(bin[10], 0xA5);
        assert_eq!(&bin[0x20_000..0x20_002], &[0xAA, 0xBB]);
        assert_eq!(&bin[0x30_000..0x30_002], &[0xCC, 0xDD]);
        assert_eq!(bin[0x30_010], 0xEE);
        assert_eq!(bin[0x80_000], 0xF0);
        assert!(map.contains("rodata_value 0x060000"), "{map}");
        assert!(map.contains("TEXT_DATA    0x060000"), "{map}");
        assert!(map.contains("data_value   0x070000"), "{map}");
        assert!(map.contains("bss_value    0x070010"), "{map}");
        assert_eq!(&bin[69..73], &[0x21, 0x00, 0x00, 0x06]);
        assert_eq!(&bin[73..77], &[0x11, 0x00, 0x00, 0x07]);

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn bare_z80_source_build_starts_at_zero_without_header() {
        let root = temp_root("bare_z80_source_bin");
        std::fs::create_dir_all(&root).unwrap();
        let source_path = root.join("main.ezra");
        std::fs::write(&source_path, "fn main() {}\n").unwrap();

        let outputs = build_source_with_build_options(&BuildCommandOptions {
            path: Some(source_path.to_string_lossy().into_owned()),
            debug_comments: false,
            default_sdk_symbols: true,
            input_kind: Some(InputKind::Ezra),
            assembler_cpu: None,
            layout_path: None,
            target: Some("bare-z80".to_owned()),
        })
        .unwrap();
        let map = std::fs::read_to_string(outputs.map).unwrap();
        let bin = std::fs::read(outputs.executable).unwrap();

        assert!(map.contains(".text        0x000000"), "{map}");
        assert!(!bin.starts_with(b"EZRA"), "{bin:02X?}");
        assert!(!bin.is_empty());

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn bare_source_build_can_emit_com_and_intel_hex() {
        for (name, output, extension, prefix) in [
            ("bare_z80_source_com", "com", "com", ""),
            ("bare_z80_source_hex", "hex", "hex", ":020000040000FA"),
        ] {
            let root = temp_root(name);
            std::fs::create_dir_all(&root).unwrap();
            std::fs::write(
                root.join("Ezra.toml"),
                format!(
                    r#"
                    [project]
                    name = "bare-demo"

                    [build]
                    input = "main.ezra"
                    target = "bare-z80"
                    output = "{output}"
                    executable = "demo"
                    "#
                ),
            )
            .unwrap();
            let source_path = root.join("main.ezra");
            std::fs::write(&source_path, "fn main() {}\n").unwrap();

            let outputs = build_source(source_path.to_str().unwrap()).unwrap();
            let bytes = std::fs::read(&outputs.executable).unwrap();

            assert_eq!(
                outputs.executable.extension().and_then(|ext| ext.to_str()),
                Some(extension)
            );
            if !prefix.is_empty() {
                let text = String::from_utf8(bytes).unwrap();
                assert!(text.starts_with(prefix), "{text}");
                assert!(text.ends_with(":00000001FF\n"), "{text}");
            }

            let _ = std::fs::remove_dir_all(root);
        }
    }

    #[test]
    fn bare_z80n_source_build_accepts_z80n_inline_asm() {
        let root = temp_root("bare_z80n_source_inline_asm");
        std::fs::create_dir_all(&root).unwrap();
        let source_path = root.join("main.ezra");
        std::fs::write(
            &source_path,
            r#"
                fn main() {
                    asm volatile {
                        "nextreg 12h,a"
                    }
                }
            "#,
        )
        .unwrap();

        let outputs = build_source_with_build_options(&BuildCommandOptions {
            path: Some(source_path.to_string_lossy().into_owned()),
            debug_comments: false,
            default_sdk_symbols: true,
            input_kind: Some(InputKind::Ezra),
            assembler_cpu: None,
            layout_path: None,
            target: Some("bare-z80n".to_owned()),
        })
        .unwrap();
        let asm = std::fs::read_to_string(outputs.asm).unwrap();
        let bin = std::fs::read(outputs.executable).unwrap();

        assert!(asm.contains("; target: z80n"), "{asm}");
        assert!(bin.windows(3).any(|bytes| bytes == [0xED, 0x92, 0x12]));

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn z80_source_build_rejects_z80n_inline_asm() {
        let root = temp_root("z80_rejects_z80n_inline_asm");
        std::fs::create_dir_all(&root).unwrap();
        let source_path = root.join("main.ezra");
        std::fs::write(
            &source_path,
            r#"
                fn main() {
                    asm volatile {
                        "nextreg 12h,a"
                    }
                }
            "#,
        )
        .unwrap();

        let error = build_source_with_build_options(&BuildCommandOptions {
            path: Some(source_path.to_string_lossy().into_owned()),
            debug_comments: false,
            default_sdk_symbols: true,
            input_kind: Some(InputKind::Ezra),
            assembler_cpu: None,
            layout_path: None,
            target: Some("bare-z80".to_owned()),
        })
        .unwrap_err();

        assert!(
            error.contains("test assembler does not support instruction `nextreg 12h,a`"),
            "{error}"
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn bare_z180_source_build_accepts_z180_inline_asm() {
        let root = temp_root("bare_z180_source_inline_asm");
        std::fs::create_dir_all(&root).unwrap();
        let source_path = root.join("main.ezra");
        std::fs::write(
            &source_path,
            r#"
                fn main() {
                    asm volatile(clobber flags) {
                        "tst a"
                    }
                }
            "#,
        )
        .unwrap();

        let outputs = build_source_with_build_options(&BuildCommandOptions {
            path: Some(source_path.to_string_lossy().into_owned()),
            debug_comments: false,
            default_sdk_symbols: true,
            input_kind: Some(InputKind::Ezra),
            assembler_cpu: None,
            layout_path: None,
            target: Some("bare-z180".to_owned()),
        })
        .unwrap();
        let asm = std::fs::read_to_string(outputs.asm).unwrap();
        let bin = std::fs::read(outputs.executable).unwrap();

        assert!(asm.contains("; target: z180"), "{asm}");
        assert!(bin.windows(2).any(|bytes| bytes == [0xED, 0x3C]));

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn z80_family_source_builds_reject_z180_only_inline_asm() {
        for target in ["bare-z80", "bare-z80n"] {
            let root = temp_root(target);
            std::fs::create_dir_all(&root).unwrap();
            let source_path = root.join("main.ezra");
            std::fs::write(
                &source_path,
                r#"
                    fn main() {
                        asm volatile(clobber flags) {
                            "tst a"
                        }
                    }
                "#,
            )
            .unwrap();

            let error = build_source_with_build_options(&BuildCommandOptions {
                path: Some(source_path.to_string_lossy().into_owned()),
                debug_comments: false,
                default_sdk_symbols: true,
                input_kind: Some(InputKind::Ezra),
                assembler_cpu: None,
                layout_path: None,
                target: Some(target.to_owned()),
            })
            .unwrap_err();

            assert!(
                error.contains("test assembler does not support instruction `tst a`"),
                "{target}: {error}"
            );

            let _ = std::fs::remove_dir_all(root);
        }
    }

    #[test]
    fn bare_i8080_source_build_emits_intel_assembly() {
        let root = temp_root("bare_i8080_source_build");
        std::fs::create_dir_all(&root).unwrap();
        let source_path = root.join("main.ezra");
        std::fs::write(&source_path, "fn main() { test.pass() }\n").unwrap();

        let outputs = build_source_with_build_options(&BuildCommandOptions {
            path: Some(source_path.to_string_lossy().into_owned()),
            debug_comments: false,
            default_sdk_symbols: true,
            input_kind: Some(InputKind::Ezra),
            assembler_cpu: None,
            layout_path: None,
            target: Some("bare-i8080".to_owned()),
        })
        .unwrap();
        let asm = std::fs::read_to_string(outputs.asm).unwrap();
        let bin = std::fs::read(outputs.executable).unwrap();

        assert!(asm.contains("; target: i8080"), "{asm}");
        assert!(asm.contains("    lxi sp,"), "{asm}");
        assert!(asm.contains("    call _main"), "{asm}");
        assert!(asm.contains("    out 0Dh"), "{asm}");
        assert!(!asm.contains("    ld "), "{asm}");
        assert!(!asm.contains("ldir"), "{asm}");
        assert!(!bin.is_empty());

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn bare_i8080_source_builds_core_language_program() {
        let root = temp_root("bare_i8080_source_core_language");
        std::fs::create_dir_all(&root).unwrap();
        let source_path = root.join("main.ezra");
        std::fs::write(
            &source_path,
            r#"
                fn main() {
                    let left: u8 = 2
                    let right: u8 = 3
                    let sum: u8 = left + right
                    if sum == 5 {
                        test.pass()
                    } else {
                        test.fail(1)
                    }
                }
            "#,
        )
        .unwrap();

        let outputs = build_source_with_build_options(&BuildCommandOptions {
            path: Some(source_path.to_string_lossy().into_owned()),
            debug_comments: false,
            default_sdk_symbols: true,
            input_kind: Some(InputKind::Ezra),
            assembler_cpu: None,
            layout_path: None,
            target: Some("bare-i8080".to_owned()),
        })
        .unwrap();
        let asm = std::fs::read_to_string(outputs.asm).unwrap();

        assert!(
            asm.contains("    adi ") || asm.contains("    add "),
            "{asm}"
        );
        assert!(asm.contains("    j"), "{asm}");
        assert!(!asm.contains("sbc hl"), "{asm}");
        assert!(!asm.contains("ldir"), "{asm}");

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn bare_i8085_source_build_accepts_i8085_inline_asm() {
        let root = temp_root("bare_i8085_source_inline_asm");
        std::fs::create_dir_all(&root).unwrap();
        let source_path = root.join("main.ezra");
        std::fs::write(
            &source_path,
            r#"
                fn main() {
                    asm volatile {
                        "rim"
                        "sim"
                    }
                }
            "#,
        )
        .unwrap();

        let outputs = build_source_with_build_options(&BuildCommandOptions {
            path: Some(source_path.to_string_lossy().into_owned()),
            debug_comments: false,
            default_sdk_symbols: true,
            input_kind: Some(InputKind::Ezra),
            assembler_cpu: None,
            layout_path: None,
            target: Some("bare-i8085".to_owned()),
        })
        .unwrap();
        let asm = std::fs::read_to_string(outputs.asm).unwrap();
        let bin = std::fs::read(outputs.executable).unwrap();

        assert!(asm.contains("; target: i8085"), "{asm}");
        assert!(bin.windows(2).any(|bytes| bytes == [0x20, 0x30]));

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn bare_i8080_source_build_rejects_i8085_inline_asm() {
        let root = temp_root("bare_i8080_rejects_i8085_inline_asm");
        std::fs::create_dir_all(&root).unwrap();
        let source_path = root.join("main.ezra");
        std::fs::write(
            &source_path,
            r#"
                fn main() {
                    asm volatile {
                        "rim"
                    }
                }
            "#,
        )
        .unwrap();

        let error = build_source_with_build_options(&BuildCommandOptions {
            path: Some(source_path.to_string_lossy().into_owned()),
            debug_comments: false,
            default_sdk_symbols: true,
            input_kind: Some(InputKind::Ezra),
            assembler_cpu: None,
            layout_path: None,
            target: Some("bare-i8080".to_owned()),
        })
        .unwrap_err();

        assert!(
            error.contains("test assembler does not support instruction `rim`"),
            "{error}"
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn bare_assembly_targets_cover_each_cpu_mode() {
        let cases = [
            ("bare-i8080", "mvi a, 42h\nret\n", vec![0x3E, 0x42, 0xC9]),
            ("bare-i8085", "rim\nsim\nret\n", vec![0x20, 0x30, 0xC9]),
            ("bare-z80", "ld a, 42h\nret\n", vec![0x3E, 0x42, 0xC9]),
            ("bare-z80n", "ld a, 42h\nret\n", vec![0x3E, 0x42, 0xC9]),
            ("bare-z180", "mlt bc\nret\n", vec![0xED, 0x4C, 0xC9]),
            ("bare-ez80", "ld a, 42h\nret\n", vec![0x3E, 0x42, 0xC9]),
        ];

        for (target, source, expected) in cases {
            let root = temp_root(target);
            std::fs::create_dir_all(&root).unwrap();
            let source_path = root.join("main.asm");
            let output_path = root.join("main.bin");
            std::fs::write(&source_path, source).unwrap();

            assemble_file(&AssembleOptions {
                path: source_path.to_string_lossy().into_owned(),
                output: Some(output_path.to_string_lossy().into_owned()),
                base_addr: None,
                assembler_cpu: None,
                layout_path: None,
                map_path: None,
                target: Some(target.to_owned()),
            })
            .unwrap();

            assert_eq!(std::fs::read(output_path).unwrap(), expected, "{target}");
            let _ = std::fs::remove_dir_all(root);
        }
    }

    #[test]
    fn build_uses_project_input_kind_for_assembly() {
        let root = temp_root("build_asm_project_kind");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("Ezra.toml"),
            r#"
            [project]
            name = "asm-demo"

            [build]
            target = "cpm-2.2-z80"
            output = "com"
            input_kind = "assembly"
            executable = "demo"
            "#,
        )
        .unwrap();
        let source_path = root.join("src/main.txt");
        std::fs::write(
            &source_path,
            r#"
            start:
                ld c, 00h
                call 0005h
            "#,
        )
        .unwrap();

        let outputs = build_source(source_path.to_str().unwrap()).unwrap();
        let expected_base = root.join("target/cpm-2.2-z80/src/demo");

        assert_eq!(outputs.asm, expected_base.with_extension("asm"));
        assert_eq!(outputs.map, expected_base.with_extension("map"));
        assert_eq!(outputs.executable, expected_base.with_extension("com"));
        assert_eq!(
            std::fs::read(outputs.executable).unwrap(),
            [0x0E, 0x00, 0xCD, 0x05, 0x00]
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn build_uses_project_input_when_path_is_omitted() {
        let _lock = CWD_LOCK.lock().unwrap();
        let root = temp_root("build_project_input");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("Ezra.toml"),
            r#"
            [project]
            name = "asm-demo"

            [build]
            input = "src/main.asm"
            target = "cpm-2.2-z80"
            input_kind = "assembly"
            executable = "demo"
            "#,
        )
        .unwrap();
        std::fs::write(
            root.join("src/main.asm"),
            r#"
            start:
                ld c, 00h
                call 0005h
            "#,
        )
        .unwrap();

        let _cwd = CurrentDirGuard::switch_to(&root);
        let outputs = build_source_with_build_options(&BuildCommandOptions {
            path: None,
            debug_comments: false,
            default_sdk_symbols: true,
            input_kind: None,
            assembler_cpu: None,
            layout_path: None,
            target: None,
        })
        .unwrap();
        let expected_base = root.join("target/cpm-2.2-z80/src/demo");

        assert_eq!(outputs.asm, expected_base.with_extension("asm"));
        assert_eq!(outputs.map, expected_base.with_extension("map"));
        assert_eq!(outputs.executable, expected_base.with_extension("com"));
        assert_eq!(
            std::fs::read(outputs.executable).unwrap(),
            [0x0E, 0x00, 0xCD, 0x05, 0x00]
        );

        drop(_cwd);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn build_can_emit_debug_source_comments() {
        let root = temp_root("debug_build");
        std::fs::create_dir_all(&root).unwrap();
        let source_path = root.join("game.ezra");
        std::fs::write(
            &source_path,
            "fn main() { let x: u8 = 4; x += 1; test.pass() }\n",
        )
        .unwrap();

        let plain = build_source(source_path.to_str().unwrap()).unwrap();
        let plain_asm = std::fs::read_to_string(&plain.asm).unwrap();
        let debug = build_source_with_options(source_path.to_str().unwrap(), true).unwrap();
        let debug_asm = std::fs::read_to_string(&debug.asm).unwrap();

        assert!(!plain_asm.contains("; source:"), "{plain_asm}");
        assert!(debug_asm.contains("; source: let x: u8 = 4"), "{debug_asm}");
        assert!(debug_asm.contains("; source: x += 1"), "{debug_asm}");

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn commands_report_source_locations_for_semantic_errors() {
        let root = temp_root("command_diagnostics");
        std::fs::create_dir_all(&root).unwrap();
        let source_path = root.join("game.ezra");
        std::fs::write(
            &source_path,
            "fn main() { let value: u8 = 256; test.pass() }\n",
        )
        .unwrap();
        let prefix = format!("{}:1:29:", source_path.display());

        let build_error = build_source(source_path.to_str().unwrap()).unwrap_err();
        assert!(
            build_error.starts_with(&prefix),
            "expected `{build_error}` to start with `{prefix}`"
        );
        let emit_error = emit_assembly_with_command_options(&CommandOptions {
            path: source_path.to_string_lossy().into_owned(),
            debug_comments: false,
            default_sdk_symbols: true,
            layout_path: None,
            target: None,
        })
        .unwrap_err();
        assert!(
            emit_error.starts_with(&prefix),
            "expected `{emit_error}` to start with `{prefix}`"
        );
        let test_error = test_source(source_path.to_str().unwrap()).unwrap_err();
        assert!(
            test_error.starts_with(&prefix),
            "expected `{test_error}` to start with `{prefix}`"
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn commands_report_source_locations_for_layout_errors() {
        let root = temp_root("layout_diagnostics");
        std::fs::create_dir_all(&root).unwrap();
        let source_path = root.join("game.ezra");
        let parse_layout_path = root.join("parse.ezralayout");
        let invalid_layout_path = root.join("invalid.ezralayout");
        std::fs::write(&source_path, "fn main() { test.pass() }\n").unwrap();
        std::fs::write(
            &parse_layout_path,
            r#"
                layout broken {
                    load 0x010000;
            "#,
        )
        .unwrap();
        std::fs::write(
            &invalid_layout_path,
            r#"
                layout invalid {
                    load 0x010000;
                    entry 0x010040;
                    stack 0xF00000;

                    region code 0x010000..0x01FFFF read execute;
                    region overlap 0x018000..0x02FFFF read;
                    section .text -> code align 24;
                }
            "#,
        )
        .unwrap();

        let parse_prefix = format!("{}:1:1:", parse_layout_path.display());
        let parse_error = emit_assembly_with_command_options(&CommandOptions {
            path: source_path.to_string_lossy().into_owned(),
            debug_comments: false,
            default_sdk_symbols: true,
            layout_path: Some(parse_layout_path.to_string_lossy().into_owned()),
            target: None,
        })
        .unwrap_err();
        assert!(
            parse_error.starts_with(&parse_prefix),
            "expected `{parse_error}` to start with `{parse_prefix}`"
        );

        let invalid_prefix = format!("{}:1:1:", invalid_layout_path.display());
        let invalid_error = emit_assembly_with_command_options(&CommandOptions {
            path: source_path.to_string_lossy().into_owned(),
            debug_comments: false,
            default_sdk_symbols: true,
            layout_path: Some(invalid_layout_path.to_string_lossy().into_owned()),
            target: None,
        })
        .unwrap_err();
        assert!(
            invalid_error.contains(&invalid_prefix),
            "expected `{invalid_error}` to contain `{invalid_prefix}`"
        );
        assert!(
            invalid_error.contains("section `.text` alignment must be a power of two"),
            "{invalid_error}"
        );
        assert!(
            invalid_error.contains("regions `code` and `overlap` overlap"),
            "{invalid_error}"
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn commands_reject_custom_layouts_missing_required_sections() {
        let root = temp_root("layout_missing_required_sections");
        std::fs::create_dir_all(&root).unwrap();
        let source_path = root.join("game.ezra");
        let layout_path = root.join("game.ezralayout");
        std::fs::write(&source_path, "fn main() { test.pass() }\n").unwrap();
        std::fs::write(
            &layout_path,
            r#"
                layout incomplete {
                    load 0x020000;
                    entry 0x020040;
                    stack 0xEFFE00;

                    region code 0x020000..0x02FFFF read execute;
                    section .header -> code align 64;
                    section .text -> code align 16;
                }
            "#,
        )
        .unwrap();

        let error = check(&CommandOptions {
            path: source_path.to_string_lossy().into_owned(),
            debug_comments: false,
            default_sdk_symbols: true,
            layout_path: Some(layout_path.to_string_lossy().into_owned()),
            target: None,
        })
        .unwrap_err();
        let prefix = format!("layout is invalid:\n{}:1:1:", layout_path.display());
        assert!(
            error.starts_with(&prefix),
            "expected `{error}` to start with `{prefix}`"
        );
        for section in [".rodata", ".data", ".bss", ".assets", ".scratch"] {
            let diagnostic = format!("layout is missing required section `{section}`");
            assert!(
                error.contains(&diagnostic),
                "expected `{error}` to contain `{diagnostic}`"
            );
        }

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn zxspectrum_target_uses_spectrum_layout() {
        let root = temp_root("z80_default_layout");
        std::fs::create_dir_all(&root).unwrap();
        let source_path = root.join("game.ezra");
        std::fs::write(&source_path, "fn main() {}\n").unwrap();

        let settings = resolve_build_settings(
            &CommandOptions {
                path: source_path.to_string_lossy().into_owned(),
                debug_comments: false,
                default_sdk_symbols: true,
                layout_path: None,
                target: Some("zxspectrum-z80".to_owned()),
            },
            &source_path,
        )
        .unwrap();

        assert_eq!(settings.target.triple.cpu, CpuFamily::Z80);
        assert_eq!(settings.target.memory.pointer_width_bits, 16);
        assert_eq!(settings.target.memory.address_width_bits, 16);
        assert_eq!(settings.layout.name, "zx_spectrum_z80");
        assert_eq!(settings.layout.load.get(), 0x8000);
        assert_eq!(settings.layout.entry.get(), 0x8000);
        assert!(
            settings
                .layout
                .symbols
                .iter()
                .any(|symbol| symbol.name == "ZX_SCREEN_BASE" && symbol.value.get() == 0x4000)
        );
        assert!(
            settings
                .layout
                .regions
                .iter()
                .all(|region| region.end.get() <= 0xFFFF)
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn zxspectrum_source_build_uses_sdk_and_writes_loadable_tape() {
        let root = temp_root("zxspectrum_sdk_build");
        std::fs::create_dir_all(&root).unwrap();
        let source_path = root.join("game.ezra");
        std::fs::write(
            &source_path,
            r#"
                import zx.rom
                import zx.screen

                fn main() {
                    screen.border(2)
                    rom.print_char(65)
                }
            "#,
        )
        .unwrap();

        let outputs = build_source_with_command_options(&CommandOptions {
            path: source_path.to_string_lossy().into_owned(),
            debug_comments: false,
            default_sdk_symbols: false,
            layout_path: None,
            target: Some("zxspectrum-z80".to_owned()),
        })
        .unwrap();

        let asm = std::fs::read_to_string(outputs.asm).unwrap();
        let map = std::fs::read_to_string(outputs.map).unwrap();
        let tape = std::fs::read(&outputs.executable).unwrap();
        assert!(asm.contains("; target: Z80"), "{asm}");
        assert!(asm.contains("out (0FEh), a"), "{asm}");
        assert!(asm.contains("rst 10h"), "{asm}");
        assert!(map.contains(".text        0x008000"), "{map}");
        assert_eq!(
            outputs.executable.extension().and_then(|ext| ext.to_str()),
            Some("tap")
        );
        assert_eq!(u16::from_le_bytes([tape[0], tape[1]]), 19);
        assert_eq!(tape[2], 0x00);
        assert_eq!(tape[3], 3);
        assert!(u16::from_le_bytes([tape[14], tape[15]]) > 0);
        assert_eq!(u16::from_le_bytes([tape[16], tape[17]]), 0x8000);
        let data_block = 21;
        assert_eq!(tape[data_block + 2], 0xFF);
        assert_eq!(&tape[data_block + 3..data_block + 6], &[0xF3, 0x31, 0x00]);

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn spectrum_tap_preserves_a_custom_load_address() {
        let mut settings = resolve_build_settings(
            &CommandOptions {
                path: "game.ezra".to_owned(),
                debug_comments: false,
                default_sdk_symbols: true,
                layout_path: None,
                target: Some("zxspectrum-z80".to_owned()),
            },
            Path::new("game.ezra"),
        )
        .unwrap();
        settings.layout.load = Address24::new(0x8001);
        let tape = zx_spectrum_tap_bytes(&settings, None, &[0x00]).unwrap();
        assert_eq!(u16::from_le_bytes([tape[16], tape[17]]), 0x8001);
    }

    #[test]
    fn ti_ce_targets_use_tice_layout_and_sdk() {
        for (target, expected_layout) in [
            ("ti84plusce-ez80", "ti84plusce-ez80_layout"),
            ("ti83premiumce-ez80", "ti83premiumce-ez80_layout"),
        ] {
            let root = temp_root(target);
            std::fs::create_dir_all(&root).unwrap();
            let source_path = root.join("game.ezra");
            std::fs::write(
                &source_path,
                r#"
                    import tice.os
                    import tice.lcd

                    fn main() {
                        lcd.set_first_pixel(4)
                        os.idle()
                    }
                "#,
            )
            .unwrap();

            let outputs = build_source_with_command_options(&CommandOptions {
                path: source_path.to_string_lossy().into_owned(),
                debug_comments: false,
                default_sdk_symbols: false,
                layout_path: None,
                target: Some(target.to_owned()),
            })
            .unwrap();
            let settings = resolve_build_settings(
                &CommandOptions {
                    path: source_path.to_string_lossy().into_owned(),
                    debug_comments: false,
                    default_sdk_symbols: false,
                    layout_path: None,
                    target: Some(target.to_owned()),
                },
                &source_path,
            )
            .unwrap();
            let asm = std::fs::read_to_string(outputs.asm).unwrap();
            let map = std::fs::read_to_string(outputs.map).unwrap();
            let bin = std::fs::read(&outputs.executable).unwrap();

            assert_eq!(settings.layout.name, expected_layout);
            assert_eq!(settings.output_format, OutputFormat::Ti8xp);
            assert_eq!(settings.layout.entry.get(), 0xD1_A881);
            assert!(asm.contains("; target: eZ80 ADL mode"), "{asm}");
            assert!(asm.contains("ld (0D40000h), a"), "{asm}");
            assert!(map.contains(".text        0xD1A881"), "{map}");
            assert_eq!(
                outputs.executable.extension().and_then(|ext| ext.to_str()),
                Some("8xp")
            );
            assert_ti8xp(&bin, b"GAME\0\0\0\0", &[0xEF, 0x7B, 0xF3, 0x31]);

            let _ = std::fs::remove_dir_all(root);
        }
    }

    #[test]
    fn ti_ce_target_can_override_output_to_raw_bin() {
        let root = temp_root("ti_ce_bin_override");
        std::fs::create_dir_all(&root).unwrap();
        let source_path = root.join("game.ezra");
        std::fs::write(
            root.join("Ezra.toml"),
            r#"
                [build]
                target = "ti84plusce-ez80"
                output = "bin"
            "#,
        )
        .unwrap();
        std::fs::write(&source_path, "fn main() {}\n").unwrap();

        let outputs = build_source(source_path.to_str().unwrap()).unwrap();
        let bin = std::fs::read(&outputs.executable).unwrap();

        assert_eq!(
            outputs.executable.extension().and_then(|ext| ext.to_str()),
            Some("bin")
        );
        assert_eq!(&bin[0..4], &[0xF3, 0x31, 0xFF, 0xFF]);

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn ti_ce_target_can_emit_8ek_app_output() {
        let root = temp_root("ti_ce_8ek_output");
        std::fs::create_dir_all(&root).unwrap();
        let source_path = root.join("game.ezra");
        std::fs::write(
            root.join("Ezra.toml"),
            r#"
                [build]
                target = "ti84plusce-ez80"
                output = "8ek"
            "#,
        )
        .unwrap();
        std::fs::write(&source_path, "fn main() {}\n").unwrap();

        let outputs = build_source(source_path.to_str().unwrap()).unwrap();
        let app = std::fs::read(&outputs.executable).unwrap();

        assert_eq!(
            outputs.executable.extension().and_then(|ext| ext.to_str()),
            Some("8ek")
        );
        assert_ti_app(&app, b'E', b"GAME\0\0\0\0", 0xD1_A881, &[0xF3, 0x31]);

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn ti_z80_targets_use_ti_layout_and_sdk() {
        for (target, expected_layout) in [
            ("ti83-z80", "ti83-z80_layout"),
            ("ti83plus-z80", "ti83plus-z80_layout"),
            ("ti84-z80", "ti84-z80_layout"),
            ("ti84plus-z80", "ti84plus-z80_layout"),
        ] {
            let root = temp_root(target);
            std::fs::create_dir_all(&root).unwrap();
            let source_path = root.join("game.ezra");
            std::fs::write(
                &source_path,
                r#"
                    import ti.os
                    import ti.lcd

                    fn main() {
                        lcd.set_first_byte(4)
                        os.idle()
                    }
                "#,
            )
            .unwrap();

            let outputs = build_source_with_command_options(&CommandOptions {
                path: source_path.to_string_lossy().into_owned(),
                debug_comments: false,
                default_sdk_symbols: false,
                layout_path: None,
                target: Some(target.to_owned()),
            })
            .unwrap();
            let settings = resolve_build_settings(
                &CommandOptions {
                    path: source_path.to_string_lossy().into_owned(),
                    debug_comments: false,
                    default_sdk_symbols: false,
                    layout_path: None,
                    target: Some(target.to_owned()),
                },
                &source_path,
            )
            .unwrap();
            let asm = std::fs::read_to_string(outputs.asm).unwrap();
            let map = std::fs::read_to_string(outputs.map).unwrap();
            let bin = std::fs::read(&outputs.executable).unwrap();

            assert_eq!(settings.target.triple.cpu, CpuFamily::Z80);
            assert_eq!(settings.output_format, OutputFormat::Ti8xp);
            assert_eq!(settings.layout.name, expected_layout);
            assert_eq!(settings.layout.entry.get(), 0x9D95);
            assert!(asm.contains("; target: Z80"), "{asm}");
            assert!(asm.contains("ld (9340h), a"), "{asm}");
            assert!(map.contains(".text        0x009D95"), "{map}");
            assert_eq!(
                outputs.executable.extension().and_then(|ext| ext.to_str()),
                Some("8xp")
            );
            assert_ti8xp(&bin, b"GAME\0\0\0\0", &[0xBB, 0x6D, 0xF3, 0x31]);

            let _ = std::fs::remove_dir_all(root);
        }
    }

    #[test]
    fn ti_z80_target_can_override_output_to_raw_bin() {
        let root = temp_root("ti_z80_bin_override");
        std::fs::create_dir_all(&root).unwrap();
        let source_path = root.join("game.ezra");
        std::fs::write(
            root.join("Ezra.toml"),
            r#"
                [build]
                target = "ti84plus-z80"
                output = "bin"
            "#,
        )
        .unwrap();
        std::fs::write(&source_path, "fn main() {}\n").unwrap();

        let outputs = build_source(source_path.to_str().unwrap()).unwrap();
        let bin = std::fs::read(&outputs.executable).unwrap();

        assert_eq!(
            outputs.executable.extension().and_then(|ext| ext.to_str()),
            Some("bin")
        );
        assert_eq!(&bin[0..3], &[0xF3, 0x31, 0x00]);

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn ti_z80_target_can_emit_8xk_app_output() {
        let root = temp_root("ti_z80_8xk_output");
        std::fs::create_dir_all(&root).unwrap();
        let source_path = root.join("game.ezra");
        std::fs::write(
            root.join("Ezra.toml"),
            r#"
                [build]
                target = "ti84plus-z80"
                output = "8xk"
            "#,
        )
        .unwrap();
        std::fs::write(&source_path, "fn main() {}\n").unwrap();

        let outputs = build_source(source_path.to_str().unwrap()).unwrap();
        let app = std::fs::read(&outputs.executable).unwrap();

        assert_eq!(
            outputs.executable.extension().and_then(|ext| ext.to_str()),
            Some("8xk")
        );
        assert_ti_app(&app, b'X', b"GAME\0\0\0\0", 0x00009D95, &[0xF3, 0x31]);

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn cpm_z80_target_uses_com_layout() {
        let root = temp_root("cpm_z80_layout");
        std::fs::create_dir_all(&root).unwrap();
        let source_path = root.join("game.ezra");
        std::fs::write(&source_path, "fn main() {}\n").unwrap();

        let settings = resolve_build_settings(
            &CommandOptions {
                path: source_path.to_string_lossy().into_owned(),
                debug_comments: false,
                default_sdk_symbols: true,
                layout_path: None,
                target: Some("cpm-2.2-z80".to_owned()),
            },
            &source_path,
        )
        .unwrap();

        assert_eq!(settings.target.output_format, OutputFormat::CpmCom);
        assert_eq!(settings.layout.name, "cpm_z80_com");
        assert_eq!(settings.layout.load.get(), 0x0100);
        assert_eq!(settings.layout.entry.get(), 0x0100);

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn ez80_flat_harness_target_runs_and_captures_output() {
        let root = temp_root("ez80_flat_harness");
        std::fs::create_dir_all(&root).unwrap();
        let source_path = root.join("game.ezra");
        std::fs::write(
            &source_path,
            r#"
                fn main() {
                    debug.char('O')
                    debug.char('K')
                    test.pass()
                }
            "#,
        )
        .unwrap();

        let run = run_source_with_command_options(&CommandOptions {
            path: source_path.to_string_lossy().into_owned(),
            debug_comments: false,
            default_sdk_symbols: true,
            layout_path: None,
            target: Some("ezra-test-flat-ez80".to_owned()),
        })
        .unwrap();

        assert!(run.halted, "{run:?}");
        assert_eq!(run.result_code, 0);
        assert_eq!(run.debug_output, b"OK");

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn ez80_split_harness_target_uses_split_layout_and_memory() {
        let root = temp_root("ez80_split_harness");
        std::fs::create_dir_all(&root).unwrap();
        let source_path = root.join("game.ezra");
        std::fs::write(
            &source_path,
            r#"
                global marker: u8 = 0x42

                fn main() {
                    marker = marker + 1
                    test.assert_eq_u24(EZRA_RAM_BASE, 0x100000, 1)
                    test.assert_eq_u24(EZRA_STACK_TOP, 0x1FFF00, 2)
                    test.assert_eq_u8(marker, 0x43, 3)
                    test.pass()
                }
            "#,
        )
        .unwrap();

        test_source_with_command_options(&CommandOptions {
            path: source_path.to_string_lossy().into_owned(),
            debug_comments: false,
            default_sdk_symbols: true,
            layout_path: None,
            target: Some("ezra-test-split-ez80".to_owned()),
        })
        .unwrap();

        let outputs = build_source_with_command_options(&CommandOptions {
            path: source_path.to_string_lossy().into_owned(),
            debug_comments: false,
            default_sdk_symbols: true,
            layout_path: None,
            target: Some("ezra-test-split-ez80".to_owned()),
        })
        .unwrap();
        let map = std::fs::read_to_string(outputs.map).unwrap();
        assert!(map.contains(".text        0x020040"), "{map}");
        assert!(
            map.contains(".data        0x100000 0x100000 0x000001"),
            "{map}"
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn ez80_harness_target_reports_execution_traps() {
        let root = temp_root("ez80_harness_trap");
        std::fs::create_dir_all(&root).unwrap();
        let source_path = root.join("game.ezra");
        std::fs::write(
            &source_path,
            r#"
                naked fn main() {
                    asm volatile {
                        "jp 030000h"
                    }
                }
            "#,
        )
        .unwrap();

        let error = test_source_with_command_options(&CommandOptions {
            path: source_path.to_string_lossy().into_owned(),
            debug_comments: false,
            default_sdk_symbols: true,
            layout_path: None,
            target: Some("ezra-test-flat-ez80".to_owned()),
        })
        .unwrap_err();

        assert!(
            error.contains("test executed outside mapped memory at 0x030000"),
            "{error}"
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn ez80_harness_project_config_writes_target_artifacts() {
        let root = temp_root("ez80_harness_project_artifacts");
        std::fs::create_dir_all(root.join("src")).unwrap();
        let source_path = root.join("src/game.ezra");
        std::fs::write(
            root.join("Ezra.toml"),
            r#"
                [build]
                target = "ezra-test-flat-ez80"
                executable = "harness-game"
            "#,
        )
        .unwrap();
        std::fs::write(
            &source_path,
            r#"
                global marker: u8 = 0x5A
                fn main() {
                    test.assert_eq_u8(marker, 0x5A, 1)
                    test.pass()
                }
            "#,
        )
        .unwrap();

        let outputs = build_source(source_path.to_str().unwrap()).unwrap();
        let expected_base = root
            .join("target")
            .join("ezra-test-flat-ez80")
            .join("src")
            .join("harness-game");

        assert_eq!(outputs.asm, expected_base.with_extension("asm"));
        assert_eq!(outputs.map, expected_base.with_extension("map"));
        assert_eq!(outputs.executable, expected_base.with_extension("bin"));
        let map = std::fs::read_to_string(outputs.map).unwrap();
        assert!(map.contains(".text        0x010040"), "{map}");
        assert!(map.contains(".data        0x050000"), "{map}");

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn ez80_harness_preserves_port_output_ordering() {
        let root = temp_root("ez80_port_order");
        std::fs::create_dir_all(&root).unwrap();
        let source_path = root.join("game.ezra");
        std::fs::write(
            &source_path,
            r#"
                port DEBUG: u8 = 0x0C

                fn main() {
                    out DEBUG, 65
                    out DEBUG, 66
                    out DEBUG, 67
                    test.pass()
                }
            "#,
        )
        .unwrap();

        let run = run_source_with_command_options(&CommandOptions {
            path: source_path.to_string_lossy().into_owned(),
            debug_comments: false,
            default_sdk_symbols: true,
            layout_path: None,
            target: Some("ezra-test-flat-ez80".to_owned()),
        })
        .unwrap();

        assert_eq!(run.debug_output, b"ABC");
        assert_eq!(run.result_code, 0);

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn ez80_harness_preserves_inline_asm_memory_clobber_barrier() {
        let root = temp_root("ez80_asm_memory_barrier");
        std::fs::create_dir_all(&root).unwrap();
        let source_path = root.join("game.ezra");
        std::fs::write(
            &source_path,
            r#"
                global value: u8 = 1

                fn main() {
                    let before: u8 = value
                    asm volatile(clobber memory, clobber a) {
                        "ld a, 02h"
                        "ld (050000h), a"
                    }
                    let after: u8 = value
                    test.assert_eq_u8(before, 1, 1)
                    test.assert_eq_u8(after, 2, 2)
                    test.pass()
                }
            "#,
        )
        .unwrap();

        test_source_with_command_options(&CommandOptions {
            path: source_path.to_string_lossy().into_owned(),
            debug_comments: false,
            default_sdk_symbols: true,
            layout_path: None,
            target: Some("ezra-test-flat-ez80".to_owned()),
        })
        .unwrap();

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn ez80_harness_preserves_volatile_memory_ordering() {
        let root = temp_root("ez80_volatile_order");
        std::fs::create_dir_all(&root).unwrap();
        let source_path = root.join("game.ezra");
        std::fs::write(
            &source_path,
            r#"
                volatile mmio DEVICE: ptr<u8> = 0x050020

                fn main() {
                    *DEVICE = 1
                    *DEVICE = *DEVICE + 1
                    test.assert_eq_u8(*DEVICE, 2, 1)
                    test.pass()
                }
            "#,
        )
        .unwrap();

        test_source_with_command_options(&CommandOptions {
            path: source_path.to_string_lossy().into_owned(),
            debug_comments: false,
            default_sdk_symbols: true,
            layout_path: None,
            target: Some("ezra-test-flat-ez80".to_owned()),
        })
        .unwrap();

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn ez80_flat_harness_runs_complex_sdk_fixture_and_raw_artifacts() {
        let root = temp_root("flat_complex_fixture");
        std::fs::create_dir_all(&root).unwrap();
        let source_path = copy_fixture(&root, "flat_complex.ezra");

        let options = CommandOptions {
            path: source_path.to_string_lossy().into_owned(),
            debug_comments: false,
            default_sdk_symbols: true,
            layout_path: None,
            target: Some("ezra-test-flat-ez80".to_owned()),
        };
        let run = run_source_with_command_options(&options).unwrap();
        assert_eq!(run.result_code, 0, "{run:?}");
        assert_eq!(run.debug_output, b"FLAT");
        assert_eq!(run.ports[0x0D], 0);
        assert_eq!(run.ports[0x0E], 1);

        let outputs = build_source_with_command_options(&options).unwrap();
        assert_eq!(outputs.executable.extension().unwrap(), "bin");
        let executable = std::fs::read(&outputs.executable).unwrap();
        assert!(!executable.starts_with(b"MOS"), "{executable:02X?}");
        let map = std::fs::read_to_string(outputs.map).unwrap();
        assert!(map.contains(".text        0x010040"), "{map}");
        assert!(map.contains(".data        0x050000"), "{map}");
        assert!(map.contains(".assets      0x0C0000"), "{map}");
        assert!(map.contains("banner"), "{map}");

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn ez80_split_harness_runs_complex_sdk_fixture_and_split_artifacts() {
        let root = temp_root("split_complex_fixture");
        std::fs::create_dir_all(&root).unwrap();
        let source_path = copy_fixture(&root, "split_complex.ezra");

        let options = CommandOptions {
            path: source_path.to_string_lossy().into_owned(),
            debug_comments: false,
            default_sdk_symbols: true,
            layout_path: None,
            target: Some("ezra-test-split-ez80".to_owned()),
        };
        let run = run_source_with_command_options(&options).unwrap();
        assert_eq!(run.result_code, 0, "{run:?}");
        assert_eq!(run.debug_output, b"SPLIT");

        let outputs = build_source_with_command_options(&options).unwrap();
        assert_eq!(outputs.executable.extension().unwrap(), "bin");
        let map = std::fs::read_to_string(outputs.map).unwrap();
        assert!(map.contains(".text        0x020040"), "{map}");
        assert!(map.contains(".data        0x100000"), "{map}");
        assert!(map.contains(".assets      0x180000"), "{map}");
        assert!(map.contains("palette"), "{map}");

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn cpm_z80_harness_runs_complex_assembly_fixture_and_com_format() {
        let root = temp_root("cpm_complex_fixture");
        std::fs::create_dir_all(&root).unwrap();
        let source_path = copy_fixture(&root, "z80_cpm_complex.asm");
        let output_path = root.join("z80_cpm_complex.com");

        assemble_file(&AssembleOptions {
            path: source_path.to_string_lossy().into_owned(),
            output: Some(output_path.to_string_lossy().into_owned()),
            base_addr: Some(0x0100),
            assembler_cpu: None,
            layout_path: None,
            map_path: None,
            target: Some("cpm-2.2-z80".to_owned()),
        })
        .unwrap();
        let bytes = std::fs::read(&output_path).unwrap();
        assert_eq!(output_path.extension().unwrap(), "com");
        assert!(bytes.len() > 12, "{bytes:02X?}");

        let assembly = std::fs::read_to_string(&source_path).unwrap();
        let run = ezra::vm::run_assembly_test_with_cpu_options_at(
            CpuFamily::Z80,
            &assembly,
            &TestRunOptions {
                instruction_budget: 4_000,
                initial_ports: Vec::new(),
                initial_memory: Vec::new(),
                stack_top: 0xFF00,
            },
            0x0100,
        )
        .unwrap();
        assert!(run.halted, "{run:?}");
        assert_eq!(run.debug_output, b"Z80");

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn z80_source_check_accepts_16_bit_cfg() {
        let root = temp_root("z80_source_check");
        std::fs::create_dir_all(&root).unwrap();
        let source_path = root.join("game.ezra");
        std::fs::write(
            &source_path,
            r#"
                @cfg(pointer_width(16))
                fn main() {}
            "#,
        )
        .unwrap();

        check(&CommandOptions {
            path: source_path.to_string_lossy().into_owned(),
            debug_comments: false,
            default_sdk_symbols: true,
            layout_path: None,
            target: Some("zxspectrum-z80".to_owned()),
        })
        .unwrap();

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn z80_target_rejects_layout_addresses_above_16_bit_space() {
        let root = temp_root("z80_layout_diagnostic");
        std::fs::create_dir_all(&root).unwrap();
        let source_path = root.join("game.ezra");
        let layout_path = root.join("game.ezralayout");
        std::fs::write(
            &source_path,
            r#"
                @cfg(cpu("z80"))
                fn main() {}
            "#,
        )
        .unwrap();
        std::fs::write(
            &layout_path,
            r#"
                layout too_large {
                    load 0x010000;
                    entry 0x010040;
                    stack 0x01FF00;

                    region header 0x010000..0x01003F read;
                    region code 0x010040..0x017FFF read execute;
                    region rodata 0x018000..0x019FFF read;
                    region ram 0x01A000..0x01BFFF read write;
                    region assets 0x01C000..0x01DFFF read;
                    region scratch 0x01E000..0x01EFFF read write;
                    region stack 0x01F000..0x01FFFF read write reserved;

                    section .header -> header align 1;
                    section .text -> code align 16;
                    section .rodata -> rodata align 16;
                    section .data -> ram align 16;
                    section .bss -> ram align 16;
                    section .assets -> assets align 256;
                    section .scratch -> scratch align 16;
                }
            "#,
        )
        .unwrap();

        let error = check(&CommandOptions {
            path: source_path.to_string_lossy().into_owned(),
            debug_comments: false,
            default_sdk_symbols: true,
            layout_path: Some(layout_path.to_string_lossy().into_owned()),
            target: Some("zxspectrum-z80".to_owned()),
        })
        .unwrap_err();

        assert!(
            error.contains("requires addresses outside the 16-bit address space"),
            "{error}"
        );
        assert!(error.contains("load address 0x010000"), "{error}");

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn parses_test_port_metadata() {
        let metadata = parse_test_metadata(
            r#"
                // port 0x01 = 0x10
                // test: port 2 = 0b00100000
                // mem 0x040123 = 0x6C
                // test: mem 262436 = 0b01101101
                fn main() { test.pass() }
            "#,
        )
        .unwrap();

        assert_eq!(metadata.initial_ports, vec![(0x01, 0x10), (0x02, 0x20)]);
        assert_eq!(
            metadata.initial_memory,
            vec![(0x040123, 0x6C), (0x040124, 0x6D)]
        );

        let error = parse_test_metadata("// port 0x100 = 0").unwrap_err();
        assert!(error.contains("outside u8 range"), "{error}");

        let error = parse_test_metadata("// mem 0x1000000 = 0").unwrap_err();
        assert!(error.contains("outside u24 range"), "{error}");
    }

    #[test]
    fn test_command_uses_port_metadata() {
        let root = temp_root("test_metadata");
        std::fs::create_dir_all(&root).unwrap();
        let source_path = root.join("game.ezra");
        std::fs::write(
            &source_path,
            r#"
                // port 0x01 = 0x10
                port PAD: u8 = 0x01
                fn main() {
                    let pad: u8 = in PAD
                    test.assert_eq_u8(pad, 0x10, 1)
                    test.pass()
                }
            "#,
        )
        .unwrap();

        test_source(source_path.to_str().unwrap()).unwrap();

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn commands_can_disable_default_sdk_symbols() {
        let root = temp_root("strict_sdk_symbols");
        std::fs::create_dir_all(&root).unwrap();
        let default_port_path = root.join("default_port.ezra");
        std::fs::write(
            &default_port_path,
            r#"
                fn main() {
                    let pad: u8 = in PAD1_LO
                    test.assert_eq_u8(pad, 0, 1)
                    test.pass()
                }
            "#,
        )
        .unwrap();

        let error = check(&CommandOptions {
            path: default_port_path.to_string_lossy().into_owned(),
            debug_comments: false,
            default_sdk_symbols: false,
            layout_path: None,
            target: None,
        })
        .unwrap_err();
        assert!(error.contains("unknown port `PAD1_LO`"), "{error}");

        let explicit_port_path = root.join("explicit_port.ezra");
        std::fs::write(
            &explicit_port_path,
            r#"
                // port 0x9B = 0x42
                port AGON_VDP: u8 = 0x9B

                fn main() {
                    let value: u8 = in AGON_VDP
                    test.assert_eq_u8(value, 0x42, 1)
                    test.pass()
                }
            "#,
        )
        .unwrap();

        test_source_with_command_options(&CommandOptions {
            path: explicit_port_path.to_string_lossy().into_owned(),
            debug_comments: false,
            default_sdk_symbols: false,
            layout_path: None,
            target: None,
        })
        .unwrap();

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn test_command_uses_memory_metadata() {
        let root = temp_root("test_memory_metadata");
        std::fs::create_dir_all(&root).unwrap();
        let source_path = root.join("game.ezra");
        std::fs::write(
            &source_path,
            r#"
                // mem 0x040123 = 0x6C
                fn main() {
                    let byte: ptr<u8> = cast<ptr<u8>>(0x040123)
                    test.assert_eq_u8(*byte, 0x6C, 1)
                    test.pass()
                }
            "#,
        )
        .unwrap();

        test_source(source_path.to_str().unwrap()).unwrap();

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn test_command_can_use_custom_layout_file() {
        let root = temp_root("custom_layout_test");
        std::fs::create_dir_all(&root).unwrap();
        let source_path = root.join("game.ezra");
        let layout_path = root.join("game.ezralayout");
        std::fs::write(
            &source_path,
            r#"
                embed banked: bytes = bytes [0xA1, 0xA2] section .bank1 align 256
                embed banked2: bytes = bytes [0xB1] section .bank2 align 256
                global marker: u8 = 0x42

                fn main() {
                    test.assert_eq_u8(marker, 0x42, 1)
                    test.assert_eq_u24(EZRA_RAM_BASE, 0x030000, 2)
                    test.assert_eq_u24(EZRA_VRAM_BASE, 0x090000, 3)
                    test.assert_eq_u24(EZRA_CODE_BASE, 0x020040, 4)
                    test.assert_eq_u24(cast<ptr24>(banked.ptr), 0x120000, 5)
                    test.assert_eq_u8(*(banked.ptr + 1), 0xA2, 6)
                    test.assert_eq_u24(cast<ptr24>(banked2.ptr), 0x120100, 7)
                    test.assert_eq_u8(*(banked2.ptr), 0xB1, 8)
                    test.pass()
                }
            "#,
        )
        .unwrap();
        std::fs::write(
            &layout_path,
            r#"
                layout custom_test {
                    load 0x020000;
                    entry 0x020040;
                    stack 0xEFFE80;

                    region code 0x020000..0x02FFFF read execute;
                    region ram 0x030000..0x03FFFF read write;
                    region rodata 0x040000..0x04FFFF read;
                    region bank 0x120000..0x12FFFF read;
                    region scratch 0xE00000..0xE0FFFF read write;
                    section .header -> code align 64;
                    section .text -> code align 16;
                    section .rodata -> rodata align 16;
                    section .data -> ram align 16;
                    section .bss -> ram align 16;
                    section .assets -> bank align 256;
                    section .scratch -> scratch align 16;
                    section .bank1 -> bank align 256;
                    section .bank2 -> bank align 256;

                    symbol EZRA_RAM_BASE = 0x030000;
                    symbol EZRA_VRAM_BASE = 0x090000;
                }
            "#,
        )
        .unwrap();

        test_source_with_command_options(&CommandOptions {
            path: source_path.to_string_lossy().into_owned(),
            debug_comments: false,
            default_sdk_symbols: true,
            layout_path: Some(layout_path.to_string_lossy().into_owned()),
            target: None,
        })
        .unwrap();

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn check_command_can_use_custom_layout_file() {
        let root = temp_root("custom_layout_check");
        std::fs::create_dir_all(&root).unwrap();
        let source_path = root.join("game.ezra");
        let layout_path = root.join("game.ezralayout");
        std::fs::write(
            &source_path,
            r#"
                fn main() {
                    test.assert_eq_u24(EZRA_RAM_BASE, 0x030000, 1)
                    test.assert_eq_u24(EZRA_STACK_TOP, 0xEFFE00, 2)
                    test.pass()
                }
            "#,
        )
        .unwrap();
        std::fs::write(
            &layout_path,
            r#"
                layout check_custom {
                    load 0x020000;
                    entry 0x020040;
                    stack 0xEFFE00;

                    region code 0x020000..0x02FFFF read execute;
                    region ram 0x030000..0x03FFFF read write;
                    region rodata 0x040000..0x04FFFF read;
                    region assets 0x120000..0x12FFFF read;
                    region scratch 0xE00000..0xE0FFFF read write;
                    section .header -> code align 64;
                    section .text -> code align 16;
                    section .rodata -> rodata align 16;
                    section .data -> ram align 16;
                    section .bss -> ram align 16;
                    section .assets -> assets align 256;
                    section .scratch -> scratch align 16;

                    symbol EZRA_RAM_BASE = 0x030000;
                }
            "#,
        )
        .unwrap();

        check(&CommandOptions {
            path: source_path.to_string_lossy().into_owned(),
            debug_comments: false,
            default_sdk_symbols: true,
            layout_path: Some(layout_path.to_string_lossy().into_owned()),
            target: None,
        })
        .unwrap();

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn commands_use_ezra_toml_target_and_layout() {
        let root = temp_root("project_config_layout");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("layouts")).unwrap();
        let source_path = root.join("src/game.ezra");
        let layout_path = root.join("layouts/agon.ezralayout");
        std::fs::write(
            root.join("Ezra.toml"),
            r#"
                [build]
                target = "agonlight-console8-ez80-1.0"

                [layout]
                file = "layouts/agon.ezralayout"
            "#,
        )
        .unwrap();
        std::fs::write(
            &source_path,
            r#"
                fn main() {
                    test.assert_eq_u24(EZRA_RAM_BASE, 0x030000, 1)
                    test.pass()
                }
            "#,
        )
        .unwrap();
        std::fs::write(
            &layout_path,
            r#"
                layout project_layout {
                    load 0x020000;
                    entry 0x020040;
                    stack 0xEFFE00;

                    region code 0x020000..0x02FFFF read execute;
                    region ram 0x030000..0x03FFFF read write;
                    region rodata 0x040000..0x04FFFF read;
                    region assets 0x120000..0x12FFFF read;
                    region scratch 0xE00000..0xE0FFFF read write;
                    section .header -> code align 64;
                    section .text -> code align 16;
                    section .rodata -> rodata align 16;
                    section .data -> ram align 16;
                    section .bss -> ram align 16;
                    section .assets -> assets align 256;
                    section .scratch -> scratch align 16;

                    symbol EZRA_RAM_BASE = 0x030000;
                }
            "#,
        )
        .unwrap();

        check(&CommandOptions {
            path: source_path.to_string_lossy().into_owned(),
            debug_comments: false,
            default_sdk_symbols: true,
            layout_path: None,
            target: None,
        })
        .unwrap();

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn agon_mos_target_uses_builtin_sdk_and_layout() {
        let root = temp_root("agon_builtin_sdk");
        std::fs::create_dir_all(root.join("src")).unwrap();
        let source_path = root.join("src/main.ezra");
        std::fs::write(
            root.join("Ezra.toml"),
            r#"
                [build]
                target = "agonlight-mos-ez80"
            "#,
        )
        .unwrap();
        std::fs::write(
            &source_path,
            r#"
                import agon.vdp

                fn main() {
                    vdp.clear_screen()
                    vdp.vdu(65)
                    vdp.vdp_exit_emulator(0)
                }
            "#,
        )
        .unwrap();

        let outputs = build_source(source_path.to_str().unwrap()).unwrap();
        let map = std::fs::read_to_string(&outputs.map).unwrap();
        let asm = std::fs::read_to_string(&outputs.asm).unwrap();
        let bin = std::fs::read(&outputs.executable).unwrap();

        assert!(map.contains(".text        0x040045"), "{map}");
        assert!(asm.contains("rst.lis 10h"), "{asm}");
        assert!(asm.contains("out0 (00h), a"), "{asm}");
        assert_eq!(&bin[0..4], &[0xC3, 0x45, 0x00, 0x04]);
        assert_eq!(&bin[64..69], b"MOS\0\x01");

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn agon_mos_target_uses_expanded_builtin_sdk_modules() {
        let root = temp_root("agon_expanded_sdk");
        std::fs::create_dir_all(root.join("src")).unwrap();
        let source_path = root.join("src/main.ezra");
        std::fs::write(
            root.join("Ezra.toml"),
            r#"
                [build]
                target = "agonlight-mos-ez80"
                executable = "expanded-sdk"
            "#,
        )
        .unwrap();
        std::fs::write(
            &source_path,
            r#"
                import agon.console
                import agon.gpio
                import agon.keyboard
                import agon.mouse
                import agon.vdp

                fn main() {
                    console.color(vdp.COLOR_GREEN)
                    console.background(vdp.COLOR_BLACK)
                    console.print_line("SDK")
                    vdp.line(0, 0, 16, 16)
                    mouse.enable()
                    let key: u8 = keyboard.ascii()
                    gpio.set_port_b_direction(gpio.ALL_OUTPUTS)
                    gpio.write_port_b(key)
                }
            "#,
        )
        .unwrap();

        let outputs = build_source(source_path.to_str().unwrap()).unwrap();
        let asm = std::fs::read_to_string(&outputs.asm).unwrap();
        let bin = std::fs::read(&outputs.executable).unwrap();

        assert!(asm.contains("rst.lis 08h"), "{asm}");
        assert!(asm.contains("rst.lis 10h"), "{asm}");
        assert!(asm.contains("out0 (9Ah), a"), "{asm}");
        assert_eq!(&bin[64..69], b"MOS\0\x01");

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn commands_run_z80_source_on_emulator() {
        let root = temp_root("z80_source_test");
        std::fs::create_dir_all(&root).unwrap();
        let source_path = root.join("game.ezra");
        std::fs::write(
            &source_path,
            r#"
                fn main() {
                    let i: u8 = 0
                    let sum: u8 = 0
                    while i < 5 {
                        sum += i
                        i += 1
                    }
                    test.assert_eq_u8(sum, 10, 1)
                    test.pass()
                }
            "#,
        )
        .unwrap();

        test_source_with_command_options(&CommandOptions {
            path: source_path.to_string_lossy().into_owned(),
            debug_comments: false,
            default_sdk_symbols: true,
            layout_path: None,
            target: Some("zxspectrum-z80".to_owned()),
        })
        .unwrap();

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn z80_source_rejects_24bit_literals_before_assembly() {
        let root = temp_root("z80_source_u24_literal");
        std::fs::create_dir_all(&root).unwrap();
        let source_path = root.join("game.ezra");
        std::fs::write(
            &source_path,
            r#"
                fn main() {
                    let value: u24 = 0x010000
                    test.pass()
                }
            "#,
        )
        .unwrap();

        let options = CommandOptions {
            path: source_path.to_string_lossy().into_owned(),
            debug_comments: false,
            default_sdk_symbols: true,
            layout_path: None,
            target: Some("zxspectrum-z80".to_owned()),
        };
        let error = test_source_with_command_options(&options).unwrap_err();

        assert!(
            error.contains("24-bit value 0x010000 cannot be encoded for 16-bit target `z80`"),
            "{error}"
        );
        assert!(!error.contains("<assembly>"), "{error}");

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn z80_source_emits_z80_assembly_without_ez80_adl_forms() {
        let root = temp_root("z80_source_asm");
        std::fs::create_dir_all(&root).unwrap();
        let source_path = root.join("game.ezra");
        std::fs::write(
            &source_path,
            r#"
                fn main() {
                    let a: u16 = 12
                    let b: u16 = 13
                    let c: u16 = a * b
                    test.assert_eq_u16(c, 156, 2)
                    test.pass()
                }
            "#,
        )
        .unwrap();

        let asm = emit_assembly_with_command_options(&CommandOptions {
            path: source_path.to_string_lossy().into_owned(),
            debug_comments: false,
            default_sdk_symbols: true,
            layout_path: None,
            target: Some("zxspectrum-z80".to_owned()),
        })
        .unwrap();

        assert!(asm.contains("; target: Z80"), "{asm}");
        assert!(asm.contains("ld sp, 5B00h"), "{asm}");
        assert!(asm.contains("out (0Dh), a"), "{asm}");
        assert!(!asm.contains("out0"), "{asm}");
        assert!(!asm.contains("rst.lis"), "{asm}");
        assert!(!asm.contains("mlt"), "{asm}");

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn cpm_z80_source_build_writes_com_binary() {
        let root = temp_root("cpm_source_build");
        std::fs::create_dir_all(&root).unwrap();
        let source_path = root.join("game.ezra");
        std::fs::write(&source_path, "fn main() { test.pass() }\n").unwrap();

        let outputs = build_source_with_command_options(&CommandOptions {
            path: source_path.to_string_lossy().into_owned(),
            debug_comments: false,
            default_sdk_symbols: true,
            layout_path: None,
            target: Some("cpm-2.2-z80".to_owned()),
        })
        .unwrap();

        let asm = std::fs::read_to_string(outputs.asm).unwrap();
        let com = std::fs::read(&outputs.executable).unwrap();
        assert_eq!(
            outputs.executable.extension().and_then(|ext| ext.to_str()),
            Some("com")
        );
        assert!(asm.contains("; target: Z80"), "{asm}");
        assert_eq!(&com[0..3], &[0xF3, 0x31, 0x00]);

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn cpm_z80_source_example_uses_console_sdk_and_writes_com_binary() {
        let root = temp_root("cpm_source_example");
        std::fs::create_dir_all(&root).unwrap();
        let source = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/cpm-z80/hello-source.ezra"),
        )
        .unwrap();
        let source_path = root.join("hello-source.ezra");
        std::fs::write(&source_path, source).unwrap();

        let outputs = build_source_with_command_options(&CommandOptions {
            path: source_path.to_string_lossy().into_owned(),
            debug_comments: false,
            default_sdk_symbols: true,
            layout_path: None,
            target: Some("cpm-2.2-z80".to_owned()),
        })
        .unwrap();

        let asm = std::fs::read_to_string(outputs.asm).unwrap();
        let com = std::fs::read(&outputs.executable).unwrap();
        assert_eq!(
            outputs.executable.extension().and_then(|ext| ext.to_str()),
            Some("com")
        );
        assert!(asm.contains("; target: Z80"), "{asm}");
        assert!(asm.contains("    call 0005h"), "{asm}");
        assert!(!asm.contains("ld de, hl"), "{asm}");
        assert_eq!(&com[0..3], &[0xF3, 0x31, 0x00]);

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn cpm_z80_fcb_source_example_builds() {
        let root = temp_root("cpm_fcb_source_example");
        std::fs::create_dir_all(&root).unwrap();
        let source = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/cpm-z80/file-control.ezra"),
        )
        .unwrap();
        let source_path = root.join("file-control.ezra");
        std::fs::write(&source_path, source).unwrap();

        let outputs = build_source_with_command_options(&CommandOptions {
            path: source_path.to_string_lossy().into_owned(),
            debug_comments: false,
            default_sdk_symbols: true,
            layout_path: None,
            target: Some("cpm-2.2-z80".to_owned()),
        })
        .unwrap();

        let asm = std::fs::read_to_string(outputs.asm).unwrap();
        assert_eq!(
            outputs.executable.extension().and_then(|ext| ext.to_str()),
            Some("com")
        );
        assert!(asm.contains("; target: Z80"), "{asm}");
        assert!(asm.contains("    call 0005h"), "{asm}");

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn cpm_8080_source_build_uses_sdk_and_writes_com_binary() {
        let root = temp_root("cpm_8080_source_build");
        std::fs::create_dir_all(&root).unwrap();
        let source_path = root.join("game.ezra");
        std::fs::write(
            &source_path,
            r#"
                import cpm.bdos

                fn main() {
                    bdos.console_output(65)
                    bdos.exit()
                }
            "#,
        )
        .unwrap();

        let outputs = build_source_with_command_options(&CommandOptions {
            path: source_path.to_string_lossy().into_owned(),
            debug_comments: false,
            default_sdk_symbols: false,
            layout_path: None,
            target: Some("cpm-2.2-i8080".to_owned()),
        })
        .unwrap();

        let asm = std::fs::read_to_string(outputs.asm).unwrap();
        let com = std::fs::read(&outputs.executable).unwrap();
        assert_eq!(
            outputs.executable.extension().and_then(|ext| ext.to_str()),
            Some("com")
        );
        assert!(asm.contains("; target: i8080"), "{asm}");
        assert!(asm.contains("    call 0005h"), "{asm}");
        assert!(asm.contains("    mov c,"), "{asm}");
        assert!(!asm.contains("    ld "), "{asm}");
        assert_eq!(&com[0..3], &[0xF3, 0x31, 0x00]);

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn cpm_8085_source_build_uses_sdk_and_writes_com_binary() {
        let root = temp_root("cpm_8085_source_build");
        std::fs::create_dir_all(&root).unwrap();
        let source_path = root.join("game.ezra");
        std::fs::write(
            &source_path,
            r#"
                import cpm.bdos

                fn main() {
                    asm volatile {
                        "rim"
                        "sim"
                    }
                    bdos.console_output(65)
                    bdos.exit()
                }
            "#,
        )
        .unwrap();

        let outputs = build_source_with_command_options(&CommandOptions {
            path: source_path.to_string_lossy().into_owned(),
            debug_comments: false,
            default_sdk_symbols: false,
            layout_path: None,
            target: Some("cpm-2.2-i8085".to_owned()),
        })
        .unwrap();

        let asm = std::fs::read_to_string(outputs.asm).unwrap();
        let com = std::fs::read(&outputs.executable).unwrap();
        assert_eq!(
            outputs.executable.extension().and_then(|ext| ext.to_str()),
            Some("com")
        );
        assert!(asm.contains("; target: i8085"), "{asm}");
        assert!(asm.contains("    rim"), "{asm}");
        assert!(asm.contains("    sim"), "{asm}");
        assert!(asm.contains("    call 0005h"), "{asm}");
        assert!(com.windows(2).any(|bytes| bytes == [0x20, 0x30]));
        assert_eq!(&com[0..3], &[0xF3, 0x31, 0x00]);

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn test_command_runs_cpm_8080_and_8085_source_targets() {
        let root = temp_root("test_cpm_intel_targets");
        std::fs::create_dir_all(&root).unwrap();
        for (target, extra_asm) in [
            ("cpm-2.2-i8080", ""),
            ("cpm-2.2-i8085", "asm volatile { \"rim\" \"sim\" }"),
        ] {
            let source_path = root.join(format!("{target}.ezra"));
            std::fs::write(
                &source_path,
                format!(
                    r#"
                        import cpm.bdos

                        fn main() {{
                            {extra_asm}
                            bdos.exit()
                        }}
                    "#
                ),
            )
            .unwrap();

            test_source_with_command_options(&CommandOptions {
                path: source_path.to_string_lossy().into_owned(),
                debug_comments: false,
                default_sdk_symbols: false,
                layout_path: None,
                target: Some(target.to_owned()),
            })
            .unwrap();
        }

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn cli_target_overrides_project_target() {
        let root = temp_root("target_override");
        std::fs::create_dir_all(&root).unwrap();
        let source_path = root.join("game.ezra");
        std::fs::write(&source_path, "fn main() { test.pass() }\n").unwrap();
        std::fs::write(
            root.join("Ezra.toml"),
            r#"
                [build]
                target = "zxspectrum-z80"
            "#,
        )
        .unwrap();

        check(&CommandOptions {
            path: source_path.to_string_lossy().into_owned(),
            debug_comments: false,
            default_sdk_symbols: true,
            layout_path: None,
            target: Some("ti84plusce-ez80".to_owned()),
        })
        .unwrap();

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn commands_accept_intel_hex_output_format() {
        let root = temp_root("intel_hex_output");
        std::fs::create_dir_all(&root).unwrap();
        let source_path = root.join("game.ezra");
        std::fs::write(&source_path, "fn main() { test.pass() }\n").unwrap();
        std::fs::write(
            root.join("Ezra.toml"),
            r#"
                [build]
                target = "agonlight-console8-ez80"
                output = "hex"
            "#,
        )
        .unwrap();

        let outputs = build_source(source_path.to_str().unwrap()).unwrap();
        assert_eq!(
            outputs.executable.extension().and_then(|ext| ext.to_str()),
            Some("hex")
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn test_command_reports_stack_overflow() {
        let root = temp_root("stack_overflow_test");
        std::fs::create_dir_all(&root).unwrap();
        let source_path = root.join("game.ezra");
        std::fs::write(
            &source_path,
            r#"
                naked fn main() {
                    asm volatile(clobber sp, clobber hl) {
                        "ld sp, 0EF0000h"
                        "ld hl, 012345h"
                        "push hl"
                    }
                }
            "#,
        )
        .unwrap();

        let error = test_source(source_path.to_str().unwrap()).unwrap_err();

        assert!(
            error.contains("test stack overflowed into non-stack memory at SP=0xEEFFFD"),
            "{error}"
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn test_command_reports_execution_outside_mapped_memory() {
        let root = temp_root("outside_mapped_test");
        std::fs::create_dir_all(&root).unwrap();
        let source_path = root.join("game.ezra");
        std::fs::write(
            &source_path,
            r#"
                naked fn main() {
                    asm volatile {
                        "jp 020000h"
                    }
                }
            "#,
        )
        .unwrap();

        let error = test_source(source_path.to_str().unwrap()).unwrap_err();

        assert!(
            error.contains("test executed outside mapped memory at 0x020000"),
            "{error}"
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn test_command_reports_nonzero_test_result_code() {
        let root = temp_root("nonzero_test_result");
        std::fs::create_dir_all(&root).unwrap();
        let source_path = root.join("game.ezra");
        std::fs::write(
            &source_path,
            r#"
                fn main() {
                    test.fail(37)
                }
            "#,
        )
        .unwrap();

        let error = test_source(source_path.to_str().unwrap()).unwrap_err();

        assert_eq!(error, "test failed with code 37");

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn build_can_use_custom_layout_file() {
        let root = temp_root("custom_layout_build");
        std::fs::create_dir_all(&root).unwrap();
        let source_path = root.join("game.ezra");
        let layout_path = root.join("game.ezralayout");
        std::fs::write(
            &source_path,
            r#"
                global marker: u8 = 0x5A
                fn main() {
                    test.assert_eq_u8(marker, 0x5A, 1)
                    test.assert_eq_u24(EZRA_RAM_BASE, 0x030000, 2)
                    test.assert_eq_u24(EZRA_AUDIO_BASE, 0x0D0000, 3)
                    test.assert_eq_u24(EZRA_CODE_BASE, 0x020080, 4)
                    test.pass()
                }
            "#,
        )
        .unwrap();
        std::fs::write(
            &layout_path,
            r#"
                layout custom {
                    load 0x020000;
                    entry 0x020040;
                    stack 0xEFFF00;

                    region code 0x020000..0x02FFFF read execute;
                    region ram 0x030000..0x03FFFF read write;
                    region rodata 0x040000..0x04FFFF read;
                    region assets 0x120000..0x12FFFF read;
                    region scratch 0xE00000..0xE0FFFF read write;
                    section .header -> code align 64;
                    section .text -> code align 16;
                    section .rodata -> rodata align 16;
                    section .data -> ram align 16;
                    section .bss -> ram align 16;
                    section .assets -> assets align 256;
                    section .scratch -> scratch align 16;

                    symbol EZRA_LOAD_ADDR = 0x020000;
                    symbol EZRA_ENTRY_ADDR = 0x020040;
                    symbol EZRA_CODE_BASE = 0x020000 + cast<u8>(0x0180);
                    symbol EZRA_STACK_TOP = 0xEFFEFF + cast<bool>(0x1234);
                    symbol EZRA_RAM_BASE = 0x020000 + cast<ptr<u8>>(0x1010000);
                    symbol EZRA_AUDIO_BASE = 0x0CFF00 + cast<u16>(0x010100);
                }
            "#,
        )
        .unwrap();

        let outputs = build_source_with_command_options(&CommandOptions {
            path: source_path.to_string_lossy().into_owned(),
            debug_comments: false,
            default_sdk_symbols: true,
            layout_path: Some(layout_path.to_string_lossy().into_owned()),
            target: None,
        })
        .unwrap();

        let map = std::fs::read_to_string(&outputs.map).unwrap();
        let asm = std::fs::read_to_string(&outputs.asm).unwrap();
        let bin = std::fs::read(&outputs.executable).unwrap();

        assert!(
            map.starts_with("section      start      end        size\n"),
            "{map}"
        );
        assert!(map.contains(".text        0x020040"), "{map}");
        assert!(asm.contains("    ld sp, EFFF00h"), "{asm}");
        assert!(asm.contains("    ld (030000h), a"), "{asm}");
        assert!(!asm.contains("    ld (040000h), a"), "{asm}");
        assert_eq!(
            outputs.executable.extension().and_then(|ext| ext.to_str()),
            Some("bin")
        );
        assert_eq!(&bin[0..5], &[0xF3, 0x31, 0x00, 0xFF, 0xEF]);

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn emit_asm_can_use_custom_layout_file() {
        let root = temp_root("custom_layout_emit");
        std::fs::create_dir_all(&root).unwrap();
        let source_path = root.join("game.ezra");
        let layout_path = root.join("game.ezralayout");
        std::fs::write(&source_path, "fn main() { test.pass() }\n").unwrap();
        std::fs::write(
            &layout_path,
            r#"
                layout custom {
                    load 0x020000;
                    entry 0x020040;
                    stack 0xEFFE00;

                    region code 0x020000..0x02FFFF read execute;
                    region rodata 0x040000..0x04FFFF read;
                    region ram 0x050000..0x05FFFF read write;
                    region assets 0x120000..0x12FFFF read;
                    region scratch 0xE00000..0xE0FFFF read write;
                    section .header -> code align 64;
                    section .text -> code align 16;
                    section .rodata -> rodata align 16;
                    section .data -> ram align 16;
                    section .bss -> ram align 16;
                    section .assets -> assets align 256;
                    section .scratch -> scratch align 16;
                }
            "#,
        )
        .unwrap();

        let asm = emit_assembly_with_command_options(&CommandOptions {
            path: source_path.to_string_lossy().into_owned(),
            debug_comments: false,
            default_sdk_symbols: true,
            layout_path: Some(layout_path.to_string_lossy().into_owned()),
            target: None,
        })
        .unwrap();

        assert!(asm.contains("    ld sp, EFFE00h"), "{asm}");

        let _ = std::fs::remove_dir_all(root);
    }
}
