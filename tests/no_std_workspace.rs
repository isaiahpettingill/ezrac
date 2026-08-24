#![cfg(all(feature = "no-std", not(feature = "std")))]

#[cfg(any(feature = "i8086", feature = "z80"))]
use ezra::api::compile_workspace_to_assembly;
use ezra::{
    api::{CompileRequest, SdkLookupMode, Workspace, WorkspaceFile, build_workspace},
    ast::{Declaration, EmbedSource, Expr, Program},
    disk::{DiskFile, DiskFormat, DiskRequest, create_disk_image},
};
#[cfg(feature = "i8086")]
use ezra::{
    api::{
        build_generated_assembly, emit_optimized_i8086_program, optimize_i8086_program,
        resolve_workspace_program,
    },
    internal_ir::{ArtifactStage, ProgramArtifact},
};
#[cfg(feature = "z80")]
use ezra::{
    asm::{AssemblyPreprocessOptions, preprocess_assembly_workspace},
    target::AssemblerCpu,
    vm::assemble_program_at,
};

#[cfg(feature = "i8086")]
#[test]
fn alloc_only_external_dos_sdk_resolves_and_preserves_relative_embeds() {
    let root_source = "import dos.console\nfn main() { let value: u8 = console.EXTERNAL }\n";
    let module_source = "pub embed external_payload: bytes = file(\"assets/payload.bin\")\npub const EXTERNAL: u8 = 7\n";
    let files = [
        WorkspaceFile::text("src/main.ezra", root_source),
        WorkspaceFile::text("vendor-sdk/dos/console.ezra", module_source),
        WorkspaceFile::new("vendor-sdk/dos/assets/payload.bin", &[0xA5, 0x5A]),
    ];
    let mut request = CompileRequest::new("src/main.ezra", "msdos-com-i8086");
    request.sdk_roots.push("vendor-sdk".to_owned());
    request.sdk_mode = SdkLookupMode::ExternalOnly;

    let compilation =
        compile_workspace_to_assembly(&Workspace::new(&files), "src/main.ezra", &request).unwrap();
    assert!(compilation.report.has_main);
    assert!(compilation
        .program
        .declarations
        .iter()
        .any(|declaration| matches!(declaration, Declaration::Embed(embed) if embed.name == "dos.console.external_payload")));
}

#[cfg(feature = "z80")]
#[test]
fn alloc_only_external_cpm_and_agon_sdks_resolve_without_embedded_lookup() {
    for (target, module) in [
        ("cpm-2.2-z80", "cpm.console"),
        ("agonlight-mos-ez80", "agon.console"),
    ] {
        let root_source = format!(
            "import {module}\nfn main() {{ let value: u8 = {}.EXTERNAL }}\n",
            module.rsplit('.').next().unwrap()
        );
        let module_source = "pub const EXTERNAL: u8 = 7\n";
        let module_path = format!("vendor-sdk/{}.ezra", module.replace('.', "/"));
        let files = [
            WorkspaceFile::text("src/main.ezra", &root_source),
            WorkspaceFile::text(&module_path, module_source),
        ];
        let mut request = CompileRequest::new("src/main.ezra", target);
        request.sdk_roots.push("vendor-sdk".to_owned());
        request.sdk_mode = SdkLookupMode::ExternalOnly;

        let compilation =
            compile_workspace_to_assembly(&Workspace::new(&files), "src/main.ezra", &request)
                .unwrap();
        assert!(compilation.report.has_main);
    }
}

#[cfg(feature = "z80")]
#[test]
fn alloc_only_missing_sdk_import_uses_the_shared_diagnostic() {
    let files = [WorkspaceFile::text(
        "src/main.ezra",
        "import cpm.console\nfn main() {}\n",
    )];
    let mut request = CompileRequest::new("src/main.ezra", "cpm-2.2-z80");
    request.sdk_roots.push("vendor-sdk".to_owned());
    request.sdk_mode = SdkLookupMode::ExternalOnly;

    let error = compile_workspace_to_assembly(&Workspace::new(&files), "src/main.ezra", &request)
        .unwrap_err();
    assert_eq!(
        error.message,
        "failed to resolve import `cpm.console` from `src/main.ezra`: no source-relative, caller SDK, or embedded SDK module was found"
    );
}

#[cfg(feature = "i8086")]
#[test]
fn split_dos_pipeline_matches_the_in_process_build() {
    let source = "fn main() { let value: u8 = 1 value += 2 }\n";
    let files = [WorkspaceFile::text("main.ezra", source)];
    let workspace = Workspace::new(&files);
    let request = CompileRequest::new("main.ezra", "msdos-com-i8086");
    let expected = build_workspace(&workspace, "main.ezra", &request).unwrap();

    let program = resolve_workspace_program(&workspace, "main.ezra", &request).unwrap();
    let frontend = ProgramArtifact::new(
        ArtifactStage::Frontend,
        request.target.clone(),
        request.optimization.level,
        program,
    );
    let frontend = ProgramArtifact::decode(&frontend.encode().unwrap()).unwrap();
    let optimized = optimize_i8086_program(&frontend.program, &request).unwrap();
    let optimized = ProgramArtifact::new(
        ArtifactStage::Optimized,
        frontend.target,
        frontend.optimization_level,
        optimized,
    );
    let optimized = ProgramArtifact::decode(&optimized.encode().unwrap()).unwrap();
    let assembly = emit_optimized_i8086_program(&optimized.program, &request).unwrap();
    let linked = build_generated_assembly("main.asm", &assembly, "msdos-com-i8086").unwrap();

    assert!(!assembly.contains("preserved source comments"));
    assert_eq!(linked.executable, expected.executable);
}

