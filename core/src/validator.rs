use std::collections::HashMap;
use std::path::Path;

use thiserror::Error;

use crate::ast::{ResponseMapping, UsmlDocument};
use crate::resolver::{self, DbmlTable, OpenapiResponse};

/// 解決済みの外部スキーマ情報
pub struct ResolveContext {
    pub openapi: Option<OpenapiResponse>,
    pub dbml_tables: Vec<DbmlTable>,
}

#[derive(Debug, Error, PartialEq)]
pub enum ValidationError {
    #[error("バリデーション[{0}]: {1}")]
    Rule(String, String),
    #[error("警告[{0}]: {1}")]
    Warning(String, String),
}

/// バリデーション結果を収集する
pub fn validate(doc: &UsmlDocument) -> Vec<ValidationError> {
    let mut errors = Vec::new();
    let imported_tables = parse_imported_tables(doc);

    validate_imports(doc, &imported_tables, &mut errors);
    validate_response_mapping(&doc.usecase.response_mapping, &imported_tables, &mut errors);
    validate_filters(doc, &mut errors);
    validate_transforms(doc, &mut errors);

    errors
}

/// import 宣言を実際に解決する
fn resolve_imports(doc: &UsmlDocument, base_dir: &str) -> (ResolveContext, Vec<ValidationError>) {
    let mut errors = Vec::new();
    let mut ctx = ResolveContext {
        openapi: None,
        dbml_tables: Vec::new(),
    };

    // OpenAPI 解決
    if let Some(openapi_ref) = &doc.import.openapi
        && let Some((file, path, method, status)) =
            resolver::openapi::parse_openapi_ref(openapi_ref)
    {
        let full_path = Path::new(base_dir).join(file).to_string_lossy().to_string();
        match resolver::openapi::resolve_openapi(&full_path, path, method, status) {
            Ok(resp) => ctx.openapi = Some(resp),
            Err(e) => errors.push(ValidationError::Warning(
                "import.openapi".to_string(),
                format!("OpenAPI解決に失敗しました: {}", e),
            )),
        }
    }

    // DBML 解決
    if let Some(dbml_refs) = &doc.import.dbml {
        for dbml_ref in dbml_refs {
            if let Some((file, _table_name)) = resolver::dbml::parse_dbml_ref(dbml_ref) {
                let full_path = Path::new(base_dir).join(file).to_string_lossy().to_string();
                match resolver::dbml::resolve_dbml(&full_path) {
                    Ok(tables) => {
                        for table in tables {
                            if !ctx.dbml_tables.iter().any(|t| t.name == table.name) {
                                ctx.dbml_tables.push(table);
                            }
                        }
                    }
                    Err(e) => errors.push(ValidationError::Warning(
                        "import.dbml".to_string(),
                        format!("DBML解決に失敗しました: {}", e),
                    )),
                }
            }
        }
    }

    (ctx, errors)
}

/// リゾルバーを使用したバリデーション
/// base_dir: import参照のファイルパスを解決するための基準ディレクトリ
pub fn validate_with_resolve(doc: &UsmlDocument, base_dir: &str) -> Vec<ValidationError> {
    let mut errors = Vec::new();

    // まず基本バリデーション実行
    errors.extend(validate(doc));

    // 外部ファイル解決
    let (ctx, resolve_errors) = resolve_imports(doc, base_dir);
    errors.extend(resolve_errors);

    // Rule 1: OpenAPIレスポンスフィールドとの照合
    if let Some(ref openapi) = ctx.openapi {
        validate_openapi_fields(&doc.usecase.response_mapping, openapi, &mut errors);
    }

    // Rule 3: DBMLカラム存在確認
    if !ctx.dbml_tables.is_empty() {
        validate_dbml_columns(&doc.usecase.response_mapping, &ctx.dbml_tables, &mut errors);
    }

    // Rule 10アップグレード: OpenAPIパラメータの存在確認
    if let Some(ref openapi) = ctx.openapi {
        validate_transform_params(&doc.usecase.transforms, openapi, &mut errors);
    }

    errors
}

/// import.dbml から テーブル名のリストを抽出する
fn parse_imported_tables(doc: &UsmlDocument) -> Vec<String> {
    match &doc.import.dbml {
        Some(refs) => refs
            .iter()
            .filter_map(|r| {
                r.split("tables[\"")
                    .nth(1)
                    .and_then(|s| s.strip_suffix("\"]"))
                    .map(|s| s.to_string())
            })
            .collect(),
        None => Vec::new(),
    }
}

