#![cfg(feature = "no-std")]

#[cfg(feature = "z80")]
use ezra::api::compile_workspace_to_assembly;
use ezra::{
    api::{CompileRequest, Workspace, WorkspaceFile, build_workspace},
    ast::{Declaration, EmbedSource, Expr, Program},
};

fn materialized_embed_bytes(program: &Program, name: &str) -> Vec<u8> {
    let embed = program
        .declarations
        .iter()
        .find_map(|declaration| match declaration {
            Declaration::Embed(embed) if embed.name == name => Some(embed),
            _ => None,
        })
        .expect("materialized embed declaration");
    let EmbedSource::Bytes(values) = &embed.source else {
        panic!("workspace file embed was not materialized");
    };
    values
        .iter()
        .map(|value| match value {
            Expr::Int(value) => *value as u8,
            _ => panic!("materialized workspace byte is not an integer"),
        })
        .collect()
}

#[cfg(feature = "z80")]
#[test]
fn builds_imported_z80_workspace_without_host_io() {
    let files = [
        WorkspaceFile::text(
            "src/main.ezra",
            "import math\nfn main() { let answer: u8 = math.ANSWER }\n",
        ),
        WorkspaceFile::text("src/math.ezra", "pub const ANSWER: u8 = 42\n"),
    ];
    let build = build_workspace(
        &Workspace::new(&files),
        "src/main.ezra",
        &CompileRequest::new("src/main.ezra", "cpm-2.2-z80"),
    )
    .expect("virtual no-std workspace should build");

    assert!(build.report.has_main);
    assert!(build.assembly.contains("_main:"));
    assert!(!build.machine_code.is_empty());
    assert_eq!(build.executable, build.machine_code);
    assert_eq!(build.executable_extension, "com");
}

#[cfg(feature = "z80")]
#[test]
fn materializes_root_relative_workspace_assets() {
    let files = [
        WorkspaceFile::text(
            "src/main.ezra",
            "embed blob: bytes = file(\"assets/blob.bin\")\nfn main() {}\n",
        ),
        WorkspaceFile::new("src/assets/blob.bin", &[0xA5, 0x00, 0xFF]),
    ];
    let build = build_workspace(
        &Workspace::new(&files),
        "src/main.ezra",
        &CompileRequest::new("src/main.ezra", "cpm-2.2-z80"),
    )
    .expect("root-relative workspace asset should build");

    assert_eq!(
        materialized_embed_bytes(&build.program, "blob"),
        [0xA5, 0x00, 0xFF]
    );
}

#[cfg(feature = "z80")]
#[test]
fn reports_missing_virtual_workspace_assets() {
    let files = [WorkspaceFile::text(
        "src/main.ezra",
        "embed blob: bytes = file(\"assets/missing.bin\")\nfn main() {}\n",
    )];
    let error = compile_workspace_to_assembly(
        &Workspace::new(&files),
        "src/main.ezra",
        &CompileRequest::new("src/main.ezra", "cpm-2.2-z80"),
    )
    .unwrap_err();

    assert_eq!(
        error.message,
        "virtual workspace asset `assets/missing.bin` referenced from `src/main.ezra` was not found (resolved as `src/assets/missing.bin`)"
    );
}

#[cfg(feature = "z80")]
#[test]
fn builds_and_packages_ez80_workspace_without_host_io() {
    let files = [WorkspaceFile::text("main.ezra", "fn main() {}\n")];
    let build = build_workspace(
        &Workspace::new(&files),
        "main.ezra",
        &CompileRequest::new("main.ezra", "agonlight-mos-ez80"),
    )
    .expect("virtual no-std eZ80 workspace should build");

    assert!(!build.machine_code.is_empty());
    assert_eq!(&build.executable[64..69], b"MOS\0\x01");
    assert_eq!(build.executable_extension, "bin");
}

#[cfg(feature = "mos6502")]
#[test]
fn materializes_imported_module_relative_workspace_assets_for_c64() {
    let files = [
        WorkspaceFile::text("src/main.ezra", "import lib.media\nfn main() {}\n"),
        WorkspaceFile::text(
            "src/lib/media.ezra",
            "pub embed sprite: bytes = file(\"assets/sprite.bin\")\n",
        ),
        WorkspaceFile::new("src/lib/assets/sprite.bin", &[0xDE, 0xAD]),
    ];
    let build = build_workspace(
        &Workspace::new(&files),
        "src/main.ezra",
        &CompileRequest::new("src/main.ezra", "commodore64-6502"),
    )
    .expect("imported module-relative C64 asset should build");

    assert_eq!(
        materialized_embed_bytes(&build.program, "sprite"),
        [0xDE, 0xAD]
    );
    assert_eq!(&build.executable[..2], &[0x01, 0x08]);
}

#[cfg(feature = "mos6502")]
#[test]
fn builds_and_packages_c64_workspace_without_host_io() {
    let files = [WorkspaceFile::text(
        "src/main.ezra",
        "fn main() { let border: u8 = 6 }\n",
    )];
    let build = build_workspace(
        &Workspace::new(&files),
        "src/main.ezra",
        &CompileRequest::new("src/main.ezra", "commodore64-6502"),
    )
    .expect("virtual no-std C64 workspace should build");

    assert!(build.report.has_main);
    assert!(build.assembly.contains("_main:"));
    assert!(!build.machine_code.is_empty());
    assert_eq!(&build.executable[..2], &[0x01, 0x08]);
    assert_eq!(build.executable_extension, "prg");
}
