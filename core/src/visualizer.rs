use std::collections::{HashMap, HashSet};
use std::fmt::Write;

use crate::ast::{ResponseMapping, Transform, UsmlDocument};
use crate::resolver;

struct FieldEntry {
    field: String,
    field_path: String, // フルパス（例: "comments.id"）
    source: Option<String>, // 元のsource（例: "posts.id"）
    badges: Vec<String>,
    join_lines: Vec<String>,
    transforms: Vec<String>,
    depth: usize,
    tables: Vec<String>,
    join_type: String,
}

pub fn generate_html(doc: &UsmlDocument) -> String {
    let transform_map = build_transform_map(&doc.usecase.transforms);
    let mut table_order = extract_import_tables(doc);
    let mut table_seen: HashSet<String> = table_order.iter().cloned().collect();
    let mut table_columns: HashMap<String, HashSet<String>> = table_order
        .iter()
        .cloned()
        .map(|table| (table, HashSet::new()))
        .collect();
    let mut entries = Vec::new();
    let mut alias_map: HashMap<String, String> = HashMap::new(); // alias -> actual table name

    collect_entries(
        &doc.usecase.response_mapping,
        0,
        "",
        &transform_map,
        &mut entries,
        &mut table_columns,
        &mut table_order,
        &mut table_seen,
        &mut alias_map,
    );

    let mut html = String::new();
    html.push_str("<!DOCTYPE html>\n<html>\n<head>\n<meta charset=\"utf-8\">\n");
    html.push_str("<title>USML Data Flow Visualizer</title>\n");
    html.push_str("<link rel=\"stylesheet\" href=\"https://cdnjs.cloudflare.com/ajax/libs/font-awesome/6.4.0/css/all.min.css\">\n");
    html.push_str("<style>\n");
    html.push_str(
        "body { font-family: 'Inter', 'Helvetica Neue', Arial, sans-serif; background: #f5f7fa; color: #1f2a37; margin: 0; padding: 0; }\n",
    );
    html.push_str(".header { background: #fff; border-bottom: 2px solid #e5e7eb; padding: 24px 32px 0 32px; }\n");
    html.push_str(".header h1 { font-size: 1.8rem; margin: 0 0 8px 0; color: #1f2937; }\n");
    html.push_str(".header .summary { font-size: 0.95rem; color: #6b7280; margin-bottom: 16px; line-height: 1.5; }\n");
    html.push_str(".api-info { display: flex; align-items: center; gap: 12px; margin-bottom: 24px; flex-wrap: wrap; }\n");
    html.push_str(".method-badge { display: inline-block; padding: 4px 10px; border-radius: 4px; font-size: 0.75rem; font-weight: 700; text-transform: uppercase; letter-spacing: 0.05em; }\n");
    html.push_str(".method-get { background: #dbeafe; color: #1e40af; }\n");
    html.push_str(".method-post { background: #dcfce7; color: #15803d; }\n");
    html.push_str(".method-put { background: #fef3c7; color: #92400e; }\n");
    html.push_str(".method-delete { background: #fee2e2; color: #991b1b; }\n");
    html.push_str(".method-patch { background: #f3e8ff; color: #6b21a8; }\n");
    html.push_str(".api-path { font-family: 'Monaco', 'Menlo', monospace; font-size: 0.9rem; color: #374151; background: #f3f4f6; padding: 6px 12px; border-radius: 4px; }\n");
    html.push_str(".status-badge { display: inline-block; padding: 4px 10px; border-radius: 4px; font-size: 0.75rem; font-weight: 600; background: #d1fae5; color: #065f46; }\n");
    html.push_str(".tabs { display: flex; gap: 4px; margin-top: 0; }\n");
    html.push_str(".tab { display: flex; align-items: center; gap: 8px; padding: 12px 24px; background: transparent; color: #6b7280; border: none; border-bottom: 3px solid transparent; cursor: pointer; font-size: 0.95rem; font-weight: 500; transition: all 0.2s; }\n");
    html.push_str(".tab:hover { color: #1f2937; background: #f9fafb; }\n");
    html.push_str(".tab.active { color: #3b82f6; border-bottom-color: #3b82f6; }\n");
    html.push_str(".tab i { font-size: 1.1rem; }\n");
    html.push_str(".main-content { padding: 32px 32px 80px 32px; background: #fff; min-height: calc(100vh - 180px); }\n");
    html.push_str(".view { display: none; }\n");
    html.push_str(".view.active { display: block; }\n");
    html.push_str(
        ".grid { display: grid; grid-template-columns: repeat(3, 1fr); gap: 16px; align-items: start; }\n",
    );
    html.push_str(".column h2 { font-size: 1.1rem; margin-bottom: 12px; }\n");
    html.push_str(
        ".card { border-radius: 12px; padding: 12px 16px; margin-bottom: 12px; box-shadow: 0 4px 12px rgba(15, 23, 42, 0.08); transition: all 0.2s ease; }\n",
    );
    html.push_str(".response-card { background: #e8f4fd; }\n");
    html.push_str(".join-card { background: #fff8e1; }\n");
    html.push_str(".table-card { background: #f0faf0; }\n");
    html.push_str(
        ".badge { display: inline-block; background: #6c757d; color: #fff; border-radius: 999px; font-size: 0.72rem; padding: 2px 8px; margin-right: 4px; }\n",
    );
    html.push_str(".field-name { font-weight: 600; margin-bottom: 6px; }\n");
    html.push_str(".field-name.small { font-weight: 500; font-size: 0.9rem; color: #394150; }\n");
    html.push_str(".join-line, .transform-line { font-size: 0.9rem; margin-top: 4px; }\n");
    html.push_str(".empty { color: #6b7280; font-size: 0.9rem; }\n");
    html.push_str(".depth-1 { margin-left: 24px; padding-left: 12px; border-left: 3px solid #3b82f6; background: #dbeafe !important; }\n");
    html.push_str(".depth-2 { margin-left: 48px; padding-left: 12px; border-left: 3px solid #8b5cf6; background: #e9d5ff !important; }\n");
    html.push_str(".depth-3 { margin-left: 72px; padding-left: 12px; border-left: 3px solid #ec4899; background: #fce7f3 !important; }\n");
    html.push_str(".depth-4 { margin-left: 96px; padding-left: 12px; border-left: 3px solid #f59e0b; background: #fef3c7 !important; }\n");
    html.push_str("#flow-container { position: relative; }\n");
    html.push_str("#flow-svg { position: absolute; top: 0; left: 0; width: 100%; height: 100%; pointer-events: none; z-index: 10; }\n");
    html.push_str(".arrow-simple { stroke: #9ca3af; }\n");
    html.push_str(".arrow-join { stroke: #d4a017; }\n");
    html.push_str(".arrow-join-chain { stroke: #3b82f6; }\n");
    html.push_str(".arrow-aggregate { stroke: #8b5cf6; }\n");
    html.push_str(".card.highlighted { box-shadow: 0 0 24px rgba(251,191,36,0.9), 0 0 12px rgba(251,191,36,0.6); transform: scale(1.05); border: 3px solid #fbbf24; }\n");
    html.push_str(".legend { position: fixed; bottom: 0; left: 0; right: 0; z-index: 100; display: none; gap: 16px; flex-wrap: wrap; justify-content: center; padding: 12px 16px; background: #fff; border-top: 2px solid #e5e7eb; box-shadow: 0 -4px 12px rgba(0,0,0,0.1); }\n");
    html.push_str(".legend.active { display: flex; }\n");
    html.push_str(
        ".legend-item { display: flex; align-items: center; gap: 6px; font-size: 0.85rem; }\n",
    );
    html.push_str(".legend-line { width: 28px; height: 3px; border-radius: 2px; }\n");
    html.push_str("table { width: 100%; border-collapse: collapse; background: #fff; border-radius: 8px; overflow: hidden; box-shadow: 0 2px 8px rgba(0,0,0,0.1); }\n");
    html.push_str("thead { background: #374151; color: #fff; }\n");
    html.push_str("th { padding: 12px 16px; text-align: left; font-weight: 600; font-size: 0.9rem; }\n");
    html.push_str("td { padding: 12px 16px; border-bottom: 1px solid #e5e7eb; }\n");
    html.push_str("tbody tr:last-child td { border-bottom: none; }\n");
    html.push_str("tbody tr:hover { background: #f9fafb; }\n");
    html.push_str(".table-section { margin-bottom: 32px; }\n");
    html.push_str(".table-section h2 { font-size: 1.3rem; margin-bottom: 16px; }\n");
    html.push_str(".indent-1 { padding-left: 32px; background: #eff6ff; }\n");
    html.push_str(".indent-2 { padding-left: 48px; background: #f3e8ff; }\n");
    html.push_str(".indent-3 { padding-left: 64px; background: #fce7f3; }\n");
    html.push_str(".indent-4 { padding-left: 80px; background: #fef3c7; }\n");
    html.push_str("code.inline { background: #e5e7eb; padding: 2px 6px; border-radius: 4px; font-size: 0.9em; }\n");
    html.push_str("</style>\n</head>\n<body>\n");

    // ヘッダー
    html.push_str("<div class=\"header\">\n");
    write!(&mut html, "<h1>{}</h1>", escape_html(&doc.usecase.name)).unwrap();
    if let Some(summary) = &doc.usecase.summary {
        write!(&mut html, "<p class=\"summary\">{}</p>", escape_html(summary)).unwrap();
    }

    // OpenAPI情報を表示
    if let Some(openapi_ref) = &doc.import.openapi {
        if let Some((_file, path, method, status)) = resolver::openapi::parse_openapi_ref(openapi_ref) {
            html.push_str("<div class=\"api-info\">\n");

            // HTTPメソッドバッジ
            let method_upper = method.to_uppercase();
            let method_class = match method_upper.as_str() {
                "GET" => "method-get",
                "POST" => "method-post",
                "PUT" => "method-put",
                "DELETE" => "method-delete",
                "PATCH" => "method-patch",
                _ => "method-get",
            };
            write!(&mut html, "<span class=\"method-badge {}\">{}</span>", method_class, escape_html(&method_upper)).unwrap();

            // APIパス
            write!(&mut html, "<span class=\"api-path\">{}</span>", escape_html(&path)).unwrap();

            // ステータスコード
            write!(&mut html, "<span class=\"status-badge\">Status: {}</span>", escape_html(&status)).unwrap();

            html.push_str("</div>\n");
        }
    }

    html.push_str("<div class=\"tabs\">\n");
    html.push_str("<button class=\"tab active\" onclick=\"switchView('table', event)\"><i class=\"fas fa-table\"></i> テーブル</button>\n");
    html.push_str("<button class=\"tab\" onclick=\"switchView('visual', event)\"><i class=\"fas fa-project-diagram\"></i> ビジュアル</button>\n");
    html.push_str("</div></div>\n");

    // メインコンテンツ
    html.push_str("<div class=\"main-content\">\n");

    // ビジュアルビュー
    html.push_str("<div id=\"visual-view\" class=\"view\">\n");
    html.push_str("<div class=\"grid\">\n");

    html.push_str("<div class=\"column\">\n<h2>Response Fields</h2>\n");
    if entries.is_empty() {
        html.push_str("<div class=\"empty\">No response mappings.</div>");
    } else {
        for entry in &entries {
            let depth_class = depth_class(entry.depth);
            write!(
                &mut html,
                "<div class=\"card response-card{}\" data-field=\"{}\" data-tables=\"{}\" data-join-type=\"{}\">",
                depth_class,
                escape_html(&entry.field_path),
                escape_html(&entry.tables.join(",")),
                escape_html(&entry.join_type)
            )
            .unwrap();
            write!(
                &mut html,
                "<div class=\"field-name\">{}</div>",
                escape_html(&entry.field)
            )
            .unwrap();
            if !entry.badges.is_empty() {
                html.push_str("<div>");
                for badge in &entry.badges {
                    write!(
                        &mut html,
                        "<span class=\"badge\">{}</span>",
                        escape_html(badge)
                    )
                    .unwrap();
                }
                html.push_str("</div>");
            }
            html.push_str("</div>\n");
        }
    }
    html.push_str("</div>\n");

    html.push_str("<div class=\"column\">\n<h2>Joins &amp; Transforms</h2>\n");
    let has_joins_or_transforms = entries.iter().any(|e| !e.join_lines.is_empty() || !e.transforms.is_empty());
    if !has_joins_or_transforms {
        html.push_str("<div class=\"empty\">No joins or transforms.</div>");
    } else {
        for entry in &entries {
            // JOINやtransformがない場合はスキップ
            if entry.join_lines.is_empty() && entry.transforms.is_empty() {
                continue;
            }

            let depth_class = depth_class(entry.depth);
            write!(
                &mut html,
                "<div class=\"card join-card{}\" data-field=\"{}\">",
                depth_class,
                escape_html(&entry.field_path)
            )
            .unwrap();
            write!(
                &mut html,
                "<div class=\"field-name small\">{}</div>",
                escape_html(&entry.field)
            )
            .unwrap();

            // 種類バッジを追加
            let join_type_label = match entry.join_type.as_str() {
                "simple" => "Simple",
                "join" => "JOIN",
                "join-chain" => "JOIN Chain",
                "aggregate" => "Aggregate",
                _ => "Simple",
            };
            write!(
                &mut html,
                "<div style=\"margin-bottom: 6px;\"><span class=\"badge\">{}</span></div>",
                join_type_label
            )
            .unwrap();

            for join_line in &entry.join_lines {
                write!(
                    &mut html,
                    "<div class=\"join-line\">{}</div>",
                    escape_html(join_line)
                )
                .unwrap();
            }
            if !entry.transforms.is_empty() {
                html.push_str("<div class=\"transform-line\">Transforms:</div>");
                html.push_str("<div>");
                for transform in &entry.transforms {
                    write!(
                        &mut html,
                        "<span class=\"badge\">{}</span>",
                        escape_html(transform)
                    )
                    .unwrap();
                }
                html.push_str("</div>");
            }
            html.push_str("</div>\n");
        }
    }
    html.push_str("</div>\n");

    html.push_str("<div class=\"column\">\n<h2>Tables</h2>\n");
    if table_order.is_empty() {
        html.push_str("<div class=\"empty\">No tables imported.</div>");
    } else {
        for table in &table_order {
            let columns = table_columns.get(table);
            // エイリアスかどうかを判定
            let display_name = if let Some(actual_table) = alias_map.get(table) {
                format!("{} <span style=\"color: #6b7280; font-weight: 400;\">(as {})</span>", actual_table, table)
            } else {
                table.clone()
            };
            write!(
                &mut html,
                "<div class=\"card table-card\" data-table=\"{}\"><div class=\"field-name\">{}</div>",
                escape_html(table),
                display_name
            )
            .unwrap();

            if let Some(cols) = columns && !cols.is_empty() {
                let mut sorted_cols: Vec<_> = cols.iter().collect();
                sorted_cols.sort();
                html.push_str("<div class=\"join-line\">Columns:</div><div style=\"margin-top: 4px;\">");
                for (i, col) in sorted_cols.iter().enumerate() {
                    if i > 0 {
                        html.push_str(", ");
                    }
                    write!(&mut html, "<code style=\"background: #e5e7eb; padding: 2px 6px; border-radius: 4px; font-size: 0.85rem;\">{}</code>", escape_html(col)).unwrap();
                }
                html.push_str("</div>");
            } else {
                html.push_str("<div class=\"join-line\" style=\"color: #9ca3af;\">No columns referenced</div>");
            }
            html.push_str("</div>\n");
        }
    }
    html.push_str("</div>\n</div>\n</div>\n"); // column (Tables), grid, visual-view の終了

    // テーブルビュー
    html.push_str("<div id=\"table-view\" class=\"view active\">\n");
    generate_table_view(&mut html, &entries, &table_order, &table_columns, doc, &alias_map);
    html.push_str("</div>\n");

    html.push_str("</div>\n"); // main-content の終了

    // JavaScript for view switching
    html.push_str(r#"<script>
function switchView(viewName, event) {
  document.querySelectorAll('.view').forEach(function(v) { v.classList.remove('active'); });
  document.querySelectorAll('.tab').forEach(function(b) { b.classList.remove('active'); });
  document.getElementById(viewName + '-view').classList.add('active');
  if (event && event.target) {
    event.target.classList.add('active');
  }
}

(function() {
  function setupHover() {
    document.querySelectorAll('.response-card[data-field]').forEach(function(card) {
      card.addEventListener('mouseenter', function() {
        var field = card.dataset.field;
        var tables = (card.dataset.tables || '').split(',').filter(function(t) { return t.length > 0; });
        card.classList.add('highlighted');
        document.querySelectorAll('.join-card[data-field="' + field + '"]').forEach(function(c) { c.classList.add('highlighted'); });
        tables.forEach(function(t) {
          var tc = document.querySelector('.table-card[data-table="' + t + '"]');
          if (tc) tc.classList.add('highlighted');
        });
      });
      card.addEventListener('mouseleave', function() {
        document.querySelectorAll('.card').forEach(function(c) { c.classList.remove('highlighted'); });
      });
    });
  }
  window.addEventListener('load', function() {
    setupHover();
  });
})();
</script>
"#);
    html.push_str("</body>\n</html>\n");
    html
}

fn build_transform_map(transforms: &[Transform]) -> HashMap<String, Vec<String>> {
    let mut map = HashMap::new();
    for transform in transforms {
        map.entry(transform.target.clone())
            .or_insert_with(Vec::new)
            .push(transform.r#type.clone());
    }
    map
}

fn extract_import_tables(doc: &UsmlDocument) -> Vec<String> {
    let mut tables = Vec::new();
    let mut seen = HashSet::new();
    if let Some(dbmls) = &doc.import.dbml {
        for entry in dbmls {
            if let Some(table) = extract_table_name(entry)
                && seen.insert(table.clone())
            {
                tables.push(table);
            }
        }
    }
    tables
}

fn extract_table_name(value: &str) -> Option<String> {
    let marker = "#tables[\"";
    let start = value.find(marker)? + marker.len();
    let remainder = &value[start..];
    let end = remainder.find("\"]")?;
    Some(remainder[..end].to_string())
}

fn extract_table_identifier(value: &str) -> Option<String> {
    let token = value.split_whitespace().next()?;
    let table = token.split('.').next()?;
    if table.is_empty() {
        None
    } else {
        Some(table.to_string())
    }
}

fn collect_entries(
    mappings: &[ResponseMapping],
    depth: usize,
    parent_path: &str,
    transform_map: &HashMap<String, Vec<String>>,
    entries: &mut Vec<FieldEntry>,
    table_columns: &mut HashMap<String, HashSet<String>>,
    table_order: &mut Vec<String>,
    table_seen: &mut HashSet<String>,
    alias_map: &mut HashMap<String, String>,
) {
    for mapping in mappings {
        let mut badges = Vec::new();
        if let Some(aggregate) = &mapping.aggregate {
            badges.push(aggregate.r#type.clone());
        }
        if mapping.r#type.as_deref() == Some("array") {
            badges.push("array".to_string());
        }

        let mut join_lines = Vec::new();
        if let Some(join) = &mapping.join {
            let join_type = join.r#type.as_deref().unwrap_or("JOIN");
            let table_part = if let Some(alias) = &join.alias {
                // エイリアスマッピングを記録
                alias_map.insert(alias.clone(), join.table.clone());
                format!("{} AS {}", join.table, alias)
            } else {
                join.table.clone()
            };
            let line = format!("{} {} ON {}", join_type, table_part, join.on);
            join_lines.push(line);
        }
        if let Some(chain) = &mapping.join_chain
            && !chain.is_empty()
        {
            let chain_line = chain
                .iter()
                .map(|entry| format!("JOIN {} ON {}", entry.table, entry.on))
                .collect::<Vec<_>>()
                .join(" → ");
            join_lines.push(chain_line);
        }

        let join_type = if mapping.aggregate.is_some() {
            "aggregate".to_string()
        } else if mapping.join_chain.is_some() {
            "join-chain".to_string()
        } else if mapping.join.is_some() {
            "join".to_string()
        } else {
            "simple".to_string()
        };

        let mut field_tables: Vec<String> = Vec::new();
        if let Some(source) = &mapping.source
            && let Some(table) = extract_table_identifier(source)
            && !field_tables.contains(&table)
        {
            field_tables.push(table);
        }
        if let Some(join) = &mapping.join
            && !field_tables.contains(&join.table)
        {
            field_tables.push(join.table.clone());
        }
        if let Some(chain) = &mapping.join_chain {
            for entry in chain {
                if !field_tables.contains(&entry.table) {
                    field_tables.push(entry.table.clone());
                }
            }
        }

        let transforms = transform_map
            .get(&mapping.field)
            .cloned()
            .unwrap_or_default();

        // フルパスを構築（親がいる場合は "親.子" の形式）
        let field_path = if parent_path.is_empty() {
            mapping.field.clone()
        } else {
            format!("{}.{}", parent_path, mapping.field)
        };

        entries.push(FieldEntry {
            field: mapping.field.clone(),
            field_path,
            source: mapping.source.clone(),
            badges,
            join_lines,
            transforms,
            depth,
            tables: field_tables,
            join_type,
        });

        // テーブルとカラムの情報を記録
        if let Some(source) = &mapping.source {
            if let Some((table, column)) = source.split_once('.') {
                let table = table.to_string();
                let column = column.to_string();

                table_columns
                    .entry(table.clone())
                    .or_insert_with(HashSet::new)
                    .insert(column);

                if table_seen.insert(table.clone()) {
                    table_order.push(table);
                }
            }
        }

        // JOIN、JOIN chainのテーブルも記録（カラムなし）
        if let Some(join) = &mapping.join {
            let table = join.table.clone();
            table_columns.entry(table.clone()).or_insert_with(HashSet::new);
            if table_seen.insert(table.clone()) {
                table_order.push(table);
            }
        }

        if let Some(chain) = &mapping.join_chain {
            for entry in chain {
                let table = entry.table.clone();
                table_columns.entry(table.clone()).or_insert_with(HashSet::new);
                if table_seen.insert(table.clone()) {
                    table_order.push(table);
                }
            }
        }

        if let Some(fields) = &mapping.fields {
            // 親パスを現在のフィールドパスに更新して再帰
            let current_field_path = if parent_path.is_empty() {
                mapping.field.clone()
            } else {
                format!("{}.{}", parent_path, mapping.field)
            };
            collect_entries(
                fields,
                depth + 1,
                &current_field_path,
                transform_map,
                entries,
                table_columns,
                table_order,
                table_seen,
                alias_map,
            );
        }
    }
}

fn depth_class(depth: usize) -> String {
    if depth == 0 {
        String::new()
    } else {
        format!(" depth-{}", depth.min(4))
    }
}

fn generate_table_view(
    html: &mut String,
    entries: &[FieldEntry],
    table_order: &[String],
    table_columns: &HashMap<String, HashSet<String>>,
    doc: &UsmlDocument,
    alias_map: &HashMap<String, String>,
) {
    // Response Mapping Table
    html.push_str("<div class=\"table-section\"><h2>Response Mapping</h2>\n");
    html.push_str("<table><thead><tr><th>Field</th><th>Source</th><th>Type</th><th>JOIN</th><th>Transforms</th></tr></thead><tbody>\n");

    for entry in entries {
        let indent_class = match entry.depth {
            1 => " class=\"indent-1\"",
            2 => " class=\"indent-2\"",
            3 => " class=\"indent-3\"",
            4 => " class=\"indent-4\"",
            _ => "",
        };
        write!(html, "<tr{}>", indent_class).unwrap();

        // フィールド名にインデント表現を追加
        let field_display = if entry.depth > 0 {
            let indent = "  ".repeat(entry.depth);
            format!("{}\u{2514}\u{2500} {}", indent, entry.field)
        } else {
            entry.field.clone()
        };
        write!(html, "<td><code class=\"inline\">{}</code></td>", escape_html(&field_display)).unwrap();

        // Source - mapping.sourceまたはtables列から推定
        let source = if let Some(src) = &entry.source {
            src.clone()
        } else if !entry.tables.is_empty() {
            entry.tables.join(", ")
        } else {
            "-".to_string()
        };
        write!(html, "<td>{}</td>", escape_html(&source)).unwrap();

        // Type - badges
        let type_str = if !entry.badges.is_empty() {
            entry.badges.join(", ")
        } else {
            "-".to_string()
        };
        write!(html, "<td>{}</td>", escape_html(&type_str)).unwrap();

        // JOIN
        let join_str = if !entry.join_lines.is_empty() {
            entry.join_lines.join("<br>")
        } else {
            "-".to_string()
        };
        write!(html, "<td>{}</td>", join_str).unwrap();

        // Transforms
        let transform_str = if !entry.transforms.is_empty() {
            entry.transforms.iter().map(|t| format!("<code class=\"inline\">{}</code>", escape_html(t))).collect::<Vec<_>>().join(", ")
        } else {
            "-".to_string()
        };
        write!(html, "<td>{}</td>", transform_str).unwrap();

        html.push_str("</tr>\n");
    }

    html.push_str("</tbody></table></div>\n");

    // Tables Summary
    html.push_str("<div class=\"table-section\"><h2>Tables Summary</h2>\n");
    html.push_str("<table><thead><tr><th>Table</th><th>Columns</th></tr></thead><tbody>\n");

    for table in table_order {
        // エイリアスかどうかを判定
        let display_name = if let Some(actual_table) = alias_map.get(table) {
            format!("<strong>{}</strong> <span style=\"color: #6b7280; font-weight: 400;\">(as {})</span>", escape_html(actual_table), escape_html(table))
        } else {
            format!("<strong>{}</strong>", escape_html(table))
        };
        write!(html, "<tr><td>{}</td>", display_name).unwrap();

        if let Some(cols) = table_columns.get(table) && !cols.is_empty() {
            let mut sorted_cols: Vec<_> = cols.iter().collect();
            sorted_cols.sort();
            let cols_html = sorted_cols.iter().map(|c| format!("<code class=\"inline\">{}</code>", escape_html(c))).collect::<Vec<_>>().join(", ");
            write!(html, "<td>{}</td>", cols_html).unwrap();
        } else {
            html.push_str("<td style=\"color: #9ca3af;\">No columns referenced</td>");
        }

        html.push_str("</tr>\n");
    }

    html.push_str("</tbody></table></div>\n");

    // Filters Summary
    if !doc.usecase.filters.is_empty() {
        html.push_str("<div class=\"table-section\"><h2>Filters</h2>\n");
        html.push_str("<table><thead><tr><th>Parameter</th><th>Maps To</th><th>Details</th></tr></thead><tbody>\n");

        for filter in &doc.usecase.filters {
            write!(html, "<tr><td><code class=\"inline\">{}</code></td>", escape_html(&filter.param)).unwrap();
            write!(html, "<td><strong>{}</strong></td>", escape_html(&filter.maps_to)).unwrap();

            let mut details = Vec::new();
            if let Some(condition) = &filter.condition {
                details.push(format!("<code class=\"inline\">{}</code>", escape_html(condition)));
            }
            if let Some(strategy) = &filter.strategy {
                details.push(format!("strategy: <code class=\"inline\">{}</code>", escape_html(strategy)));
            }
            if let Some(page_size) = filter.page_size {
                details.push(format!("page_size: <code class=\"inline\">{}</code>", page_size));
            }

            let details_html = if details.is_empty() {
                "-".to_string()
            } else {
                details.join(", ")
            };
            write!(html, "<td>{}</td>", details_html).unwrap();

            html.push_str("</tr>\n");
        }

        html.push_str("</tbody></table></div>\n");
    }

    // Transforms Summary
    if !doc.usecase.transforms.is_empty() {
        html.push_str("<div class=\"table-section\"><h2>Transforms</h2>\n");
        html.push_str("<table><thead><tr><th>Target</th><th>Type</th><th>Sources</th><th>Details</th></tr></thead><tbody>\n");

        for transform in &doc.usecase.transforms {
            write!(html, "<tr><td><code class=\"inline\">{}</code></td>", escape_html(&transform.target)).unwrap();
            write!(html, "<td><strong>{}</strong></td>", escape_html(&transform.r#type)).unwrap();

            // Sources
            let sources_html = if let Some(sources) = &transform.sources {
                sources.iter().map(|s| format!("<code class=\"inline\">{}</code>", escape_html(s))).collect::<Vec<_>>().join(", ")
            } else if let Some(source) = &transform.source {
                format!("<code class=\"inline\">{}</code>", escape_html(source))
            } else {
                "-".to_string()
            };
            write!(html, "<td>{}</td>", sources_html).unwrap();

            // Details
            let mut details = Vec::new();
            if let Some(separator) = &transform.separator {
                details.push(format!("separator: <code class=\"inline\">{}</code>", escape_html(separator)));
            }
            if let Some(fallback) = &transform.fallback {
                details.push(format!("fallback: <code class=\"inline\">{}</code>", escape_html(fallback)));
            }
            if let Some(when) = &transform.when {
                if !when.is_empty() {
                    details.push(format!("when: {} conditions", when.len()));
                }
            }

            let details_html = if details.is_empty() {
                "-".to_string()
            } else {
                details.join(", ")
            };
            write!(html, "<td>{}</td>", details_html).unwrap();

            html.push_str("</tr>\n");
        }

        html.push_str("</tbody></table></div>\n");
    }
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{Aggregate, Import, Join, ResponseMapping, Transform, Usecase, UsmlDocument};

    #[test]
    fn test_generate_html_contains_sections() {
        let doc = UsmlDocument {
            version: "0.1".to_string(),
            import: Import {
                openapi: None,
                dbml: Some(vec!["./schema.dbml#tables[\"users\"]".to_string()]),
            },
            usecase: Usecase {
                name: "Users".to_string(),
                summary: None,
                response_mapping: vec![ResponseMapping {
                    field: "id".to_string(),
                    source: Some("users.id".to_string()),
                    r#type: None,
                    source_table: None,
                    join: None,
                    join_chain: None,
                    aggregate: None,
                    fields: None,
                }],
                filters: Vec::new(),
                transforms: Vec::new(),
            },
        };

        let html = generate_html(&doc);
        assert!(html.contains("Response Fields"));
        assert!(html.contains("Joins &amp; Transforms"));
        assert!(html.contains("Tables"));
    }

    #[test]
    fn test_generate_html_includes_join_and_badges() {
        let doc = UsmlDocument {
            version: "0.1".to_string(),
            import: Import {
                openapi: None,
                dbml: Some(vec![
                    "./schema.dbml#tables[\"users\"]".to_string(),
                    "./schema.dbml#tables[\"profiles\"]".to_string(),
                ]),
            },
            usecase: Usecase {
                name: "Profiles".to_string(),
                summary: None,
                response_mapping: vec![ResponseMapping {
                    field: "profile_count".to_string(),
                    source: Some("profiles.id".to_string()),
                    r#type: Some("array".to_string()),
                    source_table: None,
                    join: Some(Join {
                        table: "profiles".to_string(),
                        on: "users.id = profiles.user_id".to_string(),
                        r#type: Some("LEFT JOIN".to_string()),
                        alias: None,
                    }),
                    join_chain: None,
                    aggregate: Some(Aggregate {
                        r#type: "COUNT".to_string(),
                        group_by: None,
                    }),
                    fields: None,
                }],
                filters: Vec::new(),
                transforms: vec![Transform {
                    target: "profile_count".to_string(),
                    r#type: "COALESCE".to_string(),
                    source: None,
                    sources: None,
                    fallback: None,
                    separator: None,
                    when: None,
                    else_value: None,
                    mask_pattern: None,
                    condition: None,
                    then_source: None,
                    else_source: None,
                }],
            },
        };

        let html = generate_html(&doc);
        assert!(html.contains("LEFT JOIN profiles ON users.id = profiles.user_id"));
        assert!(html.contains("COUNT"));
        assert!(html.contains("array"));
        assert!(html.contains("COALESCE"));
        assert!(html.contains("profiles"));
    }

    #[test]
    fn test_generate_html_has_flow_svg_and_legend() {
        let doc = UsmlDocument {
            version: "0.1".to_string(),
            import: Import {
                openapi: None,
                dbml: Some(vec!["./schema.dbml#tables[\"users\"]".to_string()]),
            },
            usecase: Usecase {
                name: "Flow Test".to_string(),
                summary: None,
                response_mapping: vec![ResponseMapping {
                    field: "name".to_string(),
                    source: Some("users.name".to_string()),
                    r#type: None,
                    source_table: None,
                    join: None,
                    join_chain: None,
                    aggregate: None,
                    fields: None,
                }],
                filters: Vec::new(),
                transforms: Vec::new(),
            },
        };
        let html = generate_html(&doc);
        assert!(html.contains("flow-svg"), "SVG overlay missing");
        assert!(html.contains("flow-container"), "flow-container missing");
        assert!(html.contains("legend"), "legend missing");
        assert!(html.contains("data-field=\"name\""), "data-field missing");
        assert!(html.contains("data-table=\"users\""), "data-table missing");
        assert!(html.contains("drawFlows"), "JavaScript missing");
    }
}
