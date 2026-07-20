use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
    process::ExitCode,
};

use ezra::{
    asm::{
        AssemblyItem, AssemblyOptions, AssemblyPreprocessOptions, AssemblyProgram,
        GameBoyBankingMapper, GameBoyBankingOptions, emit_ez80_assembly_with_options,
        emit_lr35902_assembly_with_options, emit_mos6502_assembly_with_options,
        preprocess_assembly_file,
    },
    ast::Program,
    cart::{CartridgeHeader, collect_gameboy_banked_embeds, layout_section_bases},
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
        Address24, AssemblerCpu, CpuFamily, EZRA_ASSET_BASE, EZRA_AUDIO_BASE, EZRA_RAM_BASE,
        EZRA_RODATA_BASE, EZRA_VRAM_BASE, OutputFormat, TargetProfile, parse_output_format,
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
    let image = if let Some(base_addr) = options.base_addr {
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
        let preprocessed = preprocess_assembly_file(
            &source_path,
            AssemblyPreprocessOptions::for_compiled_features(
                &settings.target.triple.value,
                settings.assembler_cpu.as_str(),
            ),
        )
        .map_err(|error| error.to_string())?;
        let assembled = ezra::vm::assemble_program_with_options_at(
            settings.assembler_cpu,
            &preprocessed.program,
            base_addr,
            &assembly_source_options(&source_path, &settings.layout),
        )
        .map_err(|error| error.to_string())?;
        AssemblyBuildImage {
            map: flat_assembly_map(&settings.layout, assembled.bytes.len(), &assembled.symbols)?,
            bytes: assembled.bytes,
            symbols: assembled.symbols,
        }
    } else {
        build_assembly_image(&source_path, &settings)?
    };
    let output_path = options
        .output
        .as_ref()
        .map(PathBuf::from)
        .unwrap_or_else(|| source_path.with_extension(executable_extension(&settings)));
    let executable =
        build_executable_bytes(&settings, &image.bytes, Some(&output_path), None, &[])?;
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
        assembly_options_from_layout_and_program(
            &settings.layout,
            &program,
            settings.target.triple.cpu,
            &settings.target.triple.value,
            options.debug_comments,
            settings.default_sdk_symbols,
            settings.gameboy_banking,
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
    program: AssemblyProgram,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PlacedAssemblySection {
    name: String,
    start: u32,
    bytes: Vec<u8>,
}

fn build_assembly_image(
    source_path: &Path,
    settings: &BuildSettings,
) -> Result<AssemblyBuildImage, String> {
    let preprocessed = preprocess_assembly_file(
        source_path,
        AssemblyPreprocessOptions::for_compiled_features(
            &settings.target.triple.value,
            settings.assembler_cpu.as_str(),
        ),
    )
    .map_err(|error| error.to_string())?;
    build_assembly_program_image(source_path, &preprocessed.program, settings)
}

fn build_assembly_program_image(
    source_path: &Path,
    program: &AssemblyProgram,
    settings: &BuildSettings,
) -> Result<AssemblyBuildImage, String> {
    let sections = split_assembly_sections(program);
    let section_bases = placed_assembly_section_bases(source_path, settings, &sections)?;
    let mut options = assembly_source_options(source_path, &settings.layout);
    options.section_bases = section_bases
        .iter()
        .map(|(name, start, _)| ezra::vm::AssemblySymbol {
            name: name.clone(),
            addr: *start,
        })
        .collect();
    let assembled = ezra::vm::assemble_program_with_options_at(
        settings.assembler_cpu,
        program,
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
) -> Result<Vec<(String, u32, usize)>, String> {
    let mut lengths = BTreeMap::new();
    for section in sections {
        let len = ezra::vm::measure_assembly_program_with_options(
            settings.assembler_cpu,
            &section.program,
            &ezra::vm::AssemblerSourceOptions {
                source_path: Some(source_path.to_path_buf()),
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

fn split_assembly_sections(program: &AssemblyProgram) -> Vec<AssemblySectionSource> {
    let mut sections = BTreeMap::<String, AssemblyProgram>::new();
    let mut current = ".text".to_owned();
    sections.insert(current.clone(), AssemblyProgram { items: Vec::new() });
    for item in &program.items {
        if let AssemblyItem::Section(name) = &item.kind {
            current = name.clone();
            sections
                .entry(current.clone())
                .or_insert_with(|| AssemblyProgram { items: Vec::new() });
        } else {
            sections
                .entry(current.clone())
                .or_insert_with(|| AssemblyProgram { items: Vec::new() })
                .items
                .push(item.clone());
        }
    }
    sections
        .into_iter()
        .map(|(name, program)| AssemblySectionSource { name, program })
        .collect()
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
        ..ezra::vm::AssemblerSourceOptions::default()
    }
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

fn build_executable_bytes(
    settings: &BuildSettings,
    code: &[u8],
    output_path: Option<&Path>,
    program: Option<&Program>,
    symbols: &[ezra::vm::AssemblySymbol],
) -> Result<Vec<u8>, String> {
    if settings.output_format == OutputFormat::Arduboy {
        return arduboy_package_bytes(settings, output_path, code);
    }
    if settings
        .target
        .triple
        .value
        .starts_with("agonlight-mos-ez80")
        || matches!(
            settings.output_format,
            OutputFormat::RawBin
                | OutputFormat::CpmCom
                | OutputFormat::Ez180nGaem
                | OutputFormat::IntelHex
                | OutputFormat::ArduinoHex
                | OutputFormat::Commodore64Prg
                | OutputFormat::Commodore64Crt
        )
    {
        let request = ezra::package::PackageRequest {
            target: settings.target.triple.value.clone(),
            output_format: settings.output_format,
            load_addr: settings.layout.load.get(),
            entry_addr: settings.layout.entry.get(),
            executable_name: settings.executable_name.clone(),
        };
        return ezra::package::package_executable(&request, code)
            .map_err(|error| error.to_string());
    }
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
        return game_boy_rom_bytes(settings, output_path, code, program, symbols);
    }
    if settings.output_format == OutputFormat::Commodore64Prg {
        return commodore64_prg_bytes(settings, code);
    }
    if settings.output_format == OutputFormat::Commodore64Crt {
        return commodore64_crt_bytes(settings, code);
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

fn arduboy_package_bytes(
    settings: &BuildSettings,
    output_path: Option<&Path>,
    code: &[u8],
) -> Result<Vec<u8>, String> {
    if !settings.target.triple.value.starts_with("arduboy-") {
        return Err(format!(
            "target `{}` does not support Arduboy .arduboy output",
            settings.target.triple.value
        ));
    }
    let config = settings.arduboy.as_ref().ok_or_else(|| {
        "`[arduboy]` metadata is required when `build.output = \"arduboy\"`".to_owned()
    })?;
    let output_path = output_path.ok_or_else(|| {
        "Arduboy packaging requires an output path to determine the embedded HEX filename"
            .to_owned()
    })?;
    let executable = output_path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| !stem.is_empty())
        .ok_or_else(|| {
            format!(
                "Arduboy output path `{}` has no valid executable filename",
                output_path.display()
            )
        })?;
    let hex_filename = format!("{executable}.hex");
    let hex = ezra::package::package_executable(
        &ezra::package::PackageRequest {
            target: settings.target.triple.value.clone(),
            output_format: OutputFormat::ArduinoHex,
            load_addr: settings.layout.load.get(),
            entry_addr: settings.layout.entry.get(),
            executable_name: settings.executable_name.clone(),
        },
        code,
    )
    .map_err(|error| error.to_string())?;
    let info = arduboy_info_json(config, &hex_filename);
    stored_zip_bytes(&[("info.json", info.as_bytes()), (&hex_filename, &hex)])
}

fn arduboy_info_json(config: &ArduboyConfig, hex_filename: &str) -> String {
    let mut fields = vec![
        ("schemaVersion", "2".to_owned()),
        ("title", json_string(&config.title)),
        ("author", json_string(&config.author)),
        ("version", json_string(&config.version)),
    ];
    for (name, value) in [
        ("description", config.description.as_deref()),
        ("date", config.date.as_deref()),
        ("genre", config.genre.as_deref()),
        ("sourceUrl", config.source_url.as_deref()),
    ] {
        if let Some(value) = value {
            fields.push((name, json_string(value)));
        }
    }
    fields.push((
        "binaries",
        format!(
            "[{{\"filename\":{},\"device\":\"Arduboy\"}}]",
            json_string(hex_filename)
        ),
    ));
    let mut info = String::from("{");
    for (index, (name, value)) in fields.into_iter().enumerate() {
        if index != 0 {
            info.push(',');
        }
        info.push_str(&json_string(name));
        info.push(':');
        info.push_str(&value);
    }
    info.push('}');
    info
}

fn json_string(value: &str) -> String {
    let mut json = String::with_capacity(value.len() + 2);
    json.push('"');
    for character in value.chars() {
        match character {
            '"' => json.push_str("\\\""),
            '\\' => json.push_str("\\\\"),
            '\n' => json.push_str("\\n"),
            '\r' => json.push_str("\\r"),
            '\t' => json.push_str("\\t"),
            character if character <= '\u{1F}' => {
                json.push_str(&format!("\\u{:04X}", character as u32));
            }
            character => json.push(character),
        }
    }
    json.push('"');
    json
}

fn stored_zip_bytes(entries: &[(&str, &[u8])]) -> Result<Vec<u8>, String> {
    const LOCAL_FILE_HEADER: u32 = 0x0403_4B50;
    const CENTRAL_DIRECTORY_HEADER: u32 = 0x0201_4B50;
    const END_OF_CENTRAL_DIRECTORY: u32 = 0x0605_4B50;
    const VERSION_NEEDED: u16 = 20;
    const VERSION_MADE_BY: u16 = 20;
    const UTF8_FLAG: u16 = 1 << 11;
    const STORED: u16 = 0;
    const DOS_DATE_1980_01_01: u16 = 0x0021;

    let entry_count = u16::try_from(entries.len())
        .map_err(|_| "Arduboy ZIP contains too many entries".to_owned())?;
    let mut zip = Vec::new();
    let mut central_directory = Vec::new();
    for (name, data) in entries {
        let name = name.as_bytes();
        let name_len = u16::try_from(name.len())
            .map_err(|_| "Arduboy ZIP entry name is too long".to_owned())?;
        let data_len =
            u32::try_from(data.len()).map_err(|_| "Arduboy ZIP entry exceeds 4 GiB".to_owned())?;
        let offset =
            u32::try_from(zip.len()).map_err(|_| "Arduboy ZIP exceeds 4 GiB".to_owned())?;
        let crc = zip_crc32(data);

        push_zip_u32(&mut zip, LOCAL_FILE_HEADER);
        push_zip_u16(&mut zip, VERSION_NEEDED);
        push_zip_u16(&mut zip, UTF8_FLAG);
        push_zip_u16(&mut zip, STORED);
        push_zip_u16(&mut zip, 0);
        push_zip_u16(&mut zip, DOS_DATE_1980_01_01);
        push_zip_u32(&mut zip, crc);
        push_zip_u32(&mut zip, data_len);
        push_zip_u32(&mut zip, data_len);
        push_zip_u16(&mut zip, name_len);
        push_zip_u16(&mut zip, 0);
        zip.extend_from_slice(name);
        zip.extend_from_slice(data);

        push_zip_u32(&mut central_directory, CENTRAL_DIRECTORY_HEADER);
        push_zip_u16(&mut central_directory, VERSION_MADE_BY);
        push_zip_u16(&mut central_directory, VERSION_NEEDED);
        push_zip_u16(&mut central_directory, UTF8_FLAG);
        push_zip_u16(&mut central_directory, STORED);
        push_zip_u16(&mut central_directory, 0);
        push_zip_u16(&mut central_directory, DOS_DATE_1980_01_01);
        push_zip_u32(&mut central_directory, crc);
        push_zip_u32(&mut central_directory, data_len);
        push_zip_u32(&mut central_directory, data_len);
        push_zip_u16(&mut central_directory, name_len);
        push_zip_u16(&mut central_directory, 0);
        push_zip_u16(&mut central_directory, 0);
        push_zip_u16(&mut central_directory, 0);
        push_zip_u16(&mut central_directory, 0);
        push_zip_u32(&mut central_directory, 0);
        push_zip_u32(&mut central_directory, offset);
        central_directory.extend_from_slice(name);
    }

    let central_offset =
        u32::try_from(zip.len()).map_err(|_| "Arduboy ZIP exceeds 4 GiB".to_owned())?;
    let central_len = u32::try_from(central_directory.len())
        .map_err(|_| "Arduboy ZIP central directory exceeds 4 GiB".to_owned())?;
    zip.extend_from_slice(&central_directory);
    push_zip_u32(&mut zip, END_OF_CENTRAL_DIRECTORY);
    push_zip_u16(&mut zip, 0);
    push_zip_u16(&mut zip, 0);
    push_zip_u16(&mut zip, entry_count);
    push_zip_u16(&mut zip, entry_count);
    push_zip_u32(&mut zip, central_len);
    push_zip_u32(&mut zip, central_offset);
    push_zip_u16(&mut zip, 0);
    Ok(zip)
}

fn push_zip_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn push_zip_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn zip_crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for byte in data {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xEDB8_8320 & (0u32.wrapping_sub(crc & 1)));
        }
    }
    !crc
}

fn commodore64_prg_bytes(settings: &BuildSettings, code: &[u8]) -> Result<Vec<u8>, String> {
    if !settings.target.triple.value.starts_with("commodore64-6502") {
        return Err(format!(
            "target `{}` does not support Commodore 64 .prg output",
            settings.target.triple.value
        ));
    }
    if settings.layout.load.get() != 0x080D || settings.layout.entry.get() != 0x080D {
        return Err("Commodore 64 PRG layouts must load and enter at 0x080D".to_owned());
    }

    // BASIC starts at $0801. This tokenized `10 SYS2061` line invokes the
    // machine-code entry point at $080D when a PRG is autostarted by VICE.
    const BASIC_AUTOSTART: [u8; 12] = [
        0x0B, 0x08, // next BASIC line at $080B
        0x0A, 0x00, // line 10
        0x9E, // SYS token
        b'2', b'0', b'6', b'1', 0x00, // SYS2061
        0x00, 0x00, // end of BASIC program
    ];

    let mut output = Vec::with_capacity(code.len() + 2 + BASIC_AUTOSTART.len());
    output.extend_from_slice(&0x0801u16.to_le_bytes());
    output.extend_from_slice(&BASIC_AUTOSTART);
    output.extend_from_slice(code);
    Ok(output)
}

fn commodore64_crt_bytes(settings: &BuildSettings, code: &[u8]) -> Result<Vec<u8>, String> {
    if !settings.target.triple.value.starts_with("commodore64-6502") {
        return Err(format!(
            "target `{}` does not support Commodore 64 .crt output",
            settings.target.triple.value
        ));
    }
    if settings.layout.load.get() != 0x8009 || settings.layout.entry.get() != 0x8009 {
        return Err("standard Commodore 64 CRT layouts must load and enter at 0x8009".to_owned());
    }
    const ROM_SIZE: usize = 0x2000;
    const CARTRIDGE_HEADER_SIZE: usize = 0x40;
    const CHIP_HEADER_SIZE: usize = 0x10;
    const CARTRIDGE_STARTUP_SIZE: usize = 9;
    if code.len() > ROM_SIZE - CARTRIDGE_STARTUP_SIZE {
        return Err(format!(
            "program code is {} bytes, but the standard 8 KiB Commodore 64 CRT supports at most {} bytes; use a smaller program or a bank-switched cartridge format",
            code.len(),
            ROM_SIZE - CARTRIDGE_STARTUP_SIZE
        ));
    }

    let mut output = Vec::with_capacity(CARTRIDGE_HEADER_SIZE + CHIP_HEADER_SIZE + ROM_SIZE);
    output.extend_from_slice(b"C64 CARTRIDGE   ");
    output.extend_from_slice(&0x40u32.to_be_bytes());
    output.extend_from_slice(&0x0100u16.to_be_bytes());
    output.extend_from_slice(&0u16.to_be_bytes()); // Standard cartridge hardware type.
    output.push(0); // EXROM asserted: 8 KiB cartridge mode.
    output.push(1); // GAME inactive.
    output.extend_from_slice(&[0; 6]);
    let mut name = [0u8; 32];
    name[..10].copy_from_slice(b"EZRA C64  ");
    output.extend_from_slice(&name);

    output.extend_from_slice(b"CHIP");
    output.extend_from_slice(&(u32::try_from(CHIP_HEADER_SIZE + ROM_SIZE).unwrap()).to_be_bytes());
    output.extend_from_slice(&0u16.to_be_bytes()); // ROM chip.
    output.extend_from_slice(&0u16.to_be_bytes()); // Bank 0.
    output.extend_from_slice(&0x8000u16.to_be_bytes());
    output.extend_from_slice(&(ROM_SIZE as u16).to_be_bytes());

    output.extend_from_slice(&0x8009u16.to_le_bytes()); // Cold-start vector.
    output.extend_from_slice(&0x8009u16.to_le_bytes()); // Warm-start vector.
    output.extend_from_slice(b"CBM80");
    output.extend_from_slice(code);
    output.resize(CARTRIDGE_HEADER_SIZE + CHIP_HEADER_SIZE + ROM_SIZE, 0xFF);
    Ok(output)
}

fn game_boy_banked_code_payloads(
    code: &[u8],
    symbols: &[ezra::vm::AssemblySymbol],
    base: u32,
) -> Result<BTreeMap<usize, Vec<u8>>, String> {
    let mut starts = BTreeMap::new();
    let mut ends = BTreeMap::new();
    for symbol in symbols {
        let Some(rest) = symbol.name.strip_prefix("__ezra_bank_") else {
            continue;
        };
        let Some((bank, suffix)) = rest.split_once('_') else {
            continue;
        };
        let bank = bank
            .parse::<usize>()
            .map_err(|_| format!("invalid generated Game Boy bank marker `{}`", symbol.name))?;
        match suffix {
            "start" => {
                starts.insert(bank, symbol.addr);
            }
            "end" => {
                ends.insert(bank, symbol.addr);
            }
            _ => {}
        }
    }
    let mut payloads = BTreeMap::new();
    for (bank, start) in starts {
        let end = ends
            .remove(&bank)
            .ok_or_else(|| format!("generated Game Boy bank {bank} has no end marker"))?;
        let start = usize::try_from(
            start
                .checked_sub(base)
                .ok_or_else(|| format!("generated Game Boy bank {bank} precedes resident code"))?,
        )
        .map_err(|_| "generated Game Boy bank offset exceeds host range".to_owned())?;
        let end = usize::try_from(
            end.checked_sub(base)
                .ok_or_else(|| format!("generated Game Boy bank {bank} precedes resident code"))?,
        )
        .map_err(|_| "generated Game Boy bank offset exceeds host range".to_owned())?;
        if start > end || end > code.len() {
            return Err(format!(
                "generated Game Boy bank {bank} is outside assembled code"
            ));
        }
        payloads.insert(bank, code[start..end].to_vec());
    }
    if !ends.is_empty() {
        return Err("generated Game Boy bank end marker has no start marker".to_owned());
    }
    Ok(payloads)
}

fn game_boy_rom_bytes(
    settings: &BuildSettings,
    output_path: Option<&Path>,
    code: &[u8],
    program: Option<&Program>,
    symbols: &[ezra::vm::AssemblySymbol],
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

    const BANK_SIZE: usize = 0x4000;
    const INITIAL_ROM_SIZE: usize = 0x8000;
    const CODE_OFFSET: usize = 0x0150;
    let config = settings.gameboy.clone().unwrap_or_default();
    let mut generated_payloads =
        game_boy_banked_code_payloads(code, symbols, settings.layout.entry.get())?;
    let banked_code_start = symbols
        .iter()
        .filter(|symbol| symbol.name.starts_with("__ezra_bank_") && symbol.name.ends_with("_start"))
        .map(|symbol| symbol.addr)
        .min();
    let code = banked_code_start
        .map(|address| {
            &code[..usize::try_from(address.saturating_sub(settings.layout.entry.get()))
                .unwrap_or(code.len())]
        })
        .unwrap_or(code);
    if generated_payloads.is_empty() {
        if let Some(program) = program {
            for embed in
                collect_gameboy_banked_embeds(program).map_err(|error| error.to_string())?
            {
                let bank = usize::try_from(embed.bank)
                    .map_err(|_| format!("Game Boy bank {} is outside host range", embed.bank))?;
                let payload = generated_payloads.entry(bank).or_default();
                if payload
                    .len()
                    .checked_add(embed.bytes.len())
                    .map_or(true, |len| len > BANK_SIZE)
                {
                    return Err(format!(
                        "banked embeds in Game Boy ROM bank {bank} exceed its 16 KiB window"
                    ));
                }
                payload.extend_from_slice(&embed.bytes);
            }
        }
    }
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
    let payload_banks = game_boy_payload_banks(config.mapper, bank_payloads.len())?;
    for bank in generated_payloads.keys() {
        validate_game_boy_generated_bank(config.mapper, *bank)?;
        if payload_banks.contains(bank) {
            return Err(format!(
                "Game Boy ROM bank {bank} is used by both source-banked content and `gameboy.bank_files`"
            ));
        }
    }
    let required_banks = payload_banks
        .iter()
        .copied()
        .chain(generated_payloads.keys().copied())
        .max()
        .map(|bank| bank + 1)
        .unwrap_or(2);
    let rom_banks = game_boy_rom_banks(&config, required_banks)?;
    let rom_size = rom_banks
        .checked_mul(BANK_SIZE)
        .ok_or_else(|| "Game Boy ROM size overflow".to_owned())?;
    let fixed_code_capacity = if settings.gameboy_banking.is_some() {
        BANK_SIZE - CODE_OFFSET
    } else {
        INITIAL_ROM_SIZE - CODE_OFFSET
    };
    if code.len() > fixed_code_capacity {
        return Err(format!(
            "Game Boy fixed-bank code is {} bytes, but bank 0 supports at most {} bytes from 0x0150 when explicit banking is enabled",
            code.len(),
            fixed_code_capacity
        ));
    }
    for (index, payload) in bank_payloads.iter().enumerate() {
        if payload.len() > BANK_SIZE {
            return Err(format!(
                "Game Boy bank file `{}` is {} bytes, but switchable ROM bank {} holds at most {} bytes",
                config.bank_files[index].display(),
                payload.len(),
                payload_banks[index],
                BANK_SIZE
            ));
        }
    }

    let (cartridge_type, ram_size_code) = game_boy_cartridge_header(&config)?;
    let mut rom = vec![0xFF; rom_size];
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
    rom[0x0147] = cartridge_type;
    rom[0x0148] = game_boy_rom_size_code(rom_banks)?;
    rom[0x0149] = ram_size_code;
    rom[0x014A] = 0x01;
    rom[0x014B] = 0x33;
    rom[0x014C] = 0x00;
    rom[CODE_OFFSET..CODE_OFFSET + code.len()].copy_from_slice(code);
    for (index, payload) in bank_payloads.iter().enumerate() {
        let offset = payload_banks[index] * BANK_SIZE;
        rom[offset..offset + payload.len()].copy_from_slice(payload);
    }
    for (bank, payload) in generated_payloads {
        let offset = bank * BANK_SIZE;
        rom[offset..offset + payload.len()].copy_from_slice(&payload);
    }
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

fn validate_game_boy_generated_bank(mapper: GameBoyMapper, bank: usize) -> Result<(), String> {
    let maximum = match mapper {
        GameBoyMapper::RomOnly => 0,
        GameBoyMapper::Mbc1 => 127,
        GameBoyMapper::Mbc5 => 511,
    };
    if bank == 0 || bank > maximum || (mapper == GameBoyMapper::Mbc1 && bank & 0x1F == 0) {
        return Err(format!(
            "Game Boy mapper `{}` cannot select explicit ROM bank {bank}",
            game_boy_mapper_name(mapper)
        ));
    }
    Ok(())
}

fn game_boy_payload_banks(
    mapper: GameBoyMapper,
    payload_count: usize,
) -> Result<Vec<usize>, String> {
    if payload_count == 0 {
        return Ok(Vec::new());
    }
    let mut banks = Vec::with_capacity(payload_count);
    let maximum = match mapper {
        GameBoyMapper::RomOnly => 1,
        GameBoyMapper::Mbc1 => 127,
        GameBoyMapper::Mbc5 => 511,
    };
    for bank in 2..=maximum {
        if mapper == GameBoyMapper::Mbc1 && bank & 0x1F == 0 {
            continue;
        }
        banks.push(bank);
        if banks.len() == payload_count {
            return Ok(banks);
        }
    }
    if payload_count == 0 {
        Ok(banks)
    } else {
        Err(format!(
            "Game Boy mapper `{}` supports at most {} configured switchable bank file(s)",
            game_boy_mapper_name(mapper),
            banks.len()
        ))
    }
}

fn game_boy_rom_banks(config: &GameBoyConfig, required_banks: usize) -> Result<usize, String> {
    let mapper_max_banks = match config.mapper {
        GameBoyMapper::RomOnly => 2,
        GameBoyMapper::Mbc1 => 128,
        GameBoyMapper::Mbc5 => 512,
    };
    let rom_banks = config
        .rom_banks
        .map(usize::from)
        .unwrap_or_else(|| required_banks.next_power_of_two().max(2));
    if !rom_banks.is_power_of_two() || !(2..=512).contains(&rom_banks) {
        return Err(
            "Game Boy `gameboy.rom_banks` must be a power of two from 2 through 512".to_owned(),
        );
    }
    if rom_banks < required_banks {
        return Err(format!(
            "Game Boy `gameboy.rom_banks` is {rom_banks}, but {} banks are required for the fixed image and {} bank file(s)",
            required_banks,
            required_banks - 2
        ));
    }
    if rom_banks > mapper_max_banks {
        return Err(format!(
            "Game Boy mapper `{}` supports at most {mapper_max_banks} ROM banks, not {rom_banks}",
            game_boy_mapper_name(config.mapper)
        ));
    }
    if config.mapper == GameBoyMapper::RomOnly && (required_banks != 2 || rom_banks != 2) {
        return Err("Game Boy ROM-only cartridges cannot use `gameboy.bank_files` or more than two ROM banks".to_owned());
    }
    Ok(rom_banks)
}

fn game_boy_mapper_name(mapper: GameBoyMapper) -> &'static str {
    match mapper {
        GameBoyMapper::RomOnly => "rom-only",
        GameBoyMapper::Mbc1 => "mbc1",
        GameBoyMapper::Mbc5 => "mbc5",
    }
}

fn game_boy_rom_size_code(rom_banks: usize) -> Result<u8, String> {
    match rom_banks {
        2 | 4 | 8 | 16 | 32 | 64 | 128 | 256 | 512 => Ok(rom_banks.trailing_zeros() as u8 - 1),
        _ => Err(format!("unsupported Game Boy ROM bank count {rom_banks}")),
    }
}

fn game_boy_cartridge_header(config: &GameBoyConfig) -> Result<(u8, u8), String> {
    let ram_size_code = match config.ram_banks {
        0 => 0x00,
        1 => 0x02,
        4 => 0x03,
        8 => 0x05,
        16 => 0x04,
        _ => return Err("Game Boy RAM bank count must be one of 0, 1, 4, 8, or 16".to_owned()),
    };
    if config.battery && config.ram_banks == 0 {
        return Err(
            "Game Boy battery-backed cartridges require at least one external RAM bank".to_owned(),
        );
    }
    if config.mapper == GameBoyMapper::Mbc1 && config.ram_banks > 4 {
        return Err("Game Boy MBC1 cartridges support at most four external RAM banks".to_owned());
    }
    if config.mapper == GameBoyMapper::Mbc5 && config.rumble && config.ram_banks > 8 {
        return Err(
            "Game Boy MBC5 rumble cartridges support at most eight external RAM banks".to_owned(),
        );
    }
    let cartridge_type = match config.mapper {
        GameBoyMapper::RomOnly if config.ram_banks == 0 && !config.battery && !config.rumble => {
            0x00
        }
        GameBoyMapper::RomOnly => {
            return Err(
                "Game Boy ROM-only cartridges cannot declare RAM, battery, or rumble".to_owned(),
            );
        }
        GameBoyMapper::Mbc1 if config.rumble => {
            return Err("Game Boy MBC1 cartridges do not support rumble".to_owned());
        }
        GameBoyMapper::Mbc1 if config.ram_banks == 0 => 0x01,
        GameBoyMapper::Mbc1 if config.battery => 0x03,
        GameBoyMapper::Mbc1 => 0x02,
        GameBoyMapper::Mbc5 if config.rumble && config.ram_banks == 0 => 0x1C,
        GameBoyMapper::Mbc5 if config.rumble && config.battery => 0x1E,
        GameBoyMapper::Mbc5 if config.rumble => 0x1D,
        GameBoyMapper::Mbc5 if config.ram_banks == 0 => 0x19,
        GameBoyMapper::Mbc5 if config.battery => 0x1B,
        GameBoyMapper::Mbc5 => 0x1A,
    };
    Ok((cartridge_type, ram_size_code))
}

#[cfg(test)]
mod game_boy_banking_tests {
    use super::*;

    #[test]
    fn mbc1_payload_banks_skip_unselectable_multiples_of_32() {
        let banks = game_boy_payload_banks(GameBoyMapper::Mbc1, 31).unwrap();
        assert_eq!(banks[..30], (2..=31).collect::<Vec<_>>());
        assert_eq!(banks[30], 33);
    }

    #[test]
    fn mbc5_rumble_header_describes_rom_ram_and_battery() {
        let config = GameBoyConfig {
            mapper: GameBoyMapper::Mbc5,
            rom_banks: Some(8),
            ram_banks: 4,
            battery: true,
            rumble: true,
            bank_files: Vec::new(),
        };
        assert_eq!(game_boy_cartridge_header(&config), Ok((0x1E, 0x03)));
        assert_eq!(game_boy_rom_size_code(8), Ok(0x02));
    }
}

fn ti8xp_bytes(
    settings: &BuildSettings,
    output_path: Option<&Path>,
    code: &[u8],
) -> Result<Vec<u8>, String> {
    let name = ti8xp_variable_name(settings, output_path)?;
    let mut program = ti8xp_payload_prefix(settings)?.to_vec();
    program.extend_from_slice(code);
    let program_len = u16::try_from(program.len())
        .map_err(|_| "TI .8xp program exceeds 65535 bytes".to_owned())?;

    // TI program variables contain a length-prefixed token stream. The outer
    // variable data length includes this inner length word.
    let payload_len = program_len
        .checked_add(2)
        .ok_or_else(|| "TI .8xp payload exceeds 65535 bytes".to_owned())?;

    let mut data = Vec::new();
    push16_le(&mut data, 13); // variable-entry header length
    push16_le(&mut data, payload_len);
    data.push(0x06); // protected program
    data.extend_from_slice(&name);
    data.push(0x00); // version
    data.push(0x00); // RAM/unarchived flag
    push16_le(&mut data, payload_len);
    push16_le(&mut data, program_len);
    data.extend_from_slice(&program);

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
    if settings.target.triple.value == "zxspectrum-z80-128k" {
        return zx_spectrum_128k_tap_bytes(settings, output_path, code);
    }
    let load = u16::try_from(settings.layout.load.get())
        .map_err(|_| "ZX Spectrum load address exceeds 16-bit address space".to_owned())?;
    let entry = u16::try_from(settings.layout.entry.get())
        .map_err(|_| "ZX Spectrum entry address exceeds 16-bit address space".to_owned())?;
    let ram_top = load
        .checked_sub(1)
        .ok_or_else(|| "ZX Spectrum CODE load address must be above zero".to_owned())?;
    let length = u16::try_from(code.len())
        .map_err(|_| "ZX Spectrum CODE block exceeds 65535 bytes".to_owned())?;
    let name = zx_tap_name(settings, output_path);

    let mut loader = Vec::new();
    let mut clear = vec![0xfd, b' ']; // CLEAR
    push_zx_basic_integer(&mut clear, ram_top);
    push_zx_basic_line(&mut loader, 10, &clear)?;
    push_zx_basic_line(&mut loader, 20, &[0xef, b' ', b'"', b'"', b' ', 0xaf])?; // LOAD "" CODE
    let mut run = vec![0xf9, b' ', 0xc0, b' ']; // RANDOMIZE USR
    push_zx_basic_integer(&mut run, entry);
    push_zx_basic_line(&mut loader, 30, &run)?;
    let loader_length = u16::try_from(loader.len())
        .map_err(|_| "ZX Spectrum BASIC loader exceeds 65535 bytes".to_owned())?;

    let mut loader_header = Vec::with_capacity(17);
    loader_header.push(0); // BASIC program header
    loader_header.extend_from_slice(&name);
    loader_header.extend_from_slice(&loader_length.to_le_bytes());
    loader_header.extend_from_slice(&10u16.to_le_bytes()); // auto-start at line 10
    loader_header.extend_from_slice(&loader_length.to_le_bytes());

    let mut code_header = Vec::with_capacity(17);
    code_header.push(3); // CODE header
    code_header.extend_from_slice(&name);
    code_header.extend_from_slice(&length.to_le_bytes());
    code_header.extend_from_slice(&load.to_le_bytes());
    code_header.extend_from_slice(&0u16.to_le_bytes());

    let mut out =
        Vec::with_capacity(8 + loader_header.len() + loader.len() + code_header.len() + code.len());
    push_zx_tap_block(&mut out, 0x00, &loader_header)?;
    push_zx_tap_block(&mut out, 0xff, &loader)?;
    push_zx_tap_block(&mut out, 0x00, &code_header)?;
    push_zx_tap_block(&mut out, 0xff, code)?;
    Ok(out)
}

const ZX_128K_BANK_WINDOW: u16 = 0xC000;
const ZX_128K_BANK_SIZE: usize = 0x4000;
const ZX_128K_PAGE_PORT: u16 = 0x7FFD;

fn zx_spectrum_128k_tap_bytes(
    settings: &BuildSettings,
    output_path: Option<&Path>,
    code: &[u8],
) -> Result<Vec<u8>, String> {
    let load = u16::try_from(settings.layout.load.get())
        .map_err(|_| "ZX Spectrum load address exceeds 16-bit address space".to_owned())?;
    let entry = u16::try_from(settings.layout.entry.get())
        .map_err(|_| "ZX Spectrum entry address exceeds 16-bit address space".to_owned())?;
    if load != 0x8000 || entry < load || entry > 0xBFFF {
        return Err(
            "the `zxspectrum-z80-128k` target requires resident code and its entry point in 0x8000..0xBFFF"
                .to_owned(),
        );
    }
    let code_length = u16::try_from(code.len())
        .map_err(|_| "ZX Spectrum resident CODE block exceeds 65535 bytes".to_owned())?;
    let mut banks = settings.zxspectrum.clone().unwrap_or_default().banks;
    banks.sort_by_key(|bank| bank.page);
    let bank_payloads = banks
        .iter()
        .map(|bank| {
            let bytes = fs::read(&bank.file).map_err(|error| {
                format!(
                    "failed to read ZX Spectrum RAM page {} payload `{}`: {error}",
                    bank.page,
                    bank.file.display()
                )
            })?;
            if bytes.len() > ZX_128K_BANK_SIZE {
                return Err(format!(
                    "ZX Spectrum RAM page {} payload `{}` is {} bytes, but a pageable RAM bank holds at most {} bytes",
                    bank.page,
                    bank.file.display(),
                    bytes.len(),
                    ZX_128K_BANK_SIZE
                ));
            }
            Ok(bytes)
        })
        .collect::<Result<Vec<_>, String>>()?;

    let mut loader = Vec::new();
    let mut clear = vec![0xfd, b' ']; // CLEAR
    push_zx_basic_integer(&mut clear, 0x5FFF);
    push_zx_basic_line(&mut loader, 10, &clear)?;
    push_zx_basic_line(&mut loader, 20, &[0xef, b' ', b'"', b'"', b' ', 0xaf])?; // LOAD "" CODE
    let mut line = 30u16;
    for bank in &banks {
        let mut page = vec![0xdf, b' ']; // OUT
        push_zx_basic_integer(&mut page, ZX_128K_PAGE_PORT);
        page.extend_from_slice(b", ");
        push_zx_basic_integer(&mut page, u16::from(bank.page));
        push_zx_basic_line(&mut loader, line, &page)?;
        line = line
            .checked_add(10)
            .ok_or_else(|| "ZX Spectrum BASIC loader line number overflow".to_owned())?;
        push_zx_basic_line(&mut loader, line, &[0xef, b' ', b'"', b'"', b' ', 0xaf])?; // LOAD "" CODE
        line = line
            .checked_add(10)
            .ok_or_else(|| "ZX Spectrum BASIC loader line number overflow".to_owned())?;
    }
    let mut restore_page_zero = vec![0xdf, b' ']; // OUT
    push_zx_basic_integer(&mut restore_page_zero, ZX_128K_PAGE_PORT);
    restore_page_zero.extend_from_slice(b", ");
    push_zx_basic_integer(&mut restore_page_zero, 0);
    push_zx_basic_line(&mut loader, line, &restore_page_zero)?;
    line = line
        .checked_add(10)
        .ok_or_else(|| "ZX Spectrum BASIC loader line number overflow".to_owned())?;
    let mut run = vec![0xf9, b' ', 0xc0, b' ']; // RANDOMIZE USR
    push_zx_basic_integer(&mut run, entry);
    push_zx_basic_line(&mut loader, line, &run)?;

    let loader_length = u16::try_from(loader.len())
        .map_err(|_| "ZX Spectrum BASIC loader exceeds 65535 bytes".to_owned())?;
    let name = zx_tap_name(settings, output_path);
    let mut loader_header = Vec::with_capacity(17);
    loader_header.push(0); // BASIC program header
    loader_header.extend_from_slice(&name);
    loader_header.extend_from_slice(&loader_length.to_le_bytes());
    loader_header.extend_from_slice(&10u16.to_le_bytes()); // auto-start at line 10
    loader_header.extend_from_slice(&loader_length.to_le_bytes());

    let mut out = Vec::new();
    push_zx_tap_block(&mut out, 0x00, &loader_header)?;
    push_zx_tap_block(&mut out, 0xff, &loader)?;
    push_zx_code_block(&mut out, name, load, code_length, code)?;
    for (bank, payload) in banks.iter().zip(&bank_payloads) {
        let length = u16::try_from(payload.len())
            .map_err(|_| "ZX Spectrum RAM bank payload exceeds 65535 bytes".to_owned())?;
        let name = zx_tap_name_for(
            bank.name
                .as_deref()
                .unwrap_or(&format!("BANK{}", bank.page)),
        );
        push_zx_code_block(&mut out, name, ZX_128K_BANK_WINDOW, length, payload)?;
    }
    Ok(out)
}

fn push_zx_code_block(
    out: &mut Vec<u8>,
    name: [u8; 10],
    load: u16,
    length: u16,
    payload: &[u8],
) -> Result<(), String> {
    let mut header = Vec::with_capacity(17);
    header.push(3); // CODE header
    header.extend_from_slice(&name);
    header.extend_from_slice(&length.to_le_bytes());
    header.extend_from_slice(&load.to_le_bytes());
    header.extend_from_slice(&0u16.to_le_bytes());
    push_zx_tap_block(out, 0x00, &header)?;
    push_zx_tap_block(out, 0xff, payload)
}

fn push_zx_basic_integer(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(value.to_string().as_bytes());
    out.push(0x0e); // numeric literal marker
    out.extend_from_slice(&[0x00, 0x00, value as u8, (value >> 8) as u8, 0x00]);
}

fn push_zx_basic_line(out: &mut Vec<u8>, number: u16, body: &[u8]) -> Result<(), String> {
    let length = body
        .len()
        .checked_add(1)
        .and_then(|length| u16::try_from(length).ok())
        .ok_or_else(|| "ZX Spectrum BASIC line exceeds 65535 bytes".to_owned())?;
    out.extend_from_slice(&number.to_be_bytes());
    out.extend_from_slice(&length.to_le_bytes());
    out.extend_from_slice(body);
    out.push(0x0d);
    Ok(())
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
    zx_tap_name_for(raw)
}

fn zx_tap_name_for(raw: &str) -> [u8; 10] {
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
    _output_path: Option<&Path>,
    _code: &[u8],
) -> Result<Vec<u8>, String> {
    let format = match settings.output_format {
        OutputFormat::Ti8ek => ".8ek",
        OutputFormat::Ti8xk => ".8xk",
        _ => unreachable!("non-app output format"),
    };
    Err(format!(
        "TI flash application output `{format}` is not implemented; use `.8xp` protected-program output"
    ))
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
        assembly_options_from_layout_and_program(
            &settings.layout,
            &program,
            settings.target.triple.cpu,
            &settings.target.triple.value,
            options.debug_comments,
            settings.default_sdk_symbols,
            settings.gameboy_banking,
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
                    &settings.target.triple.value,
                    options.command.debug_comments,
                    settings.default_sdk_symbols,
                    settings.gameboy_banking,
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
    let assembly = emit_source_assembly(
        &program,
        assembly_options_from_layout_and_program(
            &settings.layout,
            &program,
            settings.target.triple.cpu,
            &settings.target.triple.value,
            options.debug_comments,
            settings.default_sdk_symbols,
            settings.gameboy_banking,
        )?,
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
    let assembled = ezra::vm::assemble_subset_with_options_at(
        AssemblerCpu::from(settings.target.triple.cpu),
        assembly,
        settings.layout.entry.get(),
        &assembly_source_options(source_path, &settings.layout),
    )
    .map_err(|error| {
        error
            .with_location_if_missing(source_location.clone())
            .to_string()
    })?;
    validate_assembled_section_fit(
        &settings.layout,
        ".text",
        settings.layout.entry.get(),
        assembled.bytes.len(),
    )
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
        assembly_options_from_layout_and_program(
            &settings.layout,
            &program,
            settings.target.triple.cpu,
            &settings.target.triple.value,
            options.debug_comments,
            settings.default_sdk_symbols,
            settings.gameboy_banking,
        )?,
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

fn assembly_options_from_layout(
    layout: &Layout,
    cpu: CpuFamily,
    target: &str,
    debug_comments: bool,
    default_sdk_symbols: bool,
) -> AssemblyOptions {
    AssemblyOptions {
        cpu,
        debug_comments,
        default_sdk_symbols,
        dos_executable: target == ezra::target::MSDOS_COM_I8086_TARGET,
        mos_executable: layout.name == "agon_light_mos",
        c64_executable: matches!(layout.name.as_str(), "commodore64_6502" | "commodore64_crt"),
        ti_os_executable: layout.name.starts_with("ti83-z80")
            || layout.name.starts_with("ti83plus-z80")
            || layout.name.starts_with("ti84-z80")
            || layout.name.starts_with("ti84plus-z80")
            || layout.name.starts_with("ti84plusce-ez80")
            || layout.name.starts_with("ti83premiumce-ez80"),
        arduboy_executable: target.starts_with("arduboy-"),
        gameboy_banking: None,
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
    target: &str,
    debug_comments: bool,
    default_sdk_symbols: bool,
    gameboy_banking: Option<GameBoyBankingOptions>,
) -> Result<AssemblyOptions, String> {
    let mut options =
        assembly_options_from_layout(layout, cpu, target, debug_comments, default_sdk_symbols);
    options.gameboy_banking = gameboy_banking;
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
        let info = arduboy_info_json(&config, "pocket-game.hex");
        let zip = stored_zip_bytes(&[
            ("info.json", info.as_bytes()),
            ("pocket-game.hex", b":00000001FF\n"),
        ])
        .unwrap();
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
            assert_eq!(read_u32(zip, offset + 14), zip_crc32(&data));
            entries.insert(name, data);
            offset = data_start + size;
        }
        assert_eq!(read_u32(zip, offset), CENTRAL_DIRECTORY_HEADER);
        let end_offset = zip.len() - 22;
        assert_eq!(read_u32(zip, end_offset), END_OF_CENTRAL_DIRECTORY);
        assert_eq!(read_u16(zip, end_offset + 10) as usize, entries.len());
        entries
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
