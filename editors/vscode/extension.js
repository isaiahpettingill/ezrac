const vscode = require('vscode');
const { LanguageClient } = require('vscode-languageclient/node');

let client;

function activate(context) {
  const config = vscode.workspace.getConfiguration('ezra.languageServer');
  const command = config.get('command', 'ezrac');
  const args = config.get('args', ['lsp']);

  client = new LanguageClient(
    'ezraLanguageServer',
    'EZRA Language Server',
    { command, args },
    {
      documentSelector: [{ scheme: 'file', language: 'ezra' }],
      synchronize: {
        fileEvents: vscode.workspace.createFileSystemWatcher(
          '**/{Ezra.toml,*.ezra,*.ezralayout}'
        )
      }
    }
  );

  context.subscriptions.push(client);
  return client.start();
}

function deactivate() {
  return client ? client.stop() : undefined;
}

module.exports = { activate, deactivate };
