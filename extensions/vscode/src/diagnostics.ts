import * as vscode from "vscode";
import { execSync } from "child_process";
import { getBinaryPath } from "./utils";

interface DiagnosticEntry {
  severity: "error" | "warning";
  rule: string;
  message: string;
}

interface ValidateResult {
  file: string;
  status: "ok" | "error";
  diagnostics: DiagnosticEntry[];
}

export class UsmlDiagnosticsProvider {
  validate(
    document: vscode.TextDocument,
    collection: vscode.DiagnosticCollection
  ): void {
    const filePath = document.uri.fsPath;
    const binaryPath = getBinaryPath();

    if (!binaryPath) {
      collection.set(document.uri, [
        new vscode.Diagnostic(
          new vscode.Range(0, 0, 0, 0),
          "usml バイナリが見つかりません。設定で binaryPath を指定してください。",
          vscode.DiagnosticSeverity.Warning
        ),
      ]);
      return;
    }

    try {
      const output = execSync(`"${binaryPath}" validate --json "${filePath}"`, {
        encoding: "utf-8",
        timeout: 10000,
        stdio: ["pipe", "pipe", "pipe"],
      });

      const result: ValidateResult = JSON.parse(output);
      const diagnostics = result.diagnostics.map((d) => {
        const severity =
          d.severity === "error"
            ? vscode.DiagnosticSeverity.Error
            : vscode.DiagnosticSeverity.Warning;
        return new vscode.Diagnostic(
          new vscode.Range(0, 0, 0, 0),
          `[${d.rule}] ${d.message}`,
          severity
        );
      });

      collection.set(document.uri, diagnostics);
    } catch (e: unknown) {
      // CLI が exit 1 で終了した場合も stdout に JSON がある
      if (e instanceof Error && "stdout" in e) {
        try {
          const result: ValidateResult = JSON.parse(
            (e as { stdout: string }).stdout
          );
          const diagnostics = result.diagnostics.map((d) => {
            const severity =
              d.severity === "error"
                ? vscode.DiagnosticSeverity.Error
                : vscode.DiagnosticSeverity.Warning;
            return new vscode.Diagnostic(
              new vscode.Range(0, 0, 0, 0),
              `[${d.rule}] ${d.message}`,
              severity
            );
          });
          collection.set(document.uri, diagnostics);
          return;
        } catch {
          // JSON パース失敗はフォールスルー
        }
      }

      collection.set(document.uri, [
        new vscode.Diagnostic(
          new vscode.Range(0, 0, 0, 0),
          `バリデーション実行に失敗しました: ${e instanceof Error ? e.message : String(e)}`,
          vscode.DiagnosticSeverity.Error
        ),
      ]);
    }
  }
}
