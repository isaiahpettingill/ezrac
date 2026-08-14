use alloc::{
    format,
    string::{String, ToString},
};
use core::str;
use ezra::{
    api::{CompileRequest, Workspace, WorkspaceFile, resolve_workspace_program},
    internal_ir::{ArtifactStage, ProgramArtifact},
};

use crate::dos;

pub fn run(args: &[String]) -> Result<(), String> {
    if args.len() != 2 {
        return Err("usage: EZFE source.ezra output.ezi".into());
    }
    let input = &args[0];
    let output = &args[1];
    let source_bytes = dos::read_file(input)
        .map_err(|error| format!("cannot read `{input}` (DOS error {})", error.0))?;
    let source =
        str::from_utf8(&source_bytes).map_err(|_| format!("`{input}` is not valid UTF-8"))?;
    let files = [
        WorkspaceFile::text(input, source),
        WorkspaceFile::text(
            "dos/console.ezra",
            include_str!("../../toolchains/msdos-i8086/sdk/dos/console.ezra"),
        ),
        WorkspaceFile::text(
            "dos/constants.ezra",
            include_str!("../../toolchains/msdos-i8086/sdk/dos/constants.ezra"),
        ),
        WorkspaceFile::text(
            "dos/datetime.ezra",
            include_str!("../../toolchains/msdos-i8086/sdk/dos/datetime.ezra"),
        ),
        WorkspaceFile::text(
            "dos/directory.ezra",
            include_str!("../../toolchains/msdos-i8086/sdk/dos/directory.ezra"),
        ),
        WorkspaceFile::text(
            "dos/file.ezra",
            include_str!("../../toolchains/msdos-i8086/sdk/dos/file.ezra"),
        ),
        WorkspaceFile::text(
            "dos/memory.ezra",
            include_str!("../../toolchains/msdos-i8086/sdk/dos/memory.ezra"),
        ),
        WorkspaceFile::text(
            "dos/process.ezra",
            include_str!("../../toolchains/msdos-i8086/sdk/dos/process.ezra"),
        ),
        WorkspaceFile::text(
            "dos/psp.ezra",
            include_str!("../../toolchains/msdos-i8086/sdk/dos/psp.ezra"),
        ),
        WorkspaceFile::text(
            "dos/raw.ezra",
            include_str!("../../toolchains/msdos-i8086/sdk/dos/raw.ezra"),
        ),
    ];
    let request = CompileRequest::new(input, "msdos-com-i8086");
    let program = resolve_workspace_program(&Workspace::new(&files), input, &request)
        .map_err(|diagnostic| diagnostic.to_string())?;
    let bytes = ProgramArtifact::new(
        ArtifactStage::Frontend,
        request.target,
        request.optimization.level,
        program,
    )
    .encode()
    .map_err(|diagnostic| diagnostic.to_string())?;
    dos::write_file(output, &bytes)
        .map_err(|error| format!("cannot write `{output}` (DOS error {})", error.0))
}
