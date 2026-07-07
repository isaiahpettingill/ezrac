use std::{
    env, fs,
    path::{Path, PathBuf},
    process::ExitCode,
};

use ezra::{
    asm::{AssemblyOptions, emit_ez80_assembly_with_options},
    cart::{CartridgeHeader, build_cartridge_map, layout_section_bases},
    compile::{SdkResolver, load_program_with_sdk},
    diagnostic::SourceLocation,
    layout::{Layout, parse_layout},
    parser::parse_program,
    project::load_nearest_project_config,
    target::{
        Address24, EZRA_ASSET_BASE, EZRA_AUDIO_BASE, EZRA_RAM_BASE, EZRA_RODATA_BASE,
        EZRA_VRAM_BASE, OutputFormat, parse_output_format, resolve_target_profile,
    },
    vm::{TestRunOptions, assemble_ez80_subset_with_symbols_at, run_assembly_test_with_options_at},
};

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
            let options = CommandOptions::parse(&args[1..])?;
            build(&options)
        }
        Some("emit-asm") => {
            let options = CommandOptions::parse(&args[1..])?;
            emit_asm(&options)
        }
        Some("test") => {
            let options = CommandOptions::parse(&args[1..])?;
            test_source_with_command_options(&options)
        }
        Some("assemble") => {
            let options = AssembleOptions::parse(&args[1..])?;
            assemble_file(&options)
        }
        Some("layout") => print_layout(args.get(1).map(String::as_str)),
        Some("header") => print_header(),
        Some("-h" | "--help") | None => {
            print_usage();
            Ok(())
        }
        Some(command) => Err(format!("unknown command `{command}`\n{}", usage())),
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
struct AssembleOptions {
    path: String,
    output: Option<String>,
    base_addr: u32,
}

