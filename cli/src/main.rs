use clap::{Arg, ArgAction, Command};
use std::fs;
use std::process;

use usml_core::{parser, validator, visualizer};

fn main() {
    let matches = Command::new("usml")
        .about("Usecase Markup Language - API と DB のデータフローを声明的に定義する")
        .version("0.1.0")
        .subcommand(
            Command::new("validate")
                .about("USML ファイルのバリデーションを実行する")
                .arg(
                    Arg::new("file")
                        .help("検証対象の .usml.yaml ファイルパス")
                        .required(true)
                        .index(1),
                )
                .arg(
                    Arg::new("json")
                        .help("JSON形式で結果を出力する")
                        .long("json")
                        .action(ArgAction::SetTrue),
                ),
        )
        .subcommand(
            Command::new("parse")
                .about("USML ファイルをパースしてAST情報を出力する")
                .arg(
                    Arg::new("file")
                        .help("パース対象の .usml.yaml ファイルパス")
                        .required(true)
                        .index(1),
                ),
        )
        .subcommand(
            Command::new("visualize")
                .about("USML ドキュメントからHTMLデータフロー図を生成する")
                .arg(
                    Arg::new("file")
                        .help("可視化対象の .usml.yaml ファイルパス")
                        .required(true)
                        .index(1),
                )
                .arg(
                    Arg::new("output")
                        .help("出力先HTMLファイルパス（デフォルト: ./output/<usecase-name>.html）")
                        .short('o')
                        .long("output")
                        .value_name("FILE"),
                ),
        )
        .get_matches();

    match matches.subcommand() {
        Some(("validate", sub_matches)) => {
            let file_path = sub_matches.get_one::<String>("file").unwrap();
            let json_output = sub_matches.get_flag("json");
            cmd_validate(file_path, json_output);
        }
        Some(("parse", sub_matches)) => {
            let file_path = sub_matches.get_one::<String>("file").unwrap();
            cmd_parse(file_path);
        }
        Some(("visualize", sub_matches)) => {
            let file_path = sub_matches.get_one::<String>("file").unwrap();
            let output = sub_matches.get_one::<String>("output");
            cmd_visualize(file_path, output);
        }
        _ => {
            // サブコマンド未指定の場合はヘルプを表示
            Command::new("usml")
                .about("Usecase Markup Language - API と DB のデータフローを声明的に定義する")
                .version("0.1.0")
                .subcommand(
                    Command::new("validate").about("USML ファイルのバリデーションを実行する"),
                )
                .subcommand(
                    Command::new("parse").about("USML ファイルをパースしてAST情報を出力する"),
                )
                .subcommand(
                    Command::new("visualize")
                        .about("USML ドキュメントからHTMLデータフロー図を生成する"),
                )
                .print_help()
                .unwrap();
        }
    }
}

fn cmd_validate(file_path: &str, json_output: bool) {
    let input = read_file(file_path);
    let doc = match parser::parse(&input) {
        Ok(doc) => doc,
        Err(e) => {
            if json_output {
                println!(
                    r#"{{"file":"{}","status":"error","diagnostics":[{{"severity":"error","rule":"parse","message":"{}"}}]}}"#,
                    escape_json_string(file_path),
                    escape_json_string(&e.to_string())
                );
            } else {
                eprintln!("パースエラー: {}", e);
            }
            process::exit(1);
        }
    };

    let errors = validator::validate(&doc);

    if json_output {
        let diagnostics: Vec<String> = errors
            .iter()
            .map(|err| match err {
                validator::ValidationError::Rule(rule, msg) => format!(
                    r#"{{"severity":"error","rule":"{}","message":"{}"}}"#,
                    escape_json_string(rule),
                    escape_json_string(msg)
                ),
                validator::ValidationError::Warning(rule, msg) => format!(
                    r#"{{"severity":"warning","rule":"{}","message":"{}"}}"#,
                    escape_json_string(rule),
                    escape_json_string(msg)
                ),
            })
            .collect();
        let has_rule_error = errors
            .iter()
            .any(|err| matches!(err, validator::ValidationError::Rule(..)));
        let status = if has_rule_error { "error" } else { "ok" };
        println!(
            r#"{{"file":"{}","status":"{}","diagnostics":[{}]}}"#,
            escape_json_string(file_path),
            status,
            diagnostics.join(",")
        );
        if has_rule_error {
            process::exit(1);
        }
    } else if errors.is_empty() {
        println!("✓ バリデーション成功: '{}'", file_path);
    } else {
        eprintln!(
            "✗ バリデーションエラー: '{}' ({} 件)",
            file_path,
            errors.len()
        );
        for (i, err) in errors.iter().enumerate() {
            eprintln!("  [{}] {}", i + 1, err);
        }
        process::exit(1);
    }
}

