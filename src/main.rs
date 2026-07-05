use std::{env, fs, path::PathBuf, process::ExitCode};

use ezra::{
    asm::{AssemblyOptions, emit_ez80_assembly, emit_ez80_assembly_with_options},
    cart::{CartridgeHeader, build_cartridge_map, build_cartridge_with_layout_code_and_symbols},
    compile::{CompileOptions, check_source, load_program},
    layout::{Layout, parse_layout},
    vm::{TestRunOptions, assemble_ez80_subset_with_symbols_at, run_assembly_test_with_options},
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
        Some("check") => check(args.get(1).ok_or_else(|| usage())?),
        Some("build") => {
            let options = CommandOptions::parse(&args[1..])?;
            build(&options)
        }
        Some("emit-asm") => {
            let options = CommandOptions::parse(&args[1..])?;
            emit_asm(&options)
        }
        Some("test") => test_source(args.get(1).ok_or_else(|| usage())?),
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
    layout_path: Option<String>,
}

impl CommandOptions {
    fn parse(args: &[String]) -> Result<Self, String> {
        let mut path = None;
        let mut debug_comments = false;
        let mut layout_path = None;
        let mut iter = args.iter();
        while let Some(arg) = iter.next() {
            match arg.as_str() {
                "--debug-comments" => debug_comments = true,
                "--layout" => {
                    let value = iter.next().ok_or_else(usage)?;
                    layout_path = Some(value.clone());
                }
                _ if path.is_none() => path = Some(arg.clone()),
                _ => return Err(usage()),
            }
        }
        Ok(Self {
            path: path.ok_or_else(usage)?,
            debug_comments,
            layout_path,
        })
    }
}

fn build(options: &CommandOptions) -> Result<(), String> {
    let outputs = build_source_with_command_options(options)?;
    println!("wrote {}", outputs.asm.display());
    println!("wrote {}", outputs.map.display());
    println!("wrote {}", outputs.cart.display());
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct BuildOutputs {
    asm: PathBuf,
    map: PathBuf,
    cart: PathBuf,
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
        layout_path: None,
    })
}

