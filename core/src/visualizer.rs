use std::collections::HashMap;
use std::fmt::Write;

use crate::ast::{ColumnMapping, Domain, Presentation, ResponseMapping, UsmlDocument};
use crate::resolver;

// ============================================================
// 中間構造（4カラムの描画に必要な情報を一度に組み立てる）
// ============================================================

/// 1つのレスポンスフィールド（response_mapping のエントリ。配列は depth で表現）
struct ResponseFieldEntry {
    /// レスポンスフィールド名
    field: String,
    /// `<Entity>.<field>` 形式のドメイン語彙（解決できた場合）
    source: Option<String>,
    /// 参照しているドメインエンティティ名（解決できた場合）
    entity: Option<String>,
    /// 参照しているドメインフィールド名（解決できた場合）
    domain_field: Option<String>,
    /// 配列か
    is_array: bool,
    /// presentation 変換（種別ラベル）
    presentation_badges: Vec<String>,
    /// このフィールドが（domain 経由で）触れる DB テーブル名
    tables: Vec<String>,
    /// 入れ子の深さ
    depth: usize,
}

/// ドメインフィールド1件の永続化情報（Persistence カラム用）
struct PersistenceField {
    /// `<Entity>.<field>` 形式のキー（ホバー連携）
    domain_key: String,
    entity: String,
    field: String,
    /// マッピング種別: simple / join / join-chain / aggregate
    kind: String,
    /// `<table>.<column>` のソース表現
    source: Option<String>,
    /// JOIN 等の説明行
    detail_lines: Vec<String>,
    /// alias 種別バッジ等
    badges: Vec<String>,
    /// このフィールドが触れる DB テーブル
    tables: Vec<String>,
}

/// テーブルの収集状態（出現順を保持）
struct TableContext {
    /// テーブル名 → 参照カラム集合（出現順）
    columns: HashMap<String, Vec<String>>,
    /// 出現順のテーブル名
    order: Vec<String>,
    /// alias → 実テーブル名
    alias_map: HashMap<String, String>,
}

impl TableContext {
    fn new() -> Self {
        TableContext {
            columns: HashMap::new(),
            order: Vec::new(),
            alias_map: HashMap::new(),
        }
    }

    fn touch_table(&mut self, table: &str) {
        if !self.columns.contains_key(table) {
            self.columns.insert(table.to_string(), Vec::new());
            self.order.push(table.to_string());
        }
    }

    fn add_column(&mut self, table: &str, column: &str) {
        self.touch_table(table);
        let cols = self.columns.get_mut(table).unwrap();
        if !cols.iter().any(|c| c == column) {
            cols.push(column.to_string());
        }
    }
}

// ============================================================
// エントリポイント
// ============================================================

pub fn generate_html(doc: &UsmlDocument) -> String {
    let domain = &doc.domain;
    let presentation_map = build_presentation_map(&doc.usecase.presentation);
    let mut table_ctx = TableContext::new();

    // import.dbml で宣言されたテーブルを先に登録（出現順の基準）
    for table in extract_import_tables(doc) {
        table_ctx.touch_table(&table);
    }

    // Persistence カラムを全エンティティから収集（同時にテーブル/カラムも登録）
    let persistence_fields = collect_persistence(domain, &mut table_ctx);
    // domain_key -> PersistenceField の参照を作るための索引
    let persistence_index: HashMap<&str, &PersistenceField> = persistence_fields
        .iter()
        .map(|p| (p.domain_key.as_str(), p))
        .collect();

    // relations / through テーブルも登録
    register_relation_tables(domain, &mut table_ctx);

    // Response フィールドを収集（domain を辿りテーブルを解決）
    let mut response_entries = Vec::new();
    collect_response(
        &doc.usecase.response_mapping,
        &doc.usecase.root,
        domain,
        &presentation_map,
        &persistence_index,
        0,
        "",
        &mut response_entries,
    );

    render(
        doc,
        domain,
        &response_entries,
        &persistence_fields,
        &table_ctx,
    )
}

// ============================================================
// 収集ロジック
// ============================================================