#[test]
fn creates_disk_image_without_host_io() {
    let files = [DiskFile::new("GAME.COM", &[0xc3, 0x00, 0x01])];
    let image = create_disk_image(&DiskRequest::new(
        DiskFormat::Fat12_720K,
        "EZRA CPM",
        &files,
    ))
    .expect("in-memory FAT12 image should build");

    assert_eq!(image.len(), DiskFormat::Fat12_720K.image_size());
    let root_directory = 7 * 512;
    assert_eq!(
        &image[root_directory + 32..root_directory + 43],
        b"GAME    COM"
    );
}

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
fn builds_bare_r800_workspace_without_host_io() {
    let files = [WorkspaceFile::text("main.ezra", "fn main() {}\n")];
    let build = build_workspace(
        &Workspace::new(&files),
        "main.ezra",
        &CompileRequest::new("main.ezra", "bare-r800"),
    )
    .expect("virtual no-std R800 workspace should build");

    assert!(!build.machine_code.is_empty());
    assert_eq!(build.executable_extension, "bin");
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
fn preprocesses_and_assembles_virtual_assembly_workspace() {
    let files = [
        WorkspaceFile::text(
            "src/main.asm",
            "include \"macros.inc\"\n%if cpu(\"z80\")\n%return 42h\n%endif\n",
        ),
        WorkspaceFile::text(
            "src/macros.inc",
            "%macro return(value)\n%%entry: ld a, $value\nret\n%endmacro\n",
        ),
    ];
    let workspace = Workspace::new(&files);
    let preprocessed = preprocess_assembly_workspace(
        &workspace,
        "src/main.asm",
        AssemblyPreprocessOptions::for_compiled_features("bare-z80", "z80"),
    )
    .expect("virtual assembly workspace should preprocess without host I/O");
    let assembled = assemble_program_at(AssemblerCpu::Z80, &preprocessed.program, 0x0100)
        .expect("preprocessed virtual assembly should assemble");

    assert_eq!(assembled.bytes, [0x3E, 0x42, 0xC9]);
    assert!(
        assembled
            .symbols
            .iter()
            .any(|symbol| symbol.name.starts_with("__ezra_macro_") && symbol.addr == 0x0100)
    );
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
fn skips_inactive_root_and_imported_workspace_embeds_before_materializing() {
    let files = [
        WorkspaceFile::text(
            "src/main.ezra",
            r#"
                @cfg(cpu("ez80"))
                embed root_missing: bytes = file("assets/root-missing.bin")
                import lib.media
                fn main() {}
            "#,
        ),
        WorkspaceFile::text(
            "src/lib/media.ezra",
            r#"
                @cfg(cpu("ez80"))
                pub embed imported_missing: bytes = file("assets/imported-missing.bin")
            "#,
        ),
    ];
    let compilation = compile_workspace_to_assembly(
        &Workspace::new(&files),
        "src/main.ezra",
        &CompileRequest::new("src/main.ezra", "cpm-2.2-z80"),
    )
    .expect("inactive workspace assets should not be required");

    assert!(!compilation.program.declarations.iter().any(|declaration| {
        matches!(
            declaration,
            Declaration::Embed(embed)
                if matches!(embed.name.as_str(), "root_missing" | "imported_missing")
        )
    }));
}

#[cfg(feature = "z80")]
#[test]
fn active_root_workspace_embeds_still_require_assets() {
    let files = [WorkspaceFile::text(
        "src/main.ezra",
        r#"
            @cfg(cpu("z80"))
            embed root_missing: bytes = file("assets/root-missing.bin")
            fn main() {}
        "#,
    )];
    let error = compile_workspace_to_assembly(
        &Workspace::new(&files),
        "src/main.ezra",
        &CompileRequest::new("src/main.ezra", "cpm-2.2-z80"),
    )
    .unwrap_err();

    assert_eq!(
        error.message,
        "virtual workspace asset `assets/root-missing.bin` referenced from `src/main.ezra` was not found (resolved as `src/assets/root-missing.bin`)"
    );
}

#[cfg(feature = "z80")]
#[test]
fn active_imported_workspace_embeds_still_require_assets() {
    let files = [
        WorkspaceFile::text("src/main.ezra", "import lib.media\nfn main() {}\n"),
        WorkspaceFile::text(
            "src/lib/media.ezra",
            r#"
                @cfg(cpu("z80"))
                pub embed imported_missing: bytes = file("assets/imported-missing.bin")
            "#,
        ),
    ];
    let error = compile_workspace_to_assembly(
        &Workspace::new(&files),
        "src/main.ezra",
        &CompileRequest::new("src/main.ezra", "cpm-2.2-z80"),
    )
    .unwrap_err();

    assert_eq!(
        error.message,
        "virtual workspace asset `assets/imported-missing.bin` referenced from `src/lib/media.ezra` was not found (resolved as `src/lib/assets/imported-missing.bin`)"
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
