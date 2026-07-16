#![cfg(feature = "no-std")]

use ezra::api::{CompileRequest, Workspace, WorkspaceFile, build_workspace};

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