/// join.on の式から テーブル名.カラム名 パターンを抽出する
fn extract_table_refs(on_expr: &str) -> Vec<(String, String)> {
    let mut refs = Vec::new();
    for token in on_expr.split_whitespace() {
        let clean = token.trim_matches(|c: char| !c.is_alphanumeric() && c != '.' && c != '_');
        if let Some((table, col)) = clean.split_once('.')
            && !table.is_empty()
            && !col.is_empty()
            && col.chars().all(|c| c.is_alphanumeric() || c == '_')
        {
            refs.push((table.to_string(), col.to_string()));
        }
    }
    refs
}

/// Rule 2: source で使われるテーブルが import.dbml に含まれるか
fn validate_imports(
    doc: &UsmlDocument,
    imported_tables: &[String],
    errors: &mut Vec<ValidationError>,
) {
    collect_used_tables(&doc.usecase.response_mapping)
        .into_iter()
        .for_each(|table| {
            if !imported_tables.contains(&table) {
                errors.push(ValidationError::Rule(
                    "import.dbml".to_string(),
                    format!("テーブル '{}' が import.dbml に含まれていません", table),
                ));
            }
        });
}

/// response_mapping の結合・エイリアス・集約・配列規則を検証
fn validate_response_mapping(
    mappings: &[ResponseMapping],
    imported_tables: &[String],
    errors: &mut Vec<ValidationError>,
) {
    let mut join_map: HashMap<String, (String, Option<String>)> = HashMap::new();

    validate_response_mapping_inner(mappings, imported_tables, &mut join_map, errors);
}

fn validate_response_mapping_inner(
    mappings: &[ResponseMapping],
    imported_tables: &[String],
    join_map: &mut HashMap<String, (String, Option<String>)>,
    errors: &mut Vec<ValidationError>,
) {
    for mapping in mappings {
        // Rule 7: 同テーブルが異なる結合条件で複数参照される場合に alias が必要
        if let Some(join) = &mapping.join {
            let key = join.table.clone();
            if let Some((existing_on, existing_alias)) = join_map.get(&key) {
                if *existing_on != join.on && join.alias.is_none() && existing_alias.is_none() {
                    errors.push(ValidationError::Rule(
                        "join.alias".to_string(),
                        format!(
                            "テーブル '{}' が異なる結合条件で複数参照されていますが、alias が指定されていません",
                            join.table
                        ),
                    ));
                }
            } else {
                join_map.insert(key, (join.on.clone(), join.alias.clone()));
            }

            // Rule 6: join.on で参照されるテーブルが import.dbml に含まれるか
            let refs = extract_table_refs(&join.on);
            for (table, _col) in &refs {
                // エイリアス名は検証対象外
                if let Some(alias) = &join.alias
                    && table == alias
                {
                    continue;
                }
                if !imported_tables.contains(table) {
                    errors.push(ValidationError::Rule(
                        "join.on".to_string(),
                        format!(
                            "join.on で参照されるテーブル '{}' が import.dbml に含まれていません",
                            table
                        ),
                    ));
                }
            }
        }

        // Rule 6: join_chain で参照されるテーブルも検証
        if let Some(chain) = &mapping.join_chain {
            for entry in chain {
                let refs = extract_table_refs(&entry.on);
                for (table, _col) in &refs {
                    if !imported_tables.contains(table) {
                        errors.push(ValidationError::Rule(
                            "join_chain.on".to_string(),
                            format!(
                                "join_chain.on で参照されるテーブル '{}' が import.dbml に含まれていません",
                                table
                            ),
                        ));
                    }
                }
            }
        }

        // Rule 8: aggregate を使用するフィールドに group_by が明示されているか（警告）
        if let Some(agg) = &mapping.aggregate
            && agg.group_by.is_none()
        {
            errors.push(ValidationError::Warning(
                "aggregate.group_by".to_string(),
                format!(
                    "フィールド '{}' に aggregate ({}) が使われていますが group_by が指定されていません。省略時はルートテーブルの主キーが自動適用されます",
                    mapping.field, agg.r#type
                ),
            ));
        }

        // Rule 11: source_table が配列フィールドの join で参照されるテーブルと一致するか
        if mapping.r#type.as_deref() == Some("array")
            && let (Some(source_table), Some(join)) = (&mapping.source_table, &mapping.join)
        {
            // join_chain がある場合は最後のテーブルが実際のソース
            let actual_source = if let Some(chain) = &mapping.join_chain {
                chain
                    .last()
                    .map(|e| e.table.as_str())
                    .unwrap_or(&join.table)
            } else {
                &join.table
            };
            if source_table != actual_source {
                errors.push(ValidationError::Rule(
                    "source_table".to_string(),
                    format!(
                        "配列フィールド '{}' の source_table '{}' がjoin の実際のソーステーブル '{}' と一致しません",
                        mapping.field, source_table, actual_source
                    ),
                ));
            }
        }

        // 配列フィールドの再帰検証
        if let Some(sub_fields) = &mapping.fields {
            validate_response_mapping_inner(sub_fields, imported_tables, join_map, errors);
        }
    }
}

