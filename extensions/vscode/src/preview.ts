import * as vscode from "vscode";
import { execSync } from "child_process";
import { getBinaryPath } from "./utils";

export class UsmlPreviewProvider {
  private readonly context: vscode.ExtensionContext;

  constructor(context: vscode.ExtensionContext) {
    this.context = context;
  }

  async showPreview(document: vscode.TextDocument): Promise<void> {
    const filePath = document.uri.fsPath;
    const binaryPath = getBinaryPath();

    if (!binaryPath) {
      vscode.window.showWarningMessage(
        "usml バイナリが見つかりません。設定で binaryPath を指定してください。"
      );
      return;
    }

    let html: string;
    try {
      html = execSync(`"${binaryPath}" visualize "${filePath}"`, {
        encoding: "utf-8",
        timeout: 10000,
        stdio: ["pipe", "pipe", "pipe"],
      });
    } catch (e) {
      vscode.window.showErrorMessage(
        `Visualize に失敗しました: ${e instanceof Error ? e.message : String(e)}`
      );
      return;
    }

    const panel = vscode.window.createWebviewPanel(
      "usml.preview",
      `USML Preview: ${document.fileName.split("/").pop()}`,
      vscode.ViewColumn.Beside,
      {
        enableScripts: true,
        retainWhenHidden: true,
      }
    );

    // CSP ポリシーを設定してインラインスタイルを許可
    const nonce = this.generateNonce();
    html = html.replace(
      "<head>",
      `<meta http-equiv="Content-Security-Policy" content="default-src none; style-src unsafe-inline; script-src nonce-;">` +
        "<head>"
    );

    panel.webview.html = html;
  }

  private generateNonce(): string {
    let text = "";
    const possible =
      "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    for (let i = 0; i < 32; i++) {
      text += possible.at(Math.floor(Math.random() * possible.length));
    }
    return text;
  }
}
