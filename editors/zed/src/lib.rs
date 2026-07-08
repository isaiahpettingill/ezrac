use zed_extension_api::{self as zed, LanguageServerId, Result, Worktree};

struct EzraExtension;

impl zed::Extension for EzraExtension {
    fn new() -> Self {
        Self
    }

    fn language_server_command(
        &mut self,
        _language_server_id: &LanguageServerId,
        _worktree: &Worktree,
    ) -> Result<zed::Command> {
        Ok(zed::Command {
            command: "ezrac".into(),
            args: vec!["lsp".into()],
            env: vec![],
        })
    }
}

zed::register_extension!(EzraExtension);