fn build_source_with_command_options(options: &CommandOptions) -> Result<BuildOutputs, String> {
    let source_path = PathBuf::from(&options.path);
    let program = load_program(&source_path).map_err(|error| error.to_string())?;
    let layout = load_layout(options.layout_path.as_deref())?;
    if let Err(errors) = layout.validate() {
        let message = errors
            .into_iter()
            .map(|error| error.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        return Err(format!("layout is invalid:\n{message}"));
    }
    let assembly = emit_ez80_assembly_with_options(
        &program,
        AssemblyOptions {
            debug_comments: options.debug_comments,
            stack_top: layout.stack,
        },
    )
    .map_err(|error| error.to_string())?;

    let stem = source_path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .ok_or_else(|| format!("source path `{}` has no file stem", source_path.display()))?;
    let dir = source_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    let asm_path = dir.join(format!("{stem}.asm"));
    let map_path = dir.join(format!("{stem}.map"));
    let cart_path = dir.join(format!("{stem}.ezra.cart"));

    let assembled = assemble_ez80_subset_with_symbols_at(&assembly, layout.entry.get())
        .map_err(|error| error.to_string())?;
    let map = build_cartridge_map(&program, &layout, assembled.bytes.len(), &assembled.symbols)
        .map_err(|error| error.to_string())?;
    let cart = build_cartridge_with_layout_code_and_symbols(
        &program,
        &layout,
        &assembled.bytes,
        &assembled.symbols,
    )
    .map_err(|error| error.to_string())?;

    fs::write(&asm_path, &assembly)
        .map_err(|error| format!("failed to write {}: {error}", asm_path.display()))?;
    fs::write(&map_path, map)
        .map_err(|error| format!("failed to write {}: {error}", map_path.display()))?;
    fs::write(&cart_path, cart)
        .map_err(|error| format!("failed to write {}: {error}", cart_path.display()))?;

    Ok(BuildOutputs {
        asm: asm_path,
        map: map_path,
        cart: cart_path,
    })
}

fn test_source(path: &str) -> Result<(), String> {
    let source_path = PathBuf::from(path);
    let source = fs::read_to_string(&source_path)
        .map_err(|error| format!("failed to read {}: {error}", source_path.display()))?;
    let metadata = parse_test_metadata(&source)?;
    let program = load_program(&source_path).map_err(|error| error.to_string())?;
    let assembly = emit_ez80_assembly(&program).map_err(|error| error.to_string())?;
    let run = run_assembly_test_with_options(
        &assembly,
        &TestRunOptions {
            instruction_budget: 1_000_000,
            initial_ports: metadata.initial_ports,
        },
    )
    .map_err(|error| error.to_string())?;
    if !run.halted {
        return Err(match run.failure {
            Some(ezra::vm::TestRunFailure::Timeout) | None => {
                format!("test timed out after {} instructions", run.instructions)
            }
            Some(ezra::vm::TestRunFailure::ExecutionOutsideLoadedProgram { pc }) => format!(
                "test executed outside loaded program at 0x{pc:06X} after {} instructions",
                run.instructions
            ),
            Some(ezra::vm::TestRunFailure::IllegalInstruction { pc }) => format!(
                "test hit an illegal instruction at 0x{pc:06X} after {} instructions",
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
}

fn parse_test_metadata(source: &str) -> Result<TestMetadata, String> {
    let mut initial_ports = Vec::new();
    for (index, line) in source.lines().enumerate() {
        let Some(comment) = line.trim_start().strip_prefix("//") else {
            continue;
        };
        let comment = comment.trim_start();
        let rest = if let Some(rest) = comment.strip_prefix("test:") {
            rest.trim()
        } else if comment.starts_with("port") {
            comment
        } else {
            continue;
        };
        let Some(rest) = rest.strip_prefix("port") else {
            return Err(format!("invalid test metadata on line {}", index + 1));
        };
        let (port, value) = rest
            .trim()
            .split_once('=')
            .ok_or_else(|| format!("invalid test port metadata on line {}", index + 1))?;
        let port = parse_metadata_u8(port.trim())
            .map_err(|error| format!("invalid test port on line {}: {error}", index + 1))?;
        let value = parse_metadata_u8(value.trim())
            .map_err(|error| format!("invalid test port value on line {}: {error}", index + 1))?;
        initial_ports.push((port, value));
    }
    Ok(TestMetadata { initial_ports })
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

fn emit_asm(options: &CommandOptions) -> Result<(), String> {
    let assembly = emit_assembly_with_command_options(options)?;
    print!("{assembly}");
    Ok(())
}

fn emit_assembly_with_command_options(options: &CommandOptions) -> Result<String, String> {
    let source_path = PathBuf::from(&options.path);
    let program = load_program(&source_path).map_err(|error| error.to_string())?;
    let layout = load_layout(options.layout_path.as_deref())?;
    if let Err(errors) = layout.validate() {
        let message = errors
            .into_iter()
            .map(|error| error.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        return Err(format!("layout is invalid:\n{message}"));
    }
    emit_ez80_assembly_with_options(
        &program,
        AssemblyOptions {
            debug_comments: options.debug_comments,
            stack_top: layout.stack,
        },
    )
    .map_err(|error| error.to_string())
}

fn check(path: &str) -> Result<(), String> {
    let source_path = PathBuf::from(path);
    let source = fs::read_to_string(&source_path)
        .map_err(|error| format!("failed to read {}: {error}", source_path.display()))?;
    let options = CompileOptions {
        source: source_path,
        debug_comments: false,
    };
    let report = check_source(&source, &options).map_err(|error| error.to_string())?;

    println!(
        "ok: {} imports, {} declarations, main present",
        report.imports, report.declarations
    );
    Ok(())
}

fn print_layout(path: Option<&str>) -> Result<(), String> {
    let layout = load_layout(path)?;
    if let Err(errors) = layout.validate() {
        for error in errors {
            eprintln!("error: {error}");
        }
        return Err("default layout is invalid".to_owned());
    }

    println!("layout {}", layout.name);
    println!("load  {}", layout.load);
    println!("entry {}", layout.entry);
    println!("stack {}", layout.stack);
    println!();
    print!("{}", layout.map_summary());
    Ok(())
}

fn load_layout(path: Option<&str>) -> Result<Layout, String> {
    let Some(path) = path else {
        return Ok(Layout::ezra_default());
    };
    let source =
        fs::read_to_string(path).map_err(|error| format!("failed to read {path}: {error}"))?;
    parse_layout(&source).map_err(|error| error.to_string())
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
    "usage: ezra <command>\n\ncommands:\n  check <file.ezra>                    parse and validate a source file\n  build [--debug-comments] [--layout <file.ezralayout>] <file.ezra>\n                                       write .asm, .map, and .ezra.cart artifacts\n  emit-asm [--debug-comments] [--layout <file.ezralayout>] <file.ezra>\n                                       emit readable eZ80 assembly\n  test <file.ezra>                     emit and run on the ez80 VM\n  layout [file.ezralayout]             print the default or custom EZRA layout summary\n  header                               print the default 64-byte cartridge header".to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ezra::target::{EZRA_ENTRY_ADDR, EZRA_LOAD_ADDR};

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
            &source_path,
            "import lib.math\nembed palette: bytes = bytes [0x11, 0x22]\nfn main() { let x: u8 = add_one(4); test.pass() }\n",
        )
        .unwrap();

        let outputs = build_source(source_path.to_str().unwrap()).unwrap();
        let asm = std::fs::read_to_string(&outputs.asm).unwrap();
        let map = std::fs::read_to_string(&outputs.map).unwrap();
        let cart = std::fs::read(&outputs.cart).unwrap();

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
        assert_eq!(&cart[0..4], b"EZRA");
        assert_eq!(read_addr24(&cart, 0x08), EZRA_ENTRY_ADDR.get());
        assert_eq!(&cart[64..69], &[0xF3, 0x31, 0x00, 0x00, 0xF0]);
        let layout_table = read_addr24(&cart, 0x1E);
        assert!(layout_table > EZRA_ENTRY_ADDR.get());
        let symbol_table = read_addr24(&cart, 0x24);
        assert!(symbol_table > layout_table);
        let symbol_offset = usize::try_from(symbol_table - EZRA_LOAD_ADDR.get()).unwrap();
        let symbols = std::str::from_utf8(&cart[symbol_offset..]).unwrap();
        assert!(
            symbols.contains("symbol __ezra_start 0x010040"),
            "{symbols}"
        );
        assert!(symbols.contains("symbol _main"), "{symbols}");
        assert!(read_addr24(&cart, 0x21) > read_addr24(&cart, 0x1E));
        assert!(cart.len() > 64);
        assert!(cart.ends_with(&[0x11, 0x22]));

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
    fn parses_test_port_metadata() {
        let metadata = parse_test_metadata(
            r#"
                // port 0x01 = 0x10
                // test: port 2 = 0b00100000
                fn main() { test.pass() }
            "#,
        )
        .unwrap();

        assert_eq!(metadata.initial_ports, vec![(0x01, 0x10), (0x02, 0x20)]);

        let error = parse_test_metadata("// port 0x100 = 0").unwrap_err();
        assert!(error.contains("outside u8 range"), "{error}");
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
    fn build_can_use_custom_layout_file() {
        let root = temp_root("custom_layout_build");
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
                    stack 0xEFFF00;

                    region code 0x020000..0x02FFFF read execute;
                    section .header -> code align 64;
                    section .text -> code align 16;

                    symbol EZRA_LOAD_ADDR = 0x020000;
                    symbol EZRA_ENTRY_ADDR = 0x020040;
                    symbol EZRA_STACK_TOP = 0xEFFF00;
                }
            "#,
        )
        .unwrap();

        let outputs = build_source_with_command_options(&CommandOptions {
            path: source_path.to_string_lossy().into_owned(),
            debug_comments: false,
            layout_path: Some(layout_path.to_string_lossy().into_owned()),
        })
        .unwrap();

        let map = std::fs::read_to_string(&outputs.map).unwrap();
        let asm = std::fs::read_to_string(&outputs.asm).unwrap();
        let cart = std::fs::read(&outputs.cart).unwrap();

        assert!(
            map.starts_with("section      start      end        size\n"),
            "{map}"
        );
        assert!(map.contains(".text        0x020040"), "{map}");
        assert!(asm.contains("    ld sp, EFFF00h"), "{asm}");
        assert_eq!(read_addr24(&cart, 0x08), 0x020040);
        assert_eq!(read_addr24(&cart, 0x0B), 0xEFFF00);
        let layout_table = read_addr24(&cart, 0x1E);
        assert!(layout_table > 0x020040);
        let layout_offset = usize::try_from(layout_table - 0x020000).unwrap();
        assert!(cart[layout_offset..].starts_with(b"layout custom\n"));
        assert_eq!(&cart[64..69], &[0xF3, 0x31, 0x00, 0xFF, 0xEF]);

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
                    section .header -> code align 64;
                    section .text -> code align 16;
                }
            "#,
        )
        .unwrap();

        let asm = emit_assembly_with_command_options(&CommandOptions {
            path: source_path.to_string_lossy().into_owned(),
            debug_comments: false,
            layout_path: Some(layout_path.to_string_lossy().into_owned()),
        })
        .unwrap();

        assert!(asm.contains("    ld sp, EFFE00h"), "{asm}");

        let _ = std::fs::remove_dir_all(root);
    }

    fn read_addr24(bytes: &[u8], offset: usize) -> u32 {
        u32::from(bytes[offset])
            | (u32::from(bytes[offset + 1]) << 8)
            | (u32::from(bytes[offset + 2]) << 16)
    }
}