/// Rule 9, 12: filters の検証
fn validate_filters(doc: &UsmlDocument, errors: &mut Vec<ValidationError>) {
    let declared_params: Vec<&str> = doc
        .usecase
        .filters
        .iter()
        .map(|f| f.param.as_str())
        .collect();

    for filter in &doc.usecase.filters {
        // Rule 9: condition で使用される :パラメータ がすべて filters[].param で宣言されているか
        if let Some(condition) = &filter.condition {
            for token in condition.split_whitespace() {
                if let Some(param_name) = token.strip_prefix(':') {
                    let clean =
                        param_name.trim_end_matches(|c: char| !c.is_alphanumeric() && c != '_');
                    if !clean.is_empty() && !declared_params.contains(&clean) {
                        errors.push(ValidationError::Rule(
                            "filters.condition".to_string(),
                            format!(
                                "condition で使用されるパラメータ ':{}' が filters[].param で宣言されていません",
                                clean
                            ),
                        ));
                    }
                }
            }
        }

        // Rule 12: allowed_columns がある場合、default_column がリスト内にあるか
        if filter.maps_to == "ORDER_BY"
            && let (Some(allowed), Some(default_col)) =
                (&filter.allowed_columns, &filter.default_column)
            && !allowed.contains(default_col)
        {
            errors.push(ValidationError::Rule(
                "filters.allowed_columns".to_string(),
                format!(
                    "ORDER_BY の default_column '{}' が allowed_columns リスト外です",
                    default_col
                ),
            ));
        }
    }
}

/// Rule 5, 10: transforms の検証
fn validate_transforms(doc: &UsmlDocument, errors: &mut Vec<ValidationError>) {
    let field_names: Vec<&str> = doc
        .usecase
        .response_mapping
        .iter()
        .map(|m| m.field.as_str())
        .collect();

    for transform in &doc.usecase.transforms {
        // Rule 5: target が response_mapping のいずれかの field に対応しているか
        if !field_names.contains(&transform.target.as_str()) {
            errors.push(ValidationError::Rule(
                "transforms.target".to_string(),
                format!(
                    "transform の target '{}' が response_mapping のいずれかの field に対応していません",
                    transform.target
                ),
            ));
        }

        // Rule 10: condition に param が使われている場合は警告（OpenAPI解析未実装のため）
        if let Some(conditions) = &transform.condition {
            for cond in conditions {
                if cond.param.is_some() {
                    errors.push(ValidationError::Warning(
                        "transforms.condition.param".to_string(),
                        format!(
                            "transform '{}' の condition に param が使われていますが、OpenAPI解析が未実装のためパラメータの存在確認はスキップされます",
                            transform.target
                        ),
                    ));
                }
            }
        }
    }
}

/// Rule 1: response_mapping のフィールド名がOpenAPIレスポンスに存在するか
fn validate_openapi_fields(
    mappings: &[ResponseMapping],
    openapi: &OpenapiResponse,
    errors: &mut Vec<ValidationError>,
) {
    for mapping in mappings {
        if !openapi.fields.contains(&mapping.field) {
            errors.push(ValidationError::Rule(
                "response_mapping.field".to_string(),
                format!(
                    "フィールド {} がOpenAPIレスポンスのプロパティに存在しません",
                    mapping.field
                ),
            ));
        }
    }
}

