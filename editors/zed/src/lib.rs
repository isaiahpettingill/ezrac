use zed_extension_api::{self as zed, LanguageServerId, Result, Worktree};

struct EzraExtension;

impl zed::Extension for EzraExtension {
    fn new() -> Self {
        Self
    }

    fn language_server_command(
        &mut self,
        _language_server_id: &LanguageServerId,
        worktree: &Worktree,
    ) -> Result<zed::Command> {
        let command = worktree.which("ezrac").ok_or_else(|| {
            "Could not find `ezrac` in PATH. Install it with `cargo install --path /path/to/ezrac --features lsp`."
                .to_owned()
        })?;

        Ok(zed::Command {
            command,
            args: vec!["lsp".into()],
            env: vec![],
        })
    }
}

zed::register_extension!(EzraExtension);
