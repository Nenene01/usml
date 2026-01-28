import * as vscode from "vscode";
import * as path from "path";
import * as fs from "fs";

/**
 * usml バイナリのパスを解決する
 * 優先順: 設定値 > ワークスペース内の target/debug/usml > target/release/usml
 */
export function getBinaryPath(): string | undefined {
  const config = vscode.workspace.getConfiguration("usml");
  const configuredPath = config.get<string>("binaryPath");

  if (configuredPath && fs.existsSync(configuredPath)) {
    return configuredPath;
  }

  // ワークスペース内で探す
  const workspaceFolders = vscode.workspace.workspaceFolders;
  if (workspaceFolders && workspaceFolders.length > 0) {
    const root = workspaceFolders[0].uri.fsPath;

    // debug ビルド
    const debugPath = path.join(root, "target", "debug", "usml");
    if (fs.existsSync(debugPath)) {
      return debugPath;
    }

    // release ビルド
    const releasePath = path.join(root, "target", "release", "usml");
    if (fs.existsSync(releasePath)) {
      return releasePath;
    }
  }

  return undefined;
}