/// Rule 3: source で参照されるテーブル.カラムがDBMLに実際に存在するか
fn validate_dbml_columns(
    mappings: &[ResponseMapping],
    dbml_tables: &[DbmlTable],
    errors: &mut Vec<ValidationError>,
) {
    for mapping in mappings {
        if let Some(source) = &mapping.source
            && let Some((table_name, col_name)) = source.split_once('.')
            && let Some(table) = dbml_tables.iter().find(|t| t.name == table_name)
            && !table.columns.contains(&col_name.to_string())
        {
            errors.push(ValidationError::Rule(
                "response_mapping.source".to_string(),
                format!(
                    "カラム {} がテーブル {} に存在しません",
                    col_name, table_name
                ),
            ));
        }

        // サブフィールドの再帰検証
        if let Some(sub_fields) = &mapping.fields {
            validate_dbml_columns(sub_fields, dbml_tables, errors);
        }
    }
}

/// Rule 10: transform の condition.param がOpenAPIパラメータに存在するか
fn validate_transform_params(
    transforms: &[crate::ast::Transform],
    openapi: &OpenapiResponse,
    errors: &mut Vec<ValidationError>,
) {
    for transform in transforms {
        if let Some(conditions) = &transform.condition {
            for cond in conditions {
                if let Some(param) = &cond.param
                    && !openapi.parameters.contains(param)
                {
                    errors.push(ValidationError::Rule(
                        "transforms.condition.param".to_string(),
                        format!(
                            "transform {} の condition.param {} がOpenAPIパラメータに存在しません",
                            transform.target, param
                        ),
                    ));
                }
            }
        }
    }
}

