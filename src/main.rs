use std::{env, fs, path::PathBuf, process::ExitCode};

use ezra::{
    asm::emit_ez80_assembly,
    cart::CartridgeHeader,
    compile::{CompileOptions, check_source, load_program},
    layout::Layout,
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
        Some("build") => build(args.get(1).ok_or_else(|| usage())?),
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

fn build(path: &str) -> Result<(), String> {
    let outputs = build_source(path)?;
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

fn build_source(path: &str) -> Result<BuildOutputs, String> {
    let source_path = PathBuf::from(path);
    let program = load_program(&source_path).map_err(|error| error.to_string())?;
    let assembly = emit_ez80_assembly(&program).map_err(|error| error.to_string())?;
    let layout = Layout::ezra_default();
    if let Err(errors) = layout.validate() {
        let message = errors
            .into_iter()
            .map(|error| error.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        return Err(format!("default layout is invalid:\n{message}"));
    }

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

    fs::write(&asm_path, assembly)
        .map_err(|error| format!("failed to write {}: {error}", asm_path.display()))?;
    fs::write(&map_path, layout.map_summary())
        .map_err(|error| format!("failed to write {}: {error}", map_path.display()))?;
    fs::write(&cart_path, CartridgeHeader::default().serialize())
        .map_err(|error| format!("failed to write {}: {error}", cart_path.display()))?;

    Ok(BuildOutputs {
        asm: asm_path,
        map: map_path,
        cart: cart_path,
    })
}

fn test_source(path: &str) -> Result<(), String> {
    let source_path = PathBuf::from(path);
    let program = load_program(&source_path).map_err(|error| error.to_string())?;
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
    let program = load_program(&source_path).map_err(|error| error.to_string())?;
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
    "usage: ezra <command>\n\ncommands:\n  check <file.ezra>     parse and validate a source file\n  build <file.ezra>     write .asm, .map, and .ezra.cart scaffold artifacts\n  emit-asm <file.ezra>  emit readable eZ80 assembly for the supported subset\n  test <file.ezra>      emit and run the supported subset on the ez80 VM\n  layout                print the default EZRA layout summary\n  header                print the default 64-byte cartridge header".to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_writes_scaffold_artifacts() {
        let root = std::env::temp_dir().join(format!(
            "ezra_build_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(root.join("lib")).unwrap();
        let source_path = root.join("game.ezra");
        std::fs::write(
            root.join("lib/math.ezra"),
            "fn add_one(v: u8) -> u8 { return v + 1 }\n",
        )
        .unwrap();
        std::fs::write(
            &source_path,
            "import lib.math\nfn main() { let x: u8 = add_one(4); test.pass() }\n",
        )
        .unwrap();

        let outputs = build_source(source_path.to_str().unwrap()).unwrap();
        let asm = std::fs::read_to_string(&outputs.asm).unwrap();
        let map = std::fs::read_to_string(&outputs.map).unwrap();
        let cart = std::fs::read(&outputs.cart).unwrap();

        assert!(asm.contains("__ezra_start:"));
        assert!(asm.contains("_add_one:"));
        assert!(map.contains(".header"));
        assert_eq!(&cart[0..4], b"EZRA");
        assert_eq!(cart.len(), 64);

        let _ = std::fs::remove_dir_all(root);
    }
}
