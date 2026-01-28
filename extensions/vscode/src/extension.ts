import * as vscode from "vscode";
import { UsmlDiagnosticsProvider } from "./diagnostics";
import { UsmlPreviewProvider } from "./preview";

export function activate(context: vscode.ExtensionContext) {
  const diagnosticsProvider = new UsmlDiagnosticsProvider();
  const diagnosticCollection =
    vscode.languages.createDiagnosticCollection("usml");
  context.subscriptions.push(diagnosticCollection);

  // ファイルオープン・保存時に自動バリデーション
  const validateOnOpen = vscode.workspace.onDidOpenTextDocument((doc) => {
    if (doc.fileName.endsWith(".usml.yaml")) {
      diagnosticsProvider.validate(doc, diagnosticCollection);
    }
  });

  const validateOnSave = vscode.workspace.onDidSaveTextDocument((doc) => {
    if (doc.fileName.endsWith(".usml.yaml")) {
      diagnosticsProvider.validate(doc, diagnosticCollection);
    }
  });

  context.subscriptions.push(validateOnOpen, validateOnSave);

  // 既に開いているUSMLファイルを検証
  vscode.workspace.textDocuments.forEach((doc) => {
    if (doc.fileName.endsWith(".usml.yaml")) {
      diagnosticsProvider.validate(doc, diagnosticCollection);
    }
  });

  // コマンド: バリデーション実行
  const validateCommand = vscode.commands.registerCommand(
    "usml.validate",
    () => {
      const editor = vscode.window.activeTextEditor;
      if (editor && editor.document.fileName.endsWith(".usml.yaml")) {
        diagnosticsProvider.validate(editor.document, diagnosticCollection);
        vscode.window.showInformationMessage(
          "バリデーション完了です。問題パネルを確認してください。"
        );
      } else {
        vscode.window.showWarningMessage(
          ".usml.yaml ファイルを開いてください。"
        );
      }
    }
  );

  // コマンド: データフロー図プレビュー
  const previewCommand = vscode.commands.registerCommand(
    "usml.preview",
    async () => {
      const editor = vscode.window.activeTextEditor;
      if (!editor || !editor.document.fileName.endsWith(".usml.yaml")) {
        vscode.window.showWarningMessage(
          ".usml.yaml ファイルを開いてください。"
        );
        return;
      }

      const previewProvider = new UsmlPreviewProvider(context);
      await previewProvider.showPreview(editor.document);
    }
  );

  context.subscriptions.push(validateCommand, previewCommand);
}

export function deactivate() {}