/// response_mapping から使われるテーブル名を収集する
fn collect_used_tables(mappings: &[ResponseMapping]) -> Vec<String> {
    let mut tables = Vec::new();

    for mapping in mappings {
        if let Some(source) = &mapping.source
            && let Some(table) = source.split('.').next()
            && !tables.contains(&table.to_string())
        {
            tables.push(table.to_string());
        }

        if let Some(join) = &mapping.join
            && !tables.contains(&join.table)
        {
            tables.push(join.table.clone());
        }

        if let Some(chain) = &mapping.join_chain {
            for entry in chain {
                if !tables.contains(&entry.table) {
                    tables.push(entry.table.clone());
                }
            }
        }

        if let Some(sub_fields) = &mapping.fields {
            for table in collect_used_tables(sub_fields) {
                if !tables.contains(&table) {
                    tables.push(table);
                }
            }
        }
    }

    tables
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser;
    use crate::resolver::{DbmlTable, OpenapiResponse};

    #[test]
    fn test_valid_document_no_errors() {
        let yaml = r#"
version: "0.1"
import:
  openapi: ./api.yaml#paths["/users"].get.responses["200"]
  dbml:
    - ./schema.dbml#tables["users"]
    - ./schema.dbml#tables["profiles"]
usecase:
  name: ユーザー一覧取得
  response_mapping:
    - field: id
      source: users.id
    - field: avatar_url
      source: profiles.avatar_url
      join:
        table: profiles
        on: users.id = profiles.user_id
  transforms:
    - target: avatar_url
      type: COALESCE
      sources:
        - profiles.avatar_url
      fallback: "/default.png"
"#;
        let doc = parser::parse(yaml).unwrap();
        let errors = validate(&doc);
        let hard_errors: Vec<_> = errors
            .iter()
            .filter(|e| matches!(e, ValidationError::Rule(..)))
            .collect();
        assert!(
            hard_errors.is_empty(),
            "エラーがありました: {:?}",
            hard_errors
        );
    }

    #[test]
    fn test_missing_import_table() {
        let yaml = r#"
version: "0.1"
import:
  openapi: ./api.yaml#paths["/users"].get.responses["200"]
  dbml:
    - ./schema.dbml#tables["users"]
usecase:
  name: テスト
  response_mapping:
    - field: avatar_url
      source: profiles.avatar_url
      join:
        table: profiles
        on: users.id = profiles.user_id
"#;
        let doc = parser::parse(yaml).unwrap();
        let errors = validate(&doc);
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ValidationError::Rule(rule, _) if rule == "import.dbml"))
        );
    }

    #[test]
    fn test_duplicate_join_without_alias() {
        let yaml = r#"
version: "0.1"
import:
  openapi: ./api.yaml#paths["/posts"].get.responses["200"]
  dbml:
    - ./schema.dbml#tables["posts"]
    - ./schema.dbml#tables["users"]
usecase:
  name: テスト
  response_mapping:
    - field: author_name
      source: users.name
      join:
        table: users
        on: posts.user_id = users.id
    - field: editor_name
      source: users.name
      join:
        table: users
        on: posts.editor_id = users.id
"#;
        let doc = parser::parse(yaml).unwrap();
        let errors = validate(&doc);
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ValidationError::Rule(rule, _) if rule == "join.alias"))
        );
    }

    #[test]
    fn test_transform_target_not_in_mapping() {
        let yaml = r#"
version: "0.1"
import:
  openapi: ./api.yaml#paths["/users"].get.responses["200"]
  dbml:
    - ./schema.dbml#tables["users"]
usecase:
  name: テスト
  response_mapping:
    - field: id
      source: users.id
  transforms:
    - target: nonexistent_field
      type: COALESCE
      sources:
        - users.name
"#;
        let doc = parser::parse(yaml).unwrap();
        let errors = validate(&doc);
        assert!(errors.iter().any(|e| {
            matches!(e, ValidationError::Rule(rule, _) if rule == "transforms.target")
        }));
    }

    // --- 新規テスト: Rule 6 ---
    #[test]
    fn test_rule6_join_on_references_non_imported_table() {
        let yaml = r#"
version: "0.1"
import:
  openapi: ./api.yaml#paths["/posts"].get.responses["200"]
  dbml:
    - ./schema.dbml#tables["posts"]
usecase:
  name: テスト
  response_mapping:
    - field: author_name
      source: users.name
      join:
        table: users
        on: posts.user_id = users.id
"#;
        let doc = parser::parse(yaml).unwrap();
        let errors = validate(&doc);
        // users テーブルが import にないため Rule 6 (join.on) と Rule 2 (import.dbml) が発火
        assert!(errors
            .iter()
            .any(|e| matches!(e, ValidationError::Rule(rule, _) if rule == "join.on" || rule == "import.dbml")));
    }

    // --- 新規テスト: Rule 8 ---
    #[test]
    fn test_rule8_aggregate_without_group_by_warns() {
        let yaml = r#"
version: "0.1"
import:
  openapi: ./api.yaml#paths["/posts"].get.responses["200"]
  dbml:
    - ./schema.dbml#tables["posts"]
    - ./schema.dbml#tables["likes"]
usecase:
  name: テスト
  response_mapping:
    - field: like_count
      source: likes.id
      join:
        table: likes
        on: posts.id = likes.post_id
      aggregate:
        type: COUNT
"#;
        let doc = parser::parse(yaml).unwrap();
        let errors = validate(&doc);
        assert!(errors.iter().any(
            |e| matches!(e, ValidationError::Warning(rule, _) if rule == "aggregate.group_by")
        ));
    }

    // --- 新規テスト: Rule 9 ---
    #[test]
    fn test_rule9_undeclared_param_in_condition() {
        let yaml = r#"
version: "0.1"
import:
  openapi: ./api.yaml#paths["/users"].get.responses["200"]
  dbml:
    - ./schema.dbml#tables["users"]
usecase:
  name: テスト
  response_mapping:
    - field: id
      source: users.id
  filters:
    - param: status
      maps_to: WHERE
      condition: users.status = :status AND users.role = :role
"#;
        let doc = parser::parse(yaml).unwrap();
        let errors = validate(&doc);
        // :role は filters[].param に宣言されていないため Rule 9 が発火
        assert!(
            errors.iter().any(
                |e| matches!(e, ValidationError::Rule(rule, _) if rule == "filters.condition")
            )
        );
    }

    // --- 新規テスト: Rule 11 ---
    #[test]
    fn test_rule11_source_table_mismatch() {
        let yaml = r#"
version: "0.1"
import:
  openapi: ./api.yaml#paths["/posts"].get.responses["200"]
  dbml:
    - ./schema.dbml#tables["posts"]
    - ./schema.dbml#tables["comments"]
usecase:
  name: テスト
  response_mapping:
    - field: comments
      type: array
      source_table: wrong_table
      join:
        table: comments
        on: posts.id = comments.post_id
      fields:
        - field: id
          source: comments.id
"#;
        let doc = parser::parse(yaml).unwrap();
        let errors = validate(&doc);
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ValidationError::Rule(rule, _) if rule == "source_table"))
        );
    }

    // --- 新規テスト: Rule 12 ---
    #[test]
    fn test_rule12_default_column_not_in_allowed() {
        let yaml = r#"
version: "0.1"
import:
  openapi: ./api.yaml#paths["/users"].get.responses["200"]
  dbml:
    - ./schema.dbml#tables["users"]
usecase:
  name: テスト
  response_mapping:
    - field: id
      source: users.id
  filters:
    - param: sort
      maps_to: ORDER_BY
      default_column: users.secret_field
      allowed_columns:
        - users.created_at
        - users.name
"#;
        let doc = parser::parse(yaml).unwrap();
        let errors = validate(&doc);
        assert!(errors.iter().any(
            |e| matches!(e, ValidationError::Rule(rule, _) if rule == "filters.allowed_columns")
        ));
    }

    // --- 新規テスト: Rule 11 with join_chain ---
    #[test]
    fn test_rule11_source_table_with_join_chain() {
        let yaml = r#"
version: "0.1"
import:
  openapi: ./api.yaml#paths["/posts"].get.responses["200"]
  dbml:
    - ./schema.dbml#tables["posts"]
    - ./schema.dbml#tables["post_tags"]
    - ./schema.dbml#tables["tags"]
usecase:
  name: テスト
  response_mapping:
    - field: tags
      type: array
      source_table: tags
      join:
        table: post_tags
        on: posts.id = post_tags.post_id
      join_chain:
        - table: tags
          on: post_tags.tag_id = tags.id
      fields:
        - field: id
          source: tags.id
"#;
        let doc = parser::parse(yaml).unwrap();
        let errors = validate(&doc);
        // source_table: tags と join_chain の最後のテーブル tags が一致するのでエラーなし
        let hard_errors: Vec<_> = errors
            .iter()
            .filter(|e| matches!(e, ValidationError::Rule(..)))
            .collect();
        assert!(
            hard_errors.is_empty(),
            "エラーがありました: {:?}",
            hard_errors
        );
    }

    #[test]
    fn test_validate_openapi_fields_mismatch() {
        // OpenAPI に id, name, email があるが response_mapping に nonexistent を指定
        let openapi = OpenapiResponse {
            fields: vec!["id".to_string(), "name".to_string(), "email".to_string()],
            parameters: vec!["status".to_string()],
        };
        let yaml = r#"
version: "0.1"
import:
  openapi: ./api.yaml#paths["/users"].get.responses["200"]
  dbml:
    - ./schema.dbml#tables["users"]
usecase:
  name: テスト
  response_mapping:
    - field: nonexistent
      source: users.id
"#;
        let doc = parser::parse(yaml).unwrap();
        let mappings = &doc.usecase.response_mapping;
        let mut errors = Vec::new();
        validate_openapi_fields(mappings, &openapi, &mut errors);
        assert!(errors.iter().any(
            |e| matches!(e, ValidationError::Rule(rule, _) if rule == "response_mapping.field")
        ));
    }

    #[test]
    fn test_validate_dbml_columns_missing() {
        let tables = vec![DbmlTable {
            name: "users".to_string(),
            columns: vec!["id".to_string(), "name".to_string(), "email".to_string()],
        }];
        let yaml = r#"
version: "0.1"
import:
  openapi: ./api.yaml#paths["/users"].get.responses["200"]
  dbml:
    - ./schema.dbml#tables["users"]
usecase:
  name: テスト
  response_mapping:
    - field: phone
      source: users.phone
"#;
        let doc = parser::parse(yaml).unwrap();
        let mappings = &doc.usecase.response_mapping;
        let mut errors = Vec::new();
        validate_dbml_columns(mappings, &tables, &mut errors);
        assert!(errors.iter().any(
            |e| matches!(e, ValidationError::Rule(rule, _) if rule == "response_mapping.source")
        ));
    }

    #[test]
    fn test_validate_transform_params_missing() {
        let openapi = OpenapiResponse {
            fields: vec!["id".to_string()],
            parameters: vec!["status".to_string()],
        };
        let yaml = r#"
version: "0.1"
import:
  openapi: ./api.yaml#paths["/users"].get.responses["200"]
  dbml:
    - ./schema.dbml#tables["users"]
usecase:
  name: テスト
  response_mapping:
    - field: id
      source: users.id
  transforms:
    - target: id
      type: CONDITIONAL_SOURCE
      condition:
        - param: undeclared_param
          operator: "="
          value: "active"
      then_source: users.id
      else_source: users.name
"#;
        let doc = parser::parse(yaml).unwrap();
        let mut errors = Vec::new();
        validate_transform_params(&doc.usecase.transforms, &openapi, &mut errors);
        assert!(errors.iter().any(
            |e| matches!(e, ValidationError::Rule(rule, _) if rule == "transforms.condition.param")
        ));
    }
}