fn escape_json_string(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            _ => escaped.push(ch),
        }
    }
    escaped
}

fn cmd_parse(file_path: &str) {
    let input = read_file(file_path);
    let doc = match parser::parse(&input) {
        Ok(doc) => doc,
        Err(e) => {
            eprintln!("パースエラー: {}", e);
            process::exit(1);
        }
    };

    println!("ドキュメント: {}", doc.usecase.name);
    println!("バージョン: {}", doc.version);
    if let Some(summary) = &doc.usecase.summary {
        println!("サマリー: {}", summary);
    }
    println!(
        "レスポンスマッピング: {} フィールド",
        doc.usecase.response_mapping.len()
    );
    println!("フィルタ: {} 件", doc.usecase.filters.len());
    println!("トランスフォーム: {} 件", doc.usecase.transforms.len());

    println!("\n--- レスポンスマッピング ---");
    print_mappings(&doc.usecase.response_mapping, 0);
}

fn print_mappings(mappings: &[usml_core::ast::ResponseMapping], indent: usize) {
    let prefix = "  ".repeat(indent);
    for mapping in mappings {
        let source_str = mapping.source.as_deref().unwrap_or("-");
        let type_str = mapping
            .r#type
            .as_ref()
            .map(|t| format!(" [{}]", t))
            .unwrap_or_default();
        println!("{}{}: {} {}", prefix, mapping.field, source_str, type_str);

        if let Some(join) = &mapping.join {
            let alias_str = join
                .alias
                .as_ref()
                .map(|a| format!(" (alias: {})", a))
                .unwrap_or_default();
            println!(
                "{}  └─ JOIN {} ON {}{}",
                prefix, join.table, join.on, alias_str
            );
        }

        if let Some(agg) = &mapping.aggregate {
            println!("{}  └─ {}", prefix, agg.r#type);
        }

        if let Some(sub_fields) = &mapping.fields {
            print_mappings(sub_fields, indent + 2);
        }
    }
}

fn read_file(path: &str) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| {
        eprintln!("ファイル読み込みエラー '{}': {}", path, e);
        process::exit(1);
    })
}

fn cmd_visualize(file_path: &str, output: Option<&String>) {
    let input = read_file(file_path);
    let doc = match parser::parse(&input) {
        Ok(doc) => doc,
        Err(e) => {
            eprintln!("パースエラー: {}", e);
            process::exit(1);
        }
    };

    let html = visualizer::generate_html(&doc);

    // 出力先パスを決定
    let output_path = if let Some(path) = output {
        // -o オプションが指定されている場合はそれを優先
        path.clone()
    } else if let Some(output_name) = &doc.usecase.output {
        // USMLファイル内のoutputパラメータが指定されている場合
        let output_dir = "output";
        if let Err(e) = fs::create_dir_all(output_dir) {
            eprintln!("ディレクトリ作成エラー '{}': {}", output_dir, e);
            process::exit(1);
        }
        format!("{}/{}", output_dir, output_name)
    } else {
        // デフォルト: ./output/<usecase-name>.html
        let output_dir = "output";
        if let Err(e) = fs::create_dir_all(output_dir) {
            eprintln!("ディレクトリ作成エラー '{}': {}", output_dir, e);
            process::exit(1);
        }

        // ユースケース名からファイル名を生成（スペースや特殊文字を置換）
        let safe_name = doc
            .usecase
            .name
            .chars()
            .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
            .collect::<String>();
        format!("{}/{}.html", output_dir, safe_name)
    };

    if let Err(e) = fs::write(&output_path, html) {
        eprintln!("ファイル書き込みエラー '{}': {}", output_path, e);
        process::exit(1);
    }
    println!("✓ HTML を出力しました: '{}'", output_path);
}