fn build_presentation_map(presentations: &[Presentation]) -> HashMap<String, Vec<String>> {
    let mut map: HashMap<String, Vec<String>> = HashMap::new();
    for p in presentations {
        map.entry(p.target.clone())
            .or_default()
            .push(p.r#type.clone());
    }
    map
}

fn extract_import_tables(doc: &UsmlDocument) -> Vec<String> {
    let mut tables = Vec::new();
    if let Some(dbmls) = &doc.import.dbml {
        for entry in dbmls {
            if let Some(table) = extract_table_name(entry)
                && !tables.contains(&table)
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

/// `users.id` → ("users", "id")。alias 解決はここではしない
fn split_source(source: &str) -> Option<(String, String)> {
    source
        .split_once('.')
        .map(|(t, c)| (t.to_string(), c.to_string()))
}

/// 全エンティティの persistence.columns を収集し、テーブル/カラムを登録する
fn collect_persistence(domain: &Domain, table_ctx: &mut TableContext) -> Vec<PersistenceField> {
    let mut result = Vec::new();

    for (entity_name, entity) in &domain.entities {
        let root_table = entity.persistence.root_table.clone();
        table_ctx.touch_table(&root_table);

        for (field_name, mapping) in &entity.persistence.columns {
            let domain_key = format!("{}.{}", entity_name, field_name);
            let mut detail_lines = Vec::new();
            let mut badges = Vec::new();
            let mut tables: Vec<String> = Vec::new();
            let source_str;
            let kind;

            match mapping {
                ColumnMapping::Simple(src) => {
                    source_str = Some(src.clone());
                    kind = "simple".to_string();
                    if let Some((table, column)) = split_source(src) {
                        // alias の場合は実テーブルへ寄せる
                        let real = table_ctx
                            .alias_map
                            .get(&table)
                            .cloned()
                            .unwrap_or_else(|| table.clone());
                        table_ctx.add_column(&real, &column);
                        if !tables.contains(&real) {
                            tables.push(real);
                        }
                    }
                }
                ColumnMapping::Detailed(detail) => {
                    source_str = Some(detail.source.clone());

                    // join の alias を先に登録（source のテーブルが alias を指す場合に解決可能にする）
                    if let Some(join) = &detail.join
                        && let Some(alias) = &join.alias
                    {
                        table_ctx
                            .alias_map
                            .insert(alias.clone(), join.table.clone());
                    }

                    // source カラム
                    if let Some((table, column)) = split_source(&detail.source) {
                        let real = table_ctx
                            .alias_map
                            .get(&table)
                            .cloned()
                            .unwrap_or_else(|| table.clone());
                        table_ctx.add_column(&real, &column);
                        if !tables.contains(&real) {
                            tables.push(real);
                        }
                    }

                    if let Some(join) = &detail.join {
                        let jtype = join.r#type.as_deref().unwrap_or("JOIN");
                        let table_part = if let Some(alias) = &join.alias {
                            format!("{} AS {}", join.table, alias)
                        } else {
                            join.table.clone()
                        };
                        detail_lines.push(format!("{} {} ON {}", jtype, table_part, join.on));
                        table_ctx.touch_table(&join.table);
                        if !tables.contains(&join.table) {
                            tables.push(join.table.clone());
                        }
                        if let Some(alias) = &join.alias {
                            badges.push(format!("as {}", alias));
                        }
                    }

                    if let Some(chain) = &detail.join_chain
                        && !chain.is_empty()
                    {
                        let line = chain
                            .iter()
                            .map(|e| format!("JOIN {} ON {}", e.table, e.on))
                            .collect::<Vec<_>>()
                            .join(" → ");
                        detail_lines.push(line);
                        for e in chain {
                            table_ctx.touch_table(&e.table);
                            if !tables.contains(&e.table) {
                                tables.push(e.table.clone());
                            }
                        }
                    }

                    if let Some(agg) = &detail.aggregate {
                        let mut agg_line = agg.r#type.clone();
                        if let Some(group_by) = &agg.group_by {
                            let _ = write!(agg_line, " GROUP BY {}", group_by);
                        }
                        detail_lines.push(agg_line);
                        badges.push(agg.r#type.clone());
                    }

                    kind = if detail.aggregate.is_some() {
                        "aggregate".to_string()
                    } else if detail.join_chain.is_some() {
                        "join-chain".to_string()
                    } else if detail.join.is_some() {
                        "join".to_string()
                    } else {
                        "simple".to_string()
                    };
                }
            }

            // root_table はそのエンティティが触れるテーブルとして常に含める
            if !tables.contains(&root_table) {
                tables.push(root_table.clone());
            }

            result.push(PersistenceField {
                domain_key,
                entity: entity_name.clone(),
                field: field_name.clone(),
                kind,
                source: source_str,
                detail_lines,
                badges,
                tables,
            });
        }
    }

    result
}

/// relations / through で参照されるテーブルを登録
fn register_relation_tables(domain: &Domain, table_ctx: &mut TableContext) {
    for entity in domain.entities.values() {
        for relation in entity.relations.values() {
            if let Some(through) = &relation.through {
                table_ctx.touch_table(&through.table);
            }
            // 関連先エンティティの root_table も触れておく
            if let Some(target) = domain.entities.get(&relation.target) {
                table_ctx.touch_table(&target.persistence.root_table);
            }
        }
    }
}

/// `<Entity>.<field>` を分解
fn split_domain_ref(source: &str) -> Option<(String, String)> {
    source
        .split_once('.')
        .map(|(e, f)| (e.to_string(), f.to_string()))
}

#[allow(clippy::too_many_arguments)]
fn collect_response(
    mappings: &[ResponseMapping],
    current_entity: &str,
    domain: &Domain,
    presentation_map: &HashMap<String, Vec<String>>,
    persistence_index: &HashMap<&str, &PersistenceField>,
    depth: usize,
    parent_path: &str,
    entries: &mut Vec<ResponseFieldEntry>,
) {
    for mapping in mappings {
        let field_path = if parent_path.is_empty() {
            mapping.field.clone()
        } else {
            format!("{}.{}", parent_path, mapping.field)
        };

        let is_array = mapping.r#type.as_deref() == Some("array");
        let presentation_badges = presentation_map
            .get(&mapping.field)
            .cloned()
            .unwrap_or_default();

        let mut entity = None;
        let mut domain_field = None;
        let mut tables: Vec<String> = Vec::new();
        let mut relation_name = None;

        if let Some(source) = &mapping.source
            && let Some((ent, fld)) = split_domain_ref(source)
        {
            if is_array {
                // source は <Entity>.<relation>
                relation_name = Some(fld.clone());
                entity = Some(ent.clone());
            } else {
                entity = Some(ent.clone());
                domain_field = Some(fld.clone());
                // persistence からテーブルを引く
                if let Some(pf) = persistence_index.get(source.as_str()) {
                    for t in &pf.tables {
                        if !tables.contains(t) {
                            tables.push(t.clone());
                        }
                    }
                }
            }
        }

        entries.push(ResponseFieldEntry {
            field: mapping.field.clone(),
            source: mapping.source.clone(),
            entity: entity.clone(),
            domain_field,
            is_array,
            presentation_badges,
            tables,
            depth,
        });

        // 配列フィールド: relation 先エンティティへ展開
        if let Some(fields) = &mapping.fields {
            // 子の current_entity を決定（relation の target を辿る）
            let child_entity = relation_name
                .as_ref()
                .and_then(|rel| {
                    domain
                        .entities
                        .get(current_entity)
                        .and_then(|e| e.relations.get(rel))
                        .map(|r| r.target.clone())
                })
                .unwrap_or_else(|| current_entity.to_string());

            collect_response(
                fields,
                &child_entity,
                domain,
                presentation_map,
                persistence_index,
                depth + 1,
                &field_path,
                entries,
            );
        }
    }
}

// ============================================================
// HTML レンダリング
// ============================================================

fn render(
    doc: &UsmlDocument,
    domain: &Domain,
    response_entries: &[ResponseFieldEntry],
    persistence_fields: &[PersistenceField],
    table_ctx: &TableContext,
) -> String {
    let mut html = String::new();
    html.push_str("<!DOCTYPE html>\n<html lang=\"ja\">\n<head>\n<meta charset=\"utf-8\">\n");
    html.push_str("<title>USML Data Flow Visualizer</title>\n");
    html.push_str("<link rel=\"stylesheet\" href=\"https://cdnjs.cloudflare.com/ajax/libs/font-awesome/6.4.0/css/all.min.css\">\n");
    html.push_str("<style>\n");
    push_styles(&mut html);
    html.push_str("</style>\n</head>\n<body>\n");

    render_header(&mut html, doc);
    render_tabs(&mut html);

    html.push_str("<div class=\"main-content\">\n");

    // ビジュアルビュー（4カラム）
    html.push_str("<div id=\"visual-view\" class=\"view\">\n");
    html.push_str("<div class=\"grid\">\n");
    render_response_column(&mut html, response_entries);
    render_domain_column(&mut html, domain);
    render_persistence_column(&mut html, persistence_fields);
    render_tables_column(&mut html, table_ctx);
    html.push_str("</div>\n</div>\n"); // grid, visual-view

    // テーブルビュー
    html.push_str("<div id=\"table-view\" class=\"view active\">\n");
    render_table_view(
        &mut html,
        doc,
        domain,
        response_entries,
        persistence_fields,
        table_ctx,
    );
    html.push_str("</div>\n");

    html.push_str("</div>\n"); // main-content

    push_script(&mut html);
    html.push_str("</body>\n</html>\n");
    html
}

fn push_styles(html: &mut String) {
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
    html.push_str(".root-badge { display: inline-block; padding: 4px 10px; border-radius: 4px; font-size: 0.75rem; font-weight: 600; background: #ede9fe; color: #5b21b6; }\n");
    html.push_str(".tabs { display: flex; gap: 4px; margin-top: 0; }\n");
    html.push_str(".tab { display: flex; align-items: center; gap: 8px; padding: 12px 24px; background: transparent; color: #6b7280; border: none; border-bottom: 3px solid transparent; cursor: pointer; font-size: 0.95rem; font-weight: 500; transition: all 0.2s; }\n");
    html.push_str(".tab:hover { color: #1f2937; background: #f9fafb; }\n");
    html.push_str(".tab.active { color: #3b82f6; border-bottom-color: #3b82f6; }\n");
    html.push_str(".tab i { font-size: 1.1rem; }\n");
    html.push_str(".main-content { padding: 32px 32px 80px 32px; background: #fff; min-height: calc(100vh - 180px); }\n");
    html.push_str(".view { display: none; }\n");
    html.push_str(".view.active { display: block; }\n");
    html.push_str(
        ".grid { display: grid; grid-template-columns: repeat(4, 1fr); gap: 16px; align-items: start; }\n",
    );
    html.push_str(".column h2 { font-size: 1.05rem; margin-bottom: 4px; }\n");
    html.push_str(
        ".column .col-sub { font-size: 0.78rem; color: #9ca3af; margin-bottom: 12px; }\n",
    );
    html.push_str(
        ".card { border-radius: 12px; padding: 12px 16px; margin-bottom: 12px; box-shadow: 0 4px 12px rgba(15, 23, 42, 0.08); transition: all 0.2s ease; }\n",
    );
    html.push_str(".response-card { background: #e8f4fd; }\n");
    html.push_str(".domain-card { background: #ede9fe; }\n");
    html.push_str(".persistence-card { background: #fff8e1; }\n");
    html.push_str(".table-card { background: #f0faf0; }\n");
    html.push_str(
        ".badge { display: inline-block; background: #6c757d; color: #fff; border-radius: 999px; font-size: 0.72rem; padding: 2px 8px; margin-right: 4px; margin-top: 2px; }\n",
    );
    html.push_str(".badge-pres { background: #db2777; }\n");
    html.push_str(".badge-array { background: #2563eb; }\n");
    html.push_str(".badge-agg { background: #8b5cf6; }\n");
    html.push_str(".badge-vo { background: #0e7490; }\n");
    html.push_str(".badge-derived { background: #b45309; }\n");
    html.push_str(".field-name { font-weight: 600; margin-bottom: 6px; }\n");
    html.push_str(".field-name.small { font-weight: 500; font-size: 0.9rem; color: #394150; }\n");
    html.push_str(".meta-line { font-size: 0.82rem; color: #4b5563; margin-top: 4px; }\n");
    html.push_str(".join-line { font-size: 0.85rem; margin-top: 4px; font-family: 'Monaco', 'Menlo', monospace; color: #374151; }\n");
    html.push_str(".empty { color: #6b7280; font-size: 0.9rem; }\n");
    html.push_str(".entity-title { font-weight: 700; font-size: 1rem; margin-bottom: 8px; color: #4c1d95; }\n");
    html.push_str(".entity-fieldrow { display: flex; justify-content: space-between; font-size: 0.85rem; padding: 3px 0; border-bottom: 1px dashed #d8b4fe; }\n");
    html.push_str(".entity-fieldrow:last-of-type { border-bottom: none; }\n");
    html.push_str(
        ".depth-1 { margin-left: 20px; padding-left: 12px; border-left: 3px solid #3b82f6; }\n",
    );
    html.push_str(
        ".depth-2 { margin-left: 40px; padding-left: 12px; border-left: 3px solid #8b5cf6; }\n",
    );
    html.push_str(
        ".depth-3 { margin-left: 60px; padding-left: 12px; border-left: 3px solid #ec4899; }\n",
    );
    html.push_str(
        ".depth-4 { margin-left: 80px; padding-left: 12px; border-left: 3px solid #f59e0b; }\n",
    );
    html.push_str(".card.highlighted { box-shadow: 0 0 24px rgba(251,191,36,0.9), 0 0 12px rgba(251,191,36,0.6); transform: scale(1.03); border: 2px solid #fbbf24; }\n");
    html.push_str(".entity-fieldrow.highlighted { background: #fde68a; border-radius: 4px; }\n");
    html.push_str("table { width: 100%; border-collapse: collapse; background: #fff; border-radius: 8px; overflow: hidden; box-shadow: 0 2px 8px rgba(0,0,0,0.1); }\n");
    html.push_str("thead { background: #374151; color: #fff; }\n");
    html.push_str(
        "th { padding: 12px 16px; text-align: left; font-weight: 600; font-size: 0.9rem; }\n",
    );
    html.push_str(
        "td { padding: 12px 16px; border-bottom: 1px solid #e5e7eb; vertical-align: top; }\n",
    );
    html.push_str("tbody tr:last-child td { border-bottom: none; }\n");
    html.push_str("tbody tr:hover { background: #f9fafb; }\n");
    html.push_str(".table-section { margin-bottom: 32px; }\n");
    html.push_str(".table-section h2 { font-size: 1.3rem; margin-bottom: 16px; }\n");
    html.push_str(".indent-1 { padding-left: 32px; background: #eff6ff; }\n");
    html.push_str(".indent-2 { padding-left: 48px; background: #f3e8ff; }\n");
    html.push_str(".indent-3 { padding-left: 64px; background: #fce7f3; }\n");
    html.push_str(".indent-4 { padding-left: 80px; background: #fef3c7; }\n");
    html.push_str("code.inline { background: #e5e7eb; padding: 2px 6px; border-radius: 4px; font-size: 0.9em; }\n");
}

fn render_header(html: &mut String, doc: &UsmlDocument) {
    html.push_str("<div class=\"header\">\n");
    let _ = write!(html, "<h1>{}</h1>", escape_html(&doc.usecase.name));
    if let Some(summary) = &doc.usecase.summary {
        let _ = write!(html, "<p class=\"summary\">{}</p>", escape_html(summary));
    }

    html.push_str("<div class=\"api-info\">\n");
    // OpenAPI 情報
    if let Some(openapi_ref) = &doc.import.openapi
        && let Some((_file, path, method, status)) =
            resolver::openapi::parse_openapi_ref(openapi_ref)
    {
        let method_upper = method.to_uppercase();
        let method_class = match method_upper.as_str() {
            "GET" => "method-get",
            "POST" => "method-post",
            "PUT" => "method-put",
            "DELETE" => "method-delete",
            "PATCH" => "method-patch",
            _ => "method-get",
        };
        let _ = write!(
            html,
            "<span class=\"method-badge {}\">{}</span>",
            method_class,
            escape_html(&method_upper)
        );
        let _ = write!(
            html,
            "<span class=\"api-path\">{}</span>",
            escape_html(path)
        );
        let _ = write!(
            html,
            "<span class=\"status-badge\">Status: {}</span>",
            escape_html(status)
        );
    }
    // ルートエンティティ
    let _ = write!(
        html,
        "<span class=\"root-badge\">Root: {}</span>",
        escape_html(&doc.usecase.root)
    );
    html.push_str("</div>\n");
}

fn render_tabs(html: &mut String) {
    html.push_str("<div class=\"tabs\">\n");
    html.push_str("<button class=\"tab active\" onclick=\"switchView('table', event)\"><i class=\"fas fa-table\"></i> テーブル</button>\n");
    html.push_str("<button class=\"tab\" onclick=\"switchView('visual', event)\"><i class=\"fas fa-project-diagram\"></i> ビジュアル</button>\n");
    html.push_str("</div></div>\n");
}

// --- カラム1: Response Fields ---
fn render_response_column(html: &mut String, entries: &[ResponseFieldEntry]) {
    html.push_str("<div class=\"column\">\n<h2>Response Fields</h2>\n<div class=\"col-sub\">API レスポンス（Interface 層）</div>\n");
    if entries.is_empty() {
        html.push_str("<div class=\"empty\">レスポンスマッピングがありません。</div>");
    } else {
        for entry in entries {
            let depth_class = depth_class(entry.depth);
            let _ = write!(
                html,
                "<div class=\"card response-card{}\" data-entity=\"{}\" data-domainfield=\"{}\" data-tables=\"{}\">",
                depth_class,
                escape_html(entry.entity.as_deref().unwrap_or("")),
                escape_html(
                    entry
                        .domain_field
                        .as_ref()
                        .map(|f| format!("{}.{}", entry.entity.as_deref().unwrap_or(""), f))
                        .unwrap_or_default()
                        .as_str()
                ),
                escape_html(&entry.tables.join(","))
            );
            let _ = write!(
                html,
                "<div class=\"field-name\">{}</div>",
                escape_html(&entry.field)
            );

            // バッジ
            if entry.is_array || !entry.presentation_badges.is_empty() {
                html.push_str("<div>");
                if entry.is_array {
                    html.push_str("<span class=\"badge badge-array\">array</span>");
                }
                for b in &entry.presentation_badges {
                    let _ = write!(
                        html,
                        "<span class=\"badge badge-pres\">{}</span>",
                        escape_html(b)
                    );
                }
                html.push_str("</div>");
            }

            // ソース（ドメイン語彙）
            if let Some(source) = &entry.source {
                let _ = write!(
                    html,
                    "<div class=\"meta-line\">← {}</div>",
                    escape_html(source)
                );
            }
            html.push_str("</div>\n");
        }
    }
    html.push_str("</div>\n");
}

// --- カラム2: Domain Entities ---
fn render_domain_column(html: &mut String, domain: &Domain) {
    html.push_str("<div class=\"column\">\n<h2>Domain Entities</h2>\n<div class=\"col-sub\">エンティティ・VO型・derived 導出</div>\n");
    if domain.entities.is_empty() {
        html.push_str("<div class=\"empty\">エンティティが定義されていません。</div>");
    } else {
        // VO 名 → base/format の索引（型表示用）
        let vo_index: HashMap<&str, &crate::ast::ValueObject> = domain
            .value_objects
            .iter()
            .map(|vo| (vo.name.as_str(), vo))
            .collect();

        for (entity_name, entity) in &domain.entities {
            let _ = write!(
                html,
                "<div class=\"card domain-card\" data-domain-entity=\"{}\">",
                escape_html(entity_name)
            );
            let _ = write!(
                html,
                "<div class=\"entity-title\">{}</div>",
                escape_html(entity_name)
            );

            // フィールド一覧（型・VO バッジ）
            for (fname, ftype) in &entity.fields {
                let _ = write!(
                    html,
                    "<div class=\"entity-fieldrow\" data-domain-field=\"{}.{}\"><span>{}</span><span>",
                    escape_html(entity_name),
                    escape_html(fname),
                    escape_html(fname)
                );
                if let Some(vo) = vo_index.get(ftype.as_str()) {
                    let format_part = vo
                        .format
                        .as_ref()
                        .map(|f| format!(":{}", f))
                        .unwrap_or_default();
                    let _ = write!(
                        html,
                        "<span class=\"badge badge-vo\">{} ({}{})</span>",
                        escape_html(ftype),
                        escape_html(&vo.base),
                        escape_html(&format_part)
                    );
                } else {
                    let _ = write!(html, "<code class=\"inline\">{}</code>", escape_html(ftype));
                }
                html.push_str("</span></div>");
            }

            // derived
            for d in &entity.derived {
                let detail = describe_derived(d);
                let _ = write!(
                    html,
                    "<div class=\"meta-line\"><span class=\"badge badge-derived\">derived {}</span> {} = {}</div>",
                    escape_html(&d.r#type),
                    escape_html(&d.field),
                    escape_html(&detail)
                );
            }

            // relations
            for (rel_name, rel) in &entity.relations {
                let _ = write!(
                    html,
                    "<div class=\"meta-line\">→ <strong>{}</strong> [{}] {}</div>",
                    escape_html(rel_name),
                    escape_html(&rel.kind),
                    escape_html(&rel.target)
                );
            }

            html.push_str("</div>\n");
        }
    }
    html.push_str("</div>\n");
}

fn describe_derived(d: &crate::ast::Derived) -> String {
    match d.r#type.as_str() {
        "COALESCE" => {
            let mut parts: Vec<String> = d.sources.clone().unwrap_or_default();
            if let Some(fb) = &d.fallback {
                parts.push(format!("\"{}\"", fb));
            }
            parts.join(" ?? ")
        }
        "CONCAT" => {
            let sep = d.separator.as_deref().unwrap_or("");
            d.sources
                .clone()
                .unwrap_or_default()
                .join(&format!(" {} ", sep))
        }
        "CONDITIONAL_SOURCE" => {
            let then_s = d.then_source.as_deref().unwrap_or("?");
            let else_s = d.else_source.as_deref().unwrap_or("?");
            format!("if(cond) {} else {}", then_s, else_s)
        }
        _ => d.source.clone().unwrap_or_default(),
    }
}

// --- カラム3: Persistence ---
fn render_persistence_column(html: &mut String, fields: &[PersistenceField]) {
    html.push_str("<div class=\"column\">\n<h2>Persistence</h2>\n<div class=\"col-sub\">Domain ⇄ DB（JOIN / 集約 / alias）</div>\n");
    if fields.is_empty() {
        html.push_str("<div class=\"empty\">永続化マッピングがありません。</div>");
    } else {
        for pf in fields {
            let _ = write!(
                html,
                "<div class=\"card persistence-card\" data-domainfield=\"{}\" data-tables=\"{}\">",
                escape_html(&pf.domain_key),
                escape_html(&pf.tables.join(","))
            );
            let _ = write!(
                html,
                "<div class=\"field-name small\">{}.{}</div>",
                escape_html(&pf.entity),
                escape_html(&pf.field)
            );

            // 種別バッジ
            let kind_label = match pf.kind.as_str() {
                "simple" => "Simple",
                "join" => "JOIN",
                "join-chain" => "JOIN Chain",
                "aggregate" => "Aggregate",
                _ => "Simple",
            };
            let kind_class = if pf.kind == "aggregate" {
                "badge badge-agg"
            } else {
                "badge"
            };
            html.push_str("<div>");
            let _ = write!(html, "<span class=\"{}\">{}</span>", kind_class, kind_label);
            for b in &pf.badges {
                let _ = write!(html, "<span class=\"badge\">{}</span>", escape_html(b));
            }
            html.push_str("</div>");

            if let Some(source) = &pf.source {
                let _ = write!(
                    html,
                    "<div class=\"meta-line\">source: <code class=\"inline\">{}</code></div>",
                    escape_html(source)
                );
            }
            for line in &pf.detail_lines {
                let _ = write!(html, "<div class=\"join-line\">{}</div>", escape_html(line));
            }
            html.push_str("</div>\n");
        }
    }
    html.push_str("</div>\n");
}

// --- カラム4: Tables ---
fn render_tables_column(html: &mut String, table_ctx: &TableContext) {
    html.push_str("<div class=\"column\">\n<h2>Tables</h2>\n<div class=\"col-sub\">DB テーブル（Infrastructure 層）</div>\n");
    if table_ctx.order.is_empty() {
        html.push_str("<div class=\"empty\">テーブルがインポートされていません。</div>");
    } else {
        for table in &table_ctx.order {
            let _ = write!(
                html,
                "<div class=\"card table-card\" data-table=\"{}\"><div class=\"field-name\">{}</div>",
                escape_html(table),
                escape_html(table)
            );
            match table_ctx.columns.get(table) {
                Some(cols) if !cols.is_empty() => {
                    html.push_str(
                        "<div class=\"join-line\">Columns:</div><div style=\"margin-top: 4px;\">",
                    );
                    for (i, col) in cols.iter().enumerate() {
                        if i > 0 {
                            html.push_str(", ");
                        }
                        let _ = write!(html, "<code class=\"inline\">{}</code>", escape_html(col));
                    }
                    html.push_str("</div>");
                }
                _ => {
                    html.push_str(
                        "<div class=\"join-line\" style=\"color: #9ca3af;\">参照カラムなし</div>",
                    );
                }
            }
            html.push_str("</div>\n");
        }
    }
    html.push_str("</div>\n");
}

// ============================================================
// テーブルビュー（domain 情報も反映）
// ============================================================

fn render_table_view(
    html: &mut String,
    doc: &UsmlDocument,
    domain: &Domain,
    response_entries: &[ResponseFieldEntry],
    persistence_fields: &[PersistenceField],
    table_ctx: &TableContext,
) {
    // Response Mapping
    html.push_str("<div class=\"table-section\"><h2>Response Mapping</h2>\n");
    html.push_str("<table><thead><tr><th>Field</th><th>Domain Source</th><th>Type</th><th>Presentation</th><th>Tables</th></tr></thead><tbody>\n");
    for entry in response_entries {
        let indent_class = match entry.depth {
            1 => " class=\"indent-1\"",
            2 => " class=\"indent-2\"",
            3 => " class=\"indent-3\"",
            4 => " class=\"indent-4\"",
            _ => "",
        };
        let _ = write!(html, "<tr{}>", indent_class);

        let field_display = if entry.depth > 0 {
            format!("{}└─ {}", "  ".repeat(entry.depth), entry.field)
        } else {
            entry.field.clone()
        };
        let _ = write!(
            html,
            "<td><code class=\"inline\">{}</code></td>",
            escape_html(&field_display)
        );
        let _ = write!(
            html,
            "<td>{}</td>",
            escape_html(entry.source.as_deref().unwrap_or("-"))
        );
        let type_str = if entry.is_array { "array" } else { "-" };
        let _ = write!(html, "<td>{}</td>", type_str);
        let pres = if entry.presentation_badges.is_empty() {
            "-".to_string()
        } else {
            entry.presentation_badges.join(", ")
        };
        let _ = write!(html, "<td>{}</td>", escape_html(&pres));
        let tables = if entry.tables.is_empty() {
            "-".to_string()
        } else {
            entry.tables.join(", ")
        };
        let _ = write!(html, "<td>{}</td>", escape_html(&tables));
        html.push_str("</tr>\n");
    }
    html.push_str("</tbody></table></div>\n");

    // Domain Entities
    html.push_str("<div class=\"table-section\"><h2>Domain Entities</h2>\n");
    html.push_str("<table><thead><tr><th>Entity</th><th>Field</th><th>Type</th><th>Root Table</th></tr></thead><tbody>\n");
    let vo_index: HashMap<&str, &crate::ast::ValueObject> = domain
        .value_objects
        .iter()
        .map(|vo| (vo.name.as_str(), vo))
        .collect();
    for (entity_name, entity) in &domain.entities {
        if entity.fields.is_empty() {
            let _ = writeln!(
                html,
                "<tr><td><strong>{}</strong></td><td>-</td><td>-</td><td><code class=\"inline\">{}</code></td></tr>",
                escape_html(entity_name),
                escape_html(&entity.persistence.root_table)
            );
            continue;
        }
        let mut first = true;
        for (fname, ftype) in &entity.fields {
            let type_str = if let Some(vo) = vo_index.get(ftype.as_str()) {
                let fmt = vo
                    .format
                    .as_ref()
                    .map(|f| format!(", {}", f))
                    .unwrap_or_default();
                format!("{} ({}{})", ftype, vo.base, fmt)
            } else {
                ftype.clone()
            };
            let entity_cell = if first {
                format!("<strong>{}</strong>", escape_html(entity_name))
            } else {
                String::new()
            };
            let root_cell = if first {
                format!(
                    "<code class=\"inline\">{}</code>",
                    escape_html(&entity.persistence.root_table)
                )
            } else {
                String::new()
            };
            let _ = writeln!(
                html,
                "<tr><td>{}</td><td><code class=\"inline\">{}</code></td><td>{}</td><td>{}</td></tr>",
                entity_cell,
                escape_html(fname),
                escape_html(&type_str),
                root_cell
            );
            first = false;
        }
    }
    html.push_str("</tbody></table></div>\n");

    // Persistence Mapping
    html.push_str("<div class=\"table-section\"><h2>Persistence Mapping</h2>\n");
    html.push_str("<table><thead><tr><th>Domain Field</th><th>Type</th><th>Source</th><th>JOIN / Aggregate</th></tr></thead><tbody>\n");
    for pf in persistence_fields {
        let kind_label = match pf.kind.as_str() {
            "simple" => "Simple",
            "join" => "JOIN",
            "join-chain" => "JOIN Chain",
            "aggregate" => "Aggregate",
            _ => "Simple",
        };
        let _ = write!(
            html,
            "<tr><td><code class=\"inline\">{}</code></td><td>{}</td>",
            escape_html(&pf.domain_key),
            kind_label
        );
        let _ = write!(
            html,
            "<td><code class=\"inline\">{}</code></td>",
            escape_html(pf.source.as_deref().unwrap_or("-"))
        );
        let detail = if pf.detail_lines.is_empty() {
            "-".to_string()
        } else {
            pf.detail_lines
                .iter()
                .map(|l| escape_html(l))
                .collect::<Vec<_>>()
                .join("<br>")
        };
        let _ = writeln!(html, "<td>{}</td></tr>", detail);
    }
    html.push_str("</tbody></table></div>\n");

    // Derived
    let has_derived = domain.entities.values().any(|e| !e.derived.is_empty());
    if has_derived {
        html.push_str("<div class=\"table-section\"><h2>Derived (Domain 導出)</h2>\n");
        html.push_str("<table><thead><tr><th>Entity</th><th>Field</th><th>Type</th><th>Detail</th></tr></thead><tbody>\n");
        for (entity_name, entity) in &domain.entities {
            for d in &entity.derived {
                let _ = writeln!(
                    html,
                    "<tr><td><strong>{}</strong></td><td><code class=\"inline\">{}</code></td><td>{}</td><td><code class=\"inline\">{}</code></td></tr>",
                    escape_html(entity_name),
                    escape_html(&d.field),
                    escape_html(&d.r#type),
                    escape_html(&describe_derived(d))
                );
            }
        }
        html.push_str("</tbody></table></div>\n");
    }

    // Tables Summary
    html.push_str("<div class=\"table-section\"><h2>Tables Summary</h2>\n");
    html.push_str("<table><thead><tr><th>Table</th><th>Columns</th></tr></thead><tbody>\n");
    for table in &table_ctx.order {
        let _ = write!(html, "<tr><td><strong>{}</strong></td>", escape_html(table));
        match table_ctx.columns.get(table) {
            Some(cols) if !cols.is_empty() => {
                let cols_html = cols
                    .iter()
                    .map(|c| format!("<code class=\"inline\">{}</code>", escape_html(c)))
                    .collect::<Vec<_>>()
                    .join(", ");
                let _ = write!(html, "<td>{}</td>", cols_html);
            }
            _ => html.push_str("<td style=\"color: #9ca3af;\">参照カラムなし</td>"),
        }
        html.push_str("</tr>\n");
    }
    html.push_str("</tbody></table></div>\n");

    // Filters
    if !doc.usecase.filters.is_empty() {
        html.push_str("<div class=\"table-section\"><h2>Filters</h2>\n");
        html.push_str("<table><thead><tr><th>Parameter</th><th>Maps To</th><th>Details</th></tr></thead><tbody>\n");
        for filter in &doc.usecase.filters {
            let _ = write!(
                html,
                "<tr><td><code class=\"inline\">{}</code></td><td><strong>{}</strong></td>",
                escape_html(&filter.param),
                escape_html(&filter.maps_to)
            );
            let mut details = Vec::new();
            if let Some(condition) = &filter.condition {
                details.push(format!(
                    "<code class=\"inline\">{}</code>",
                    escape_html(condition)
                ));
            }
            if let Some(strategy) = &filter.strategy {
                details.push(format!(
                    "strategy: <code class=\"inline\">{}</code>",
                    escape_html(strategy)
                ));
            }
            if let Some(page_size) = filter.page_size {
                details.push(format!(
                    "page_size: <code class=\"inline\">{}</code>",
                    page_size
                ));
            }
            let details_html = if details.is_empty() {
                "-".to_string()
            } else {
                details.join(", ")
            };
            let _ = writeln!(html, "<td>{}</td></tr>", details_html);
        }
        html.push_str("</tbody></table></div>\n");
    }

    // Presentation
    if !doc.usecase.presentation.is_empty() {
        html.push_str("<div class=\"table-section\"><h2>Presentation (表示変換)</h2>\n");
        html.push_str(
            "<table><thead><tr><th>Target</th><th>Type</th><th>Detail</th></tr></thead><tbody>\n",
        );
        for p in &doc.usecase.presentation {
            let _ = write!(
                html,
                "<tr><td><code class=\"inline\">{}</code></td><td><strong>{}</strong></td>",
                escape_html(&p.target),
                escape_html(&p.r#type)
            );
            let mut details = Vec::new();
            if let Some(source) = &p.source {
                details.push(format!(
                    "source: <code class=\"inline\">{}</code>",
                    escape_html(source)
                ));
            }
            if let Some(pattern) = &p.mask_pattern {
                details.push(format!(
                    "mask: <code class=\"inline\">{}</code>",
                    escape_html(pattern)
                ));
            }
            if let Some(when) = &p.when
                && !when.is_empty()
            {
                details.push(format!("{} 分岐", when.len()));
            }
            if let Some(else_value) = &p.else_value {
                details.push(format!(
                    "else: <code class=\"inline\">{}</code>",
                    escape_html(else_value)
                ));
            }
            if let Some(condition) = &p.condition
                && !condition.is_empty()
            {
                details.push(format!("{} 条件", condition.len()));
            }
            let details_html = if details.is_empty() {
                "-".to_string()
            } else {
                details.join(", ")
            };
            let _ = writeln!(html, "<td>{}</td></tr>", details_html);
        }
        html.push_str("</tbody></table></div>\n");
    }
}

fn depth_class(depth: usize) -> String {
    if depth == 0 {
        String::new()
    } else {
        format!(" depth-{}", depth.min(4))
    }
}

// ============================================================
// JS（ホバー連鎖ハイライト: Response → Domain → Persistence → Table）
// ============================================================

fn push_script(html: &mut String) {
    html.push_str(r#"<script>
function switchView(viewName, event) {
  document.querySelectorAll('.view').forEach(function(v) { v.classList.remove('active'); });
  document.querySelectorAll('.tab').forEach(function(b) { b.classList.remove('active'); });
  document.getElementById(viewName + '-view').classList.add('active');
  if (event && event.target) {
    var btn = event.target.closest('.tab');
    if (btn) btn.classList.add('active');
  }
}

(function() {
  function clearAll() {
    document.querySelectorAll('.highlighted').forEach(function(c) { c.classList.remove('highlighted'); });
  }

  function highlightChain(domainfield, entity, tables) {
    // Response 側（同じ domainfield を指すカード）
    if (domainfield) {
      document.querySelectorAll('[data-domainfield="' + domainfield + '"]').forEach(function(c) { c.classList.add('highlighted'); });
    }
    // Domain エンティティカード + 該当フィールド行
    if (entity) {
      var ec = document.querySelector('[data-domain-entity="' + entity + '"]');
      if (ec) ec.classList.add('highlighted');
    }
    if (domainfield) {
      document.querySelectorAll('[data-domain-field="' + domainfield + '"]').forEach(function(r) { r.classList.add('highlighted'); });
    }
    // Tables
    (tables || []).forEach(function(t) {
      if (!t) return;
      var tc = document.querySelector('.table-card[data-table="' + t + '"]');
      if (tc) tc.classList.add('highlighted');
    });
  }

  function setupHover() {
    // Response → 下流
    document.querySelectorAll('.response-card').forEach(function(card) {
      card.addEventListener('mouseenter', function() {
        card.classList.add('highlighted');
        var domainfield = card.dataset.domainfield;
        var entity = card.dataset.entity;
        var tables = (card.dataset.tables || '').split(',').filter(function(t) { return t.length > 0; });
        highlightChain(domainfield, entity, tables);
      });
      card.addEventListener('mouseleave', clearAll);
    });

    // Persistence → Response/Domain/Table（双方向）
    document.querySelectorAll('.persistence-card').forEach(function(card) {
      card.addEventListener('mouseenter', function() {
        card.classList.add('highlighted');
        var domainfield = card.dataset.domainfield;
        var entity = domainfield ? domainfield.split('.')[0] : '';
        var tables = (card.dataset.tables || '').split(',').filter(function(t) { return t.length > 0; });
        highlightChain(domainfield, entity, tables);
      });
      card.addEventListener('mouseleave', clearAll);
    });

    // Domain エンティティカード → 関連する全フィールド経路
    document.querySelectorAll('.domain-card').forEach(function(card) {
      card.addEventListener('mouseenter', function() {
        card.classList.add('highlighted');
        var entity = card.dataset.domainEntity;
        document.querySelectorAll('[data-domainfield^="' + entity + '."]').forEach(function(c) { c.classList.add('highlighted'); });
      });
      card.addEventListener('mouseleave', clearAll);
    });
  }

  window.addEventListener('load', setupHover);
})();
</script>
"#);
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
    use crate::ast::{
        ColumnMapping, DetailedColumn, Domain, Entity, Import, Join, Persistence, ResponseMapping,
        Usecase, UsmlDocument, ValueObject,
    };
    use indexmap::IndexMap;

    fn sample_doc() -> UsmlDocument {
        let mut user_fields = IndexMap::new();
        user_fields.insert("id".to_string(), "UserId".to_string());
        user_fields.insert("name".to_string(), "string".to_string());
        user_fields.insert("avatarUrl".to_string(), "Url".to_string());

        let mut user_columns = IndexMap::new();
        user_columns.insert(
            "id".to_string(),
            ColumnMapping::Simple("users.id".to_string()),
        );
        user_columns.insert(
            "name".to_string(),
            ColumnMapping::Simple("users.name".to_string()),
        );
        user_columns.insert(
            "avatarUrl".to_string(),
            ColumnMapping::Detailed(DetailedColumn {
                source: "profiles.avatar_url".to_string(),
                join: Some(Join {
                    table: "profiles".to_string(),
                    on: "users.id = profiles.user_id".to_string(),
                    r#type: Some("LEFT JOIN".to_string()),
                    alias: None,
                }),
                join_chain: None,
                aggregate: None,
            }),
        );

        let mut entities = IndexMap::new();
        entities.insert(
            "User".to_string(),
            Entity {
                fields: user_fields,
                persistence: Persistence {
                    root_table: "users".to_string(),
                    columns: user_columns,
                },
                derived: Vec::new(),
                relations: IndexMap::new(),
            },
        );

        UsmlDocument {
            version: "0.2".to_string(),
            import: Import {
                openapi: None,
                dbml: Some(vec![
                    "./schema.dbml#tables[\"users\"]".to_string(),
                    "./schema.dbml#tables[\"profiles\"]".to_string(),
                ]),
                ddml: None,
            },
            domain: Domain {
                value_objects: vec![ValueObject {
                    name: "Url".to_string(),
                    base: "string".to_string(),
                    format: Some("uri".to_string()),
                }],
                entities,
            },
            usecase: Usecase {
                name: "ユーザー一覧取得".to_string(),
                summary: Some("テスト".to_string()),
                output: None,
                root: "User".to_string(),
                response_mapping: vec![
                    ResponseMapping {
                        field: "id".to_string(),
                        source: Some("User.id".to_string()),
                        r#type: None,
                        fields: None,
                    },
                    ResponseMapping {
                        field: "avatar_url".to_string(),
                        source: Some("User.avatarUrl".to_string()),
                        r#type: None,
                        fields: None,
                    },
                ],
                filters: Vec::new(),
                presentation: Vec::new(),
            },
        }
    }

    #[test]
    fn test_generate_html_contains_four_columns() {
        let html = generate_html(&sample_doc());
        assert!(html.contains("Response Fields"));
        assert!(html.contains("Domain Entities"));
        assert!(html.contains("Persistence"));
        assert!(html.contains("Tables"));
        // 4カラムグリッド
        assert!(html.contains("repeat(4, 1fr)"));
    }

    #[test]
    fn test_generate_html_includes_domain_and_persistence() {
        let html = generate_html(&sample_doc());
        // ドメイン語彙
        assert!(html.contains("User.avatarUrl"));
        // VO 型バッジ
        assert!(html.contains("Url"));
        assert!(html.contains("uri"));
        // JOIN
        assert!(html.contains("LEFT JOIN profiles ON users.id = profiles.user_id"));
        // テーブルとカラム
        assert!(html.contains("profiles"));
        assert!(html.contains("avatar_url"));
        // ルートエンティティ
        assert!(html.contains("Root: User"));
    }

    #[test]
    fn test_root_badge_present() {
        let html = generate_html(&sample_doc());
        assert!(html.contains("root-badge"));
    }
}
