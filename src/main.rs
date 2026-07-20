use std::{
    env, fs,
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
    compile::{SdkResolver, load_program_with_sdk},
    diagnostic::SourceLocation,
    hir::HirProgram,
    layout::{Layout, parse_layout},
    parser::parse_program,
    project::{
        ArduboyConfig, AssetConfig, BankingConfig, GameBoyConfig, GameBoyMapper, ZxSpectrumConfig,
        load_nearest_project_config, load_project_config,
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
#[cfg(feature = "tms9900")]
use ezra::asm::emit_tms9900_assembly_with_options;

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
            let options = TestCommandOptions::parse(&args[1..])?;
            match options.path.as_ref() {
                Some(path) => {
                    test_source_with_command_options(&options.command_with_path(path.clone()))
                }
                None => test_project_with_command_options(&options),
            }
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

#[derive(Clone, Debug, Eq, PartialEq)]
struct TestCommandOptions {
    path: Option<String>,
    debug_comments: bool,
    default_sdk_symbols: bool,
    layout_path: Option<String>,
    target: Option<String>,
}

impl TestCommandOptions {
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
                "--layout" => layout_path = Some(iter.next().ok_or_else(usage)?.clone()),
                "--target" => target = Some(iter.next().ok_or_else(usage)?.clone()),
                _ if path.is_none() => path = Some(arg.clone()),
                _ => return Err(usage()),
            }
        }
        Ok(Self {
            path,
            debug_comments,
            default_sdk_symbols,
            layout_path,
            target,
        })
    }

    fn command_with_path(&self, path: String) -> CommandOptions {
        CommandOptions {
            path,
            debug_comments: self.debug_comments,
            default_sdk_symbols: self.default_sdk_symbols,
            layout_path: self.layout_path.clone(),
            target: self.target.clone(),
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
    let target = resolve_target_profile(options.target.as_deref())?;
    let layout_path = options.layout_path.as_ref().map(PathBuf::from);
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
        gameboy: None,
        gameboy_banking: None,
        arduboy: None,
        zxspectrum: None,
        default_sdk_symbols: true,
        output_root: source_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("target"),
        executable_name: None,
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
        .as_ref()
        .map(PathBuf::from)
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
    fs::write(&output_path, linked.executable)
        .map_err(|error| format!("failed to write {}: {error}", output_path.display()))?;
    println!("wrote {}", output_path.display());
    if let Some(map_path) = options.map_path.as_ref().map(PathBuf::from) {
        fs::write(&map_path, linked.map)
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
    gameboy: Option<GameBoyConfig>,
    gameboy_banking: Option<GameBoyBankingOptions>,
    arduboy: Option<ArduboyConfig>,
    zxspectrum: Option<ZxSpectrumConfig>,
    default_sdk_symbols: bool,
    output_root: PathBuf,
    executable_name: Option<String>,
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
    let layout = match layout_path.as_deref() {
        Some(path) => load_layout(Some(path), &target.triple.value)?,
        None if output_format == OutputFormat::Commodore64Crt => Layout::commodore64_crt(),
        None => default_layout_for_target(&target.triple.value),
    };
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
    let gameboy = project.as_ref().and_then(|project| project.gameboy.clone());
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
        gameboy,
        gameboy_banking,
        arduboy,
        zxspectrum,
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
            | CpuFamily::I8086
            | CpuFamily::Lr35902
            | CpuFamily::Avr
            | CpuFamily::Mos6502
            | CpuFamily::Tms9900
    ) {
        return Ok(());
    }
    #[cfg(feature = "m6800")]
    if settings.target.triple.cpu == CpuFamily::M6800 {
        return Ok(());
    }
    #[cfg(feature = "tms9900")]
    if settings.target.triple.cpu == CpuFamily::Tms9900 {
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
    } else if options.cpu == CpuFamily::Tms9900 {
        #[cfg(feature = "tms9900")]
        {
            emit_tms9900_assembly_with_options(program, options)
        }
        #[cfg(not(feature = "tms9900"))]
        {
            unreachable!("TMS9900 targets require the tms9900 Cargo feature")
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

fn build(options: &BuildCommandOptions) -> Result<(), String> {
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
        target_options.path = Some(source_path.to_string_lossy().into_owned());
        target_options.target = target;
        let outputs = build_source_with_build_options(&target_options)?;
        println!("wrote {}", outputs.asm.display());
        println!("wrote {}", outputs.map.display());
        println!("wrote {}", outputs.executable.display());
    }
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
        Some("asm" | "s" | "z80" | "ez80" | "i8080" | "8080" | "i8086" | "8086") => {
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
    let mut program = load_program_with_sdk(source_path, &settings.sdk).map_err(|error| {
        error
            .with_location_if_missing(source_location.clone())
            .to_string()
    })?;
    apply_asset_configuration(&mut program, settings);
    ensure_source_codegen_supported(settings)?;
    let assembly = emit_source_assembly(
        &program,
        ezra::api::assembly_options_for_layout_and_program(
            &settings.layout,
            &program,
            settings.target.triple.cpu,
            &settings.target.triple.value,
            options.debug_comments,
            settings.default_sdk_symbols,
            settings.gameboy_banking,
        )
        .map_err(|error| error.to_string())?,
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
    fs::write(&executable_path, linked.executable)
        .map_err(|error| format!("failed to write {}: {error}", executable_path.display()))?;

    Ok(BuildOutputs {
        asm: asm_path,
        map: map_path,
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
    fs::write(&executable_path, linked.executable)
        .map_err(|error| format!("failed to write {}: {error}", executable_path.display()))?;

    Ok(BuildOutputs {
        asm: asm_path,
        map: map_path,
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
    Ok(settings
        .output_root
        .join(&settings.target.triple.value)
        .join(source_stem))
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
            path: source.display().to_string(),
            debug_comments: options.debug_comments,
            default_sdk_symbols: options.default_sdk_symbols,
            layout_path: options.layout_path.clone(),
            target,
        };
        let build_options = BuildCommandOptions {
            path: Some(command.path.clone()),
            debug_comments: command.debug_comments,
            default_sdk_symbols: command.default_sdk_symbols,
            input_kind: Some(InputKind::Ezra),
            assembler_cpu: None,
            layout_path: command.layout_path.clone(),
            target: command.target.clone(),
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
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(format!(
                "failed to read test directory `{}`: {error}",
                root.display()
            ));
        }
    };
    for entry in entries {
        let entry =
            entry.map_err(|error| format!("failed to read test directory entry: {error}"))?;
        let path = entry.path();
        if path.is_dir() {
            discover_ezra_test_sources(&path, sources)?;
        } else if path
            .extension()
            .is_some_and(|extension| extension == "ezra")
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
        ezra::api::assembly_options_for_layout_and_program(
            &settings.layout,
            &program,
            settings.target.triple.cpu,
            &settings.target.triple.value,
            options.debug_comments,
            settings.default_sdk_symbols,
            settings.gameboy_banking,
        )
        .map_err(|error| error.to_string())?,
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
                &ezra::api::assembly_options_for_layout_and_program(
                    &settings.layout,
                    &program,
                    settings.target.triple.cpu,
                    &settings.target.triple.value,
                    options.command.debug_comments,
                    settings.default_sdk_symbols,
                    settings.gameboy_banking,
                )
                .map_err(|error| error.to_string())?,
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
    let assembly = emit_source_assembly(
        &program,
        ezra::api::assembly_options_for_layout_and_program(
            &settings.layout,
            &program,
            settings.target.triple.cpu,
            &settings.target.triple.value,
            options.debug_comments,
            settings.default_sdk_symbols,
            settings.gameboy_banking,
        )
        .map_err(|error| error.to_string())?,
    )
    .map_err(|error| {
        error
            .with_location_if_missing(source_location.clone())
            .to_string()
    })?;
    validate_generated_assembly_for_command(&source_path, &source_location, &settings, &assembly)?;
    Ok(assembly)
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
    let assembly = emit_source_assembly(
        &program,
        ezra::api::assembly_options_for_layout_and_program(
            &settings.layout,
            &program,
            settings.target.triple.cpu,
            &settings.target.triple.value,
            options.debug_comments,
            settings.default_sdk_symbols,
            settings.gameboy_banking,
        )
        .map_err(|error| error.to_string())?,
    )
    .map_err(|error| {
        error
            .with_location_if_missing(source_location.clone())
            .to_string()
    })?;
    validate_generated_assembly_for_command(source_path, &source_location, &settings, &assembly)?;

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

    fs::create_dir_all(root)
        .map_err(|error| format!("failed to create {}: {error}", root.display()))?;
    write_scaffold_file(
        &root.join(".gitignore"),
        options.force,
        "target/\n*.bin\n*.com\n*.gaem\n*.hex\n*.tap\n*.gb\n*.prg\n*.8xp\n*.8ek\n*.8xk\n*.map\n*.asm\n",
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
        #[cfg(feature = "dcpu")]
        TargetRow {
            triple: "generic-dcpu-bare",
            cpu: "dcpu",
            address_width_bits: 16,
            output: "bin",
            sdk: "none",
            status: "assembly-only DCPU-16 target",
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
    "usage: ezra <command>\n\ncommands:\n  init [--name <name>] [--target <triple>] [--force] [dir]\n                                       create a new EZRA project scaffold\n  install-syntax (--all | [--editor] <editor>...) [--dry-run]\n                                       install editor syntax files for selected editors\n  targets                              list documented target triples, outputs, and SDKs\n  lsp                                  start the language server; requires Cargo feature `lsp`\n  check [--target <triple>] [--debug-comments] [--no-default-sdk-symbols] [--layout <file.ezralayout>] <file.ezra>\n                                       parse and validate a source file\n  build [--target <triple>] [--cpu <mode>] [--input-kind ezra|assembly] [--debug-comments] [--no-default-sdk-symbols] [--layout <file.ezralayout>] [file.ezra|file.asm]\n                                       write .asm, .map, and target executable artifacts\n  emit-asm [--target <triple>] [--debug-comments] [--no-default-sdk-symbols] [--layout <file.ezralayout>] <file.ezra>\n                                       emit readable target assembly\n  emit-ir [--stage hir|tbir] [--target <triple>] [--debug-comments] [--no-default-sdk-symbols] [--layout <file.ezralayout>] <file.ezra>\n                                       emit inspectable HIR or TBIR text\n  test [--target <triple>] [--debug-comments] [--no-default-sdk-symbols] [--layout <file.ezralayout>] <file.ezra>\n                                       emit and run on the target VM\n  assemble [--target <triple>] [--cpu <mode>] [--layout <file.ezralayout>] [--map <file.map>] [--base <addr>] [--output <file.bin>] <file.asm>\n                                       assemble target assembly into a raw binary\n  layout [file.ezralayout]             print the default or custom EZRA layout summary\n  header                               print the default 64-byte cartridge header\n\neditors for install-syntax: vim, neovim, nano, micro, helix, vscode, zed, notepad++".to_owned()
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
            path: Some(source_path.to_string_lossy().into_owned()),
            debug_comments: false,
            default_sdk_symbols: true,
            input_kind: None,
            assembler_cpu: None,
            layout_path: None,
            target: Some("msdos-com-i8086".to_owned()),
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
            path: source_path.to_string_lossy().into_owned(),
            debug_comments: false,
            default_sdk_symbols: true,
            layout_path: None,
            target: Some("custom-board-i8086".to_owned()),
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
            path: source_path.to_string_lossy().into_owned(),
            debug_comments: false,
            default_sdk_symbols: true,
            layout_path: None,
            target: Some("bare-i8086".to_owned()),
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
            path: source_path.to_string_lossy().into_owned(),
            debug_comments: false,
            default_sdk_symbols: true,
            layout_path: None,
            target: Some("bare-i8086".to_owned()),
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
            path: source_path.to_string_lossy().into_owned(),
            debug_comments: false,
            default_sdk_symbols: true,
            layout_path: Some(layout_path.to_string_lossy().into_owned()),
            target: Some("bare-i8086".to_owned()),
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