impl AssembleOptions {
    fn parse(args: &[String]) -> Result<Self, String> {
        let mut path = None;
        let mut output = None;
        let mut base_addr = 0x01_0000;
        let mut iter = args.iter();
        while let Some(arg) = iter.next() {
            match arg.as_str() {
                "--output" | "-o" => {
                    let value = iter.next().ok_or_else(usage)?;
                    output = Some(value.clone());
                }
                "--base" => {
                    let value = iter.next().ok_or_else(usage)?;
                    base_addr = parse_cli_u24(value)?;
                }
                _ if path.is_none() => path = Some(arg.clone()),
                _ => return Err(usage()),
            }
        }
        Ok(Self {
            path: path.ok_or_else(usage)?,
            output,
            base_addr,
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
    let assembled = assemble_ez80_subset_with_symbols_at(&source, options.base_addr)
        .map_err(|error| error.to_string())?;
    let output_path = options
        .output
        .as_ref()
        .map(PathBuf::from)
        .unwrap_or_else(|| source_path.with_extension("bin"));
    fs::write(&output_path, &assembled.bytes)
        .map_err(|error| format!("failed to write {}: {error}", output_path.display()))?;
    println!("wrote {}", output_path.display());
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct BuildSettings {
    sdk: SdkResolver,
    target: String,
    output_format: OutputFormat,
    layout: Layout,
    layout_path: Option<PathBuf>,
    default_sdk_symbols: bool,
    output_root: PathBuf,
    executable_name: Option<String>,
}

fn resolve_build_settings(
    options: &CommandOptions,
    source_path: &Path,
) -> Result<BuildSettings, String> {
    let project = load_nearest_project_config(source_path).map_err(|error| error.to_string())?;
    let target_name = options.target.as_deref().or_else(|| {
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
    let layout_path = options.layout_path.as_ref().map(PathBuf::from).or_else(|| {
        project
            .as_ref()
            .and_then(|project| project.layout_file.clone())
    });
    let layout = load_layout(layout_path.as_deref(), &target.triple.value)?;
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
    let executable_name = project.as_ref().and_then(|project| project.executable.clone());

    Ok(BuildSettings {
        sdk,
        target: target.triple.value,
        output_format,
        layout,
        layout_path,
        default_sdk_symbols: options.default_sdk_symbols,
        output_root,
        executable_name,
    })
}

fn build(options: &CommandOptions) -> Result<(), String> {
    let outputs = build_source_with_command_options(options)?;
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
    build_source_with_command_options(&CommandOptions {
        path: path.to_owned(),
        debug_comments,
        default_sdk_symbols: true,
        layout_path: None,
        target: None,
    })
}

fn build_source_with_command_options(options: &CommandOptions) -> Result<BuildOutputs, String> {
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
    let assembly = emit_ez80_assembly_with_options(
        &program,
        assembly_options_from_layout_and_program(
            &settings.layout,
            &program,
            options.debug_comments,
            settings.default_sdk_symbols,
        )?,
    )
    .map_err(|error| {
        error
            .with_location_if_missing(source_location.clone())
            .to_string()
    })?;

    let output_base = build_output_base_path(&settings, &source_path)?;
    let asm_path = output_base.with_extension("asm");
    let map_path = output_base.with_extension("map");
    let executable_path = output_base.with_extension(settings.output_format.extension());

    let assembled = assemble_ez80_subset_with_symbols_at(&assembly, settings.layout.entry.get())
        .map_err(|error| error.to_string())?;
    let map = build_cartridge_map(
        &program,
        &settings.layout,
        assembled.bytes.len(),
        &assembled.symbols,
    )
    .map_err(|error| {
        error
            .with_location_if_missing(source_location.clone())
            .to_string()
    })?;
    if let Some(parent) = output_base.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
    }
    fs::write(&asm_path, &assembly)
        .map_err(|error| format!("failed to write {}: {error}", asm_path.display()))?;
    fs::write(&map_path, map)
        .map_err(|error| format!("failed to write {}: {error}", map_path.display()))?;
    let executable = build_executable_bytes(&settings, &assembled.bytes)?;
    fs::write(&executable_path, executable)
        .map_err(|error| format!("failed to write {}: {error}", executable_path.display()))?;

    Ok(BuildOutputs {
        asm: asm_path,
        map: map_path,
        executable: executable_path,
    })
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
        .strip_prefix(settings.output_root.parent().unwrap_or_else(|| Path::new(".")))
        .unwrap_or(source_parent);
    Ok(settings
        .output_root
        .join(&settings.target)
        .join(relative_parent)
        .join(source_stem))
}

fn build_executable_bytes(settings: &BuildSettings, code: &[u8]) -> Result<Vec<u8>, String> {
    if settings.target.starts_with("agonlight-mos-ez80") {
        return build_agon_mos_executable(settings.layout.entry.get(), code);
    }
    Ok(code.to_vec())
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

fn test_source_with_command_options(options: &CommandOptions) -> Result<(), String> {
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
    let assembly = emit_ez80_assembly_with_options(
        &program,
        assembly_options_from_layout_and_program(
            &settings.layout,
            &program,
            options.debug_comments,
            settings.default_sdk_symbols,
        )?,
    )
    .map_err(|error| error.with_location_if_missing(source_location).to_string())?;
    let run = run_assembly_test_with_options_at(
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
    if !run.halted {
        return Err(match run.failure {
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
        });
    }
    if run.result_code != 0 {
        return Err(format!("test failed with code {}", run.result_code));
    }
    println!("ok: test passed in {} instructions", run.instructions);
    Ok(())
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
    emit_ez80_assembly_with_options(
        &program,
        assembly_options_from_layout_and_program(
            &settings.layout,
            &program,
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
    emit_ez80_assembly_with_options(
        &program,
        assembly_options_from_layout_and_program(
            &settings.layout,
            &program,
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
    if target.starts_with("agonlight-mos-ez80") {
        Layout::agon_light_mos()
    } else {
        Layout::ezra_default()
    }
}

fn assembly_options_from_layout(
    layout: &Layout,
    debug_comments: bool,
    default_sdk_symbols: bool,
) -> AssemblyOptions {
    AssemblyOptions {
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
    debug_comments: bool,
    default_sdk_symbols: bool,
) -> Result<AssemblyOptions, String> {
    let mut options = assembly_options_from_layout(layout, debug_comments, default_sdk_symbols);
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

fn usage() -> String {
    "usage: ezra <command>\n\ncommands:\n  check [--target <triple>] [--debug-comments] [--no-default-sdk-symbols] [--layout <file.ezralayout>] <file.ezra>\n                                       parse and validate a source file\n  build [--target <triple>] [--debug-comments] [--no-default-sdk-symbols] [--layout <file.ezralayout>] <file.ezra>\n                                       write .asm, .map, and target executable artifacts\n  emit-asm [--target <triple>] [--debug-comments] [--no-default-sdk-symbols] [--layout <file.ezralayout>] <file.ezra>\n                                       emit readable target assembly\n  test [--target <triple>] [--debug-comments] [--no-default-sdk-symbols] [--layout <file.ezralayout>] <file.ezra>\n                                       emit and run on the target VM\n  assemble [--base <addr>] [--output <file.bin>] <file.asm>\n                                       assemble eZ80 assembly into a raw binary\n  layout [file.ezralayout]             print the default or custom EZRA layout summary\n  header                               print the default 64-byte cartridge header".to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(options.base_addr, 0x04_0000);
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
            base_addr: 0x04_0000,
        })
        .unwrap();

        assert_eq!(
            std::fs::read(&output_path).unwrap(),
            [0x3E, 0x42, 0x49, 0xD7, 0xC9]
        );

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
        let prefix = format!("{}:1:1:", source_path.display());

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
                    vdp.clear()
                    vdp.write(65)
                    vdp.emulator_exit(0)
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
    fn commands_reject_non_ez80_targets_for_now() {
        let root = temp_root("unsupported_target");
        std::fs::create_dir_all(&root).unwrap();
        let source_path = root.join("game.ezra");
        std::fs::write(&source_path, "fn main() { test.pass() }\n").unwrap();

        let error = check(&CommandOptions {
            path: source_path.to_string_lossy().into_owned(),
            debug_comments: false,
            default_sdk_symbols: true,
            layout_path: None,
            target: Some("zxspectrum-z80".to_owned()),
        })
        .unwrap_err();

        assert!(
            error.contains("only eZ80 codegen is implemented"),
            "{error}"
        );

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
    fn commands_reject_unimplemented_output_formats() {
        let root = temp_root("unsupported_output");
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

        let error = build_source(source_path.to_str().unwrap()).unwrap_err();
        assert!(error.contains("only `bin` is implemented"), "{error}");

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
