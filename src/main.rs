use std::{env, fs, path::PathBuf, process::ExitCode};

use ezra::{
    asm::emit_ez80_assembly,
    cart::CartridgeHeader,
    compile::{CompileOptions, check_source},
    layout::Layout,
    parser::parse_program,
    vm::run_assembly_test,
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
        Some("emit-asm") => emit_asm(args.get(1).ok_or_else(|| usage())?),
        Some("test") => test_source(args.get(1).ok_or_else(|| usage())?),
        Some("layout") => print_layout(),
        Some("header") => print_header(),
        Some("-h" | "--help") | None => {
            print_usage();
            Ok(())
        }
        Some(command) => Err(format!("unknown command `{command}`\n{}", usage())),
    }
}

fn test_source(path: &str) -> Result<(), String> {
    let source_path = PathBuf::from(path);
    let source = fs::read_to_string(&source_path)
        .map_err(|error| format!("failed to read {}: {error}", source_path.display()))?;
    let program = parse_program(&source_path, &source).map_err(|error| error.to_string())?;
    let assembly = emit_ez80_assembly(&program).map_err(|error| error.to_string())?;
    let run = run_assembly_test(&assembly, 1_000_000).map_err(|error| error.to_string())?;
    if !run.halted {
        return Err(format!(
            "test timed out after {} instructions",
            run.instructions
        ));
    }
    if run.result_code != 0 {
        return Err(format!("test failed with code {}", run.result_code));
    }
    println!("ok: test passed in {} instructions", run.instructions);
    Ok(())
}

fn emit_asm(path: &str) -> Result<(), String> {
    let source_path = PathBuf::from(path);
    let source = fs::read_to_string(&source_path)
        .map_err(|error| format!("failed to read {}: {error}", source_path.display()))?;
    let program = parse_program(&source_path, &source).map_err(|error| error.to_string())?;
    let assembly = emit_ez80_assembly(&program).map_err(|error| error.to_string())?;
    print!("{assembly}");
    Ok(())
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

fn print_layout() -> Result<(), String> {
    let layout = Layout::ezra_default();
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
    "usage: ezra <command>\n\ncommands:\n  check <file.ezra>     parse and validate a source file\n  emit-asm <file.ezra>  emit readable eZ80 assembly for the supported subset\n  test <file.ezra>      emit and run the supported subset on the ez80 VM\n  layout                print the default EZRA layout summary\n  header                print the default 64-byte cartridge header".to_owned()
}
