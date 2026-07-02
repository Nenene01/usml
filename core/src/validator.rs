use std::collections::HashMap;
use std::path::Path;

use thiserror::Error;

use crate::ast::{ColumnMapping, Domain, Entity, ResponseMapping, UsmlDocument, ValueObject};
use crate::resolver::{self, DbmlTable, DdmlItem, OpenapiResponse};

/// 解決済みの外部スキーマ情報
///
/// v0.2 では OpenAPI（Interface 層）と DBML（Infrastructure 層）の両端を保持し、
/// ドメイン層を軸とした 3 点照合（§7.2）に用いる。
/// v0.2.1 で ddml（項目定義の正本）を加え、項目 ⇄ DB 列のトレース検証を行う。
#[derive(Default)]
pub struct ResolveContext {
    pub openapi: Option<OpenapiResponse>,
    pub dbml_tables: Vec<DbmlTable>,
    /// import.ddml から解決された項目一覧
    pub ddml_items: Vec<DdmlItem>,
    /// import.ddml が宣言され、少なくとも 1 ファイルの解決に成功したか。
    /// false のときは ddml トレース検証（規則 17-19）を一切行わない（後方互換）。
    pub ddml_present: bool,
}

impl ResolveContext {
    /// テーブル名で DBML テーブルを検索する
    fn table(&self, name: &str) -> Option<&DbmlTable> {
        self.dbml_tables.iter().find(|t| t.name == name)
    }
}

#[derive(Debug, Error, PartialEq)]
pub enum ValidationError {
    #[error("バリデーション[{0}]: {1}")]
    Rule(String, String),
    #[error("警告[{0}]: {1}")]
    Warning(String, String),
}

// ============================================================
// 公開 API
// ============================================================

/// resolver 無しの構造検証（規則 1, 2, 4, 6, 7, 8, 9, 10, 11(:param 宣言), 13, 14 の構文部分）
///
/// 外部ファイル（OpenAPI / DBML）を解決しない範囲で検証できる規則のみを実行する。
/// テーブル存在やカラム存在・OpenAPI フィールド/パラメータ存在は
/// [`validate_with_resolve`] でのみ検証される。
pub fn validate(doc: &UsmlDocument) -> Vec<ValidationError> {
    let mut errors = Vec::new();

    // 規則 1: domain.entities が 1 つ以上、usecase.root が存在するエンティティを指す
    validate_root_entity(doc, &mut errors);

    // 規則 2: response_mapping[].source が <Entity>.<field> 形式でドメインに存在
    // 規則 4: 配列フィールドの source が relations を指し fields が関連先を射影
    validate_response_mapping_domain(doc, &mut errors);

    // 規則 5（構文部分）: persistence の参照テーブルが import.dbml の宣言に含まれるか
    //   ※ カラム存在は resolve 時。ここでは import 宣言済みテーブル名集合に対して検証
    // 規則 6: 同一テーブルを異なる結合条件で複数参照する場合 alias 必須
    // 規則 7: aggregate に group_by が無く主キー推定不能なら Warning
    validate_persistence(doc, &mut errors);

    // 規則 8: relations の through/on が参照するテーブルが import.dbml の宣言に含まれるか
    validate_relations(doc, &mut errors);

    // 規則 9: derived[].field が当該エンティティの fields に存在
    validate_derived(doc, &mut errors);

    // 規則 10: presentation[].target が response_mapping の field のいずれかに対応
    validate_presentation_target(doc, &mut errors);

    // 規則 11（構文部分）: condition 内の :param がすべて filters で宣言済み
    // 規則 13: ORDER_BY の default_column が allowed_columns 外でない
    validate_filters(doc, &mut errors);

    // 規則 14: entity fields の型が組み込み型または value_objects に存在
    validate_field_types(doc, &mut errors);

    errors
}

/// resolver 付きで型整合まで含めた検証
///
/// base_dir: import 参照のファイルパスを解決するための基準ディレクトリ
pub fn validate_with_resolve(doc: &UsmlDocument, base_dir: &str) -> Vec<ValidationError> {
    let mut errors = Vec::new();

    // まず構造検証を実行
    errors.extend(validate(doc));

    // 外部ファイル解決
    let (ctx, resolve_errors) = resolve_imports(doc, base_dir);
    errors.extend(resolve_errors);

    validate_with_context(doc, &ctx, &mut errors);

    errors
}

/// 解決済み [`ResolveContext`] に対する検証規則をまとめて実行する。
///
/// テストから直接呼べるよう、resolve 処理（ファイル I/O）と分離している。
fn validate_with_context(
    doc: &UsmlDocument,
    ctx: &ResolveContext,
    errors: &mut Vec<ValidationError>,
) {
    // 規則 3: response_mapping[].field が OpenAPI レスポンスフィールドに存在
    if let Some(openapi) = &ctx.openapi {
        validate_openapi_fields(&doc.usecase.response_mapping, openapi, errors);
    }

    // 規則 5（カラム部分）: persistence の参照カラムが import.dbml に存在
    if !ctx.dbml_tables.is_empty() {
        validate_persistence_columns(doc, ctx, errors);
        // 規則 8（カラム部分）: relations の through/on が参照するカラムが存在
        validate_relations_columns(doc, ctx, errors);
    }

    // 規則 11（resolve 部分）: filters[].param が OpenAPI パラメータに存在
    // 規則 12: presentation[].when[].param が OpenAPI に存在
    if let Some(openapi) = &ctx.openapi {
        validate_filter_params(doc, openapi, errors);
        validate_presentation_params(doc, openapi, errors);
    }

    // 規則 15: OpenAPI ⇄ Domain 型整合（Warning）
    if let Some(openapi) = &ctx.openapi {
        validate_openapi_domain_types(doc, openapi, errors);
    }

    // 規則 16: Domain ⇄ DB 型整合（Warning）
    if !ctx.dbml_tables.is_empty() {
        validate_domain_db_types(doc, ctx, errors);
    }

    // 規則 17-19: ddml 項目 ⇄ DB 列のトレース検証
    // import.ddml が宣言され解決に成功したときのみ実行（未宣言時は完全後方互換）
    if ctx.ddml_present {
        validate_ddml_trace(doc, ctx, errors);
    }
}

// ============================================================
// import 解決
// ============================================================

/// import 宣言を実際に解決する
fn resolve_imports(doc: &UsmlDocument, base_dir: &str) -> (ResolveContext, Vec<ValidationError>) {
    let mut errors = Vec::new();
    let mut ctx = ResolveContext::default();

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

    // ddml 解決（import.ddml。fragment 無しの素のパスのリスト）
    if let Some(ddml_refs) = &doc.import.ddml {
        for ddml_ref in ddml_refs {
            let full_path = Path::new(base_dir)
                .join(ddml_ref)
                .to_string_lossy()
                .to_string();
            match resolver::ddml::resolve_ddml(&full_path) {
                Ok((items, dict_warnings)) => {
                    // 解決に成功したら（項目 0 件でも）トレース検証を有効化する
                    ctx.ddml_present = true;
                    ctx.ddml_items.extend(items);
                    // 辞書探索での警告（読めない / 解析不能な ddml.dict.yaml）を伝播する
                    for msg in dict_warnings {
                        errors.push(ValidationError::Warning("import.ddml".to_string(), msg));
                    }
                }
                Err(e) => errors.push(ValidationError::Warning(
                    "import.ddml".to_string(),
                    format!("ddml解決に失敗しました: {}", e),
                )),
            }
        }
    }

    (ctx, errors)
}

/// import.dbml の参照文字列から宣言済みテーブル名の集合を抽出する。
///
/// 形式は `<path>#tables["<name>"]`。resolve 無しの構造検証では、
/// 実ファイルを読まずにこの宣言集合に対してテーブル参照を照合する。
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

// ============================================================
// 構造規則
// ============================================================

/// 規則 1: domain.entities が 1 つ以上定義され、usecase.root が存在するエンティティを指す
fn validate_root_entity(doc: &UsmlDocument, errors: &mut Vec<ValidationError>) {
    if doc.domain.entities.is_empty() {
        errors.push(ValidationError::Rule(
            "domain.entities".to_string(),
            "domain.entities が 1 つも定義されていません".to_string(),
        ));
    }

    if !doc.domain.entities.contains_key(&doc.usecase.root) {
        errors.push(ValidationError::Rule(
            "usecase.root".to_string(),
            format!(
                "usecase.root '{}' が domain.entities に存在しません",
                doc.usecase.root
            ),
        ));
    }
}

/// `<Entity>.<field>` を分解する。形式が満たされない場合は None。
fn split_domain_ref(source: &str) -> Option<(&str, &str)> {
    let (entity, field) = source.split_once('.')?;
    if entity.is_empty() || field.is_empty() {
        return None;
    }
    // ネストされたドット（テーブル.カラム.〜 のような多段参照）は弾く
    if field.contains('.') {
        return None;
    }
    Some((entity, field))
}

/// エンティティ名が「ドメインのエンティティ名らしいか」を判定する。
///
/// 仕様 §1.2 の核心ルール: response_mapping.source は DB カラム直接参照を禁止する。
/// DB テーブルは小文字 snake_case で命名される慣習があり、エンティティは PascalCase。
/// ここでは「先頭が大文字でないものは DB テーブル名の疑いが強い」と見なし、
/// domain.entities に無い場合のエラーメッセージで DB 直接参照を指摘する。
fn looks_like_db_table(name: &str) -> bool {
    name.chars().next().is_some_and(|c| c.is_lowercase())
}

/// 規則 2 & 4: response_mapping[].source の検証
///
/// - 規則 2: source が `<Entity>.<field>` 形式で、Entity・field が domain に存在。
///   DB カラム直接参照（小文字テーブル名 / 存在しないエンティティ）は禁止。
/// - 規則 4: type: array の source が relations を指し、fields が関連先エンティティの
///   フィールドを射影していること。
fn validate_response_mapping_domain(doc: &UsmlDocument, errors: &mut Vec<ValidationError>) {
    for mapping in &doc.usecase.response_mapping {
        validate_mapping_source(mapping, &doc.domain, errors);
    }
}

fn validate_mapping_source(
    mapping: &ResponseMapping,
    domain: &Domain,
    errors: &mut Vec<ValidationError>,
) {
    let Some(source) = &mapping.source else {
        // source 無しのフィールドは射影元不明。配列でなければ警告に留める。
        if mapping.r#type.as_deref() != Some("array") {
            errors.push(ValidationError::Warning(
                "response_mapping.source".to_string(),
                format!("フィールド '{}' に source がありません", mapping.field),
            ));
        }
        return;
    };

    // <Entity>.<name> 形式の検証
    let Some((entity_name, member)) = split_domain_ref(source) else {
        errors.push(ValidationError::Rule(
            "response_mapping.source".to_string(),
            format!(
                "source '{}' が <Entity>.<field> 形式ではありません（DBカラム直接参照は禁止）",
                source
            ),
        ));
        return;
    };

    // エンティティ存在確認
    let Some(entity) = domain.entities.get(entity_name) else {
        let hint = if looks_like_db_table(entity_name) {
            "（小文字のテーブル名と思われます。DBカラム直接参照は禁止です）"
        } else {
            ""
        };
        errors.push(ValidationError::Rule(
            "response_mapping.source".to_string(),
            format!(
                "source '{}' のエンティティ '{}' が domain.entities に存在しません{}",
                source, entity_name, hint
            ),
        ));
        return;
    };

    if mapping.r#type.as_deref() == Some("array") {
        // 規則 4: 配列は relations を指す
        let Some(relation) = entity.relations.get(member) else {
            errors.push(ValidationError::Rule(
                "response_mapping.source".to_string(),
                format!(
                    "配列フィールド '{}' の source '{}' が '{}' の relations を指していません",
                    mapping.field, source, entity_name
                ),
            ));
            return;
        };

        // fields が関連先エンティティのフィールドを射影しているか
        let target_entity = domain.entities.get(&relation.target);
        match &mapping.fields {
            Some(sub_fields) => {
                if let Some(target) = target_entity {
                    for sub in sub_fields {
                        validate_array_sub_field(mapping, sub, &relation.target, target, errors);
                    }
                } else {
                    errors.push(ValidationError::Rule(
                        "response_mapping.source".to_string(),
                        format!(
                            "配列フィールド '{}' の関連先エンティティ '{}' が domain.entities に存在しません",
                            mapping.field, relation.target
                        ),
                    ));
                }
            }
            None => {
                errors.push(ValidationError::Rule(
                    "response_mapping.fields".to_string(),
                    format!(
                        "配列フィールド '{}' に fields（関連先の射影）がありません",
                        mapping.field
                    ),
                ));
            }
        }
    } else {
        // 規則 2: 通常フィールドはエンティティの fields を指す
        if !entity.fields.contains_key(member) {
            errors.push(ValidationError::Rule(
                "response_mapping.source".to_string(),
                format!(
                    "source '{}' のフィールド '{}' がエンティティ '{}' に存在しません",
                    source, member, entity_name
                ),
            ));
        }
    }
}

/// 規則 4: 配列要素のサブフィールド source が関連先エンティティのフィールドを指すか
fn validate_array_sub_field(
    parent: &ResponseMapping,
    sub: &ResponseMapping,
    target_entity_name: &str,
    target_entity: &Entity,
    errors: &mut Vec<ValidationError>,
) {
    let Some(source) = &sub.source else {
        errors.push(ValidationError::Warning(
            "response_mapping.source".to_string(),
            format!(
                "配列 '{}' の要素フィールド '{}' に source がありません",
                parent.field, sub.field
            ),
        ));
        return;
    };

    let Some((entity_name, member)) = split_domain_ref(source) else {
        errors.push(ValidationError::Rule(
            "response_mapping.source".to_string(),
            format!(
                "配列 '{}' の要素 source '{}' が <Entity>.<field> 形式ではありません",
                parent.field, source
            ),
        ));
        return;
    };

    // 射影元のエンティティは関連先エンティティであること
    if entity_name != target_entity_name {
        errors.push(ValidationError::Rule(
            "response_mapping.source".to_string(),
            format!(
                "配列 '{}' の要素 source '{}' が関連先エンティティ '{}' を指していません",
                parent.field, source, target_entity_name
            ),
        ));
        return;
    }

    if !target_entity.fields.contains_key(member) {
        errors.push(ValidationError::Rule(
            "response_mapping.source".to_string(),
            format!(
                "配列 '{}' の要素 source '{}' のフィールド '{}' が関連先エンティティ '{}' に存在しません",
                parent.field, source, member, target_entity_name
            ),
        ));
    }
}

/// 結合式 `a.b = c.d` から (テーブルまたはエイリアス, カラム) の参照を抽出する
fn extract_table_refs(on_expr: &str) -> Vec<(String, String)> {
    let mut refs = Vec::new();
    for token in on_expr.split_whitespace() {
        let clean = token.trim_matches(|c: char| !c.is_alphanumeric() && c != '.' && c != '_');
        if let Some((table, col)) = clean.split_once('.')
            && !table.is_empty()
            && !col.is_empty()
            && col.chars().all(|c| c.is_alphanumeric() || c == '_')
            && table.chars().all(|c| c.is_alphanumeric() || c == '_')
        {
            refs.push((table.to_string(), col.to_string()));
        }
    }
    refs
}

/// `source` 文字列（`table.column` または `alias.column`）を分解する
fn split_source_ref(source: &str) -> Option<(&str, &str)> {
    let (table, col) = source.split_once('.')?;
    if table.is_empty() || col.is_empty() || col.contains('.') {
        return None;
    }
    Some((table, col))
}

/// 規則 5（構文部分）, 6, 7: persistence の検証（resolve 不要部分）
///
/// - 規則 5: source / join / join_chain / aggregate が参照するテーブルが import.dbml 宣言に存在
/// - 規則 6: 同一テーブルを異なる結合条件で複数回参照する場合 alias 必須
/// - 規則 7: aggregate に group_by が無く主キー推定不能なら Warning
fn validate_persistence(doc: &UsmlDocument, errors: &mut Vec<ValidationError>) {
    let imported = parse_imported_tables(doc);

    for (entity_name, entity) in &doc.domain.entities {
        let pers = &entity.persistence;

        // root_table は import.dbml に存在すべき
        if !imported.is_empty() && !imported.contains(&pers.root_table) {
            errors.push(ValidationError::Rule(
                "persistence.root_table".to_string(),
                format!(
                    "エンティティ '{}' の root_table '{}' が import.dbml に含まれていません",
                    entity_name, pers.root_table
                ),
            ));
        }

        // 同一テーブルの結合条件を記録し、規則 6 を判定する
        // key: テーブル名, value: (結合条件 on, alias)
        let mut join_map: HashMap<String, (String, Option<String>)> = HashMap::new();
        // 当該エンティティで宣言された alias 名（テーブル参照の検証で除外する）
        let mut aliases: Vec<String> = Vec::new();

        for (field, mapping) in &pers.columns {
            let ColumnMapping::Detailed(detailed) = mapping else {
                // Simple(table.column) はテーブル存在のみ検証する
                if let ColumnMapping::Simple(s) = mapping
                    && let Some((table, _col)) = split_source_ref(s)
                    && !imported.is_empty()
                    && !imported.contains(&table.to_string())
                {
                    errors.push(ValidationError::Rule(
                        "persistence.columns".to_string(),
                        format!(
                            "{}.{} の source テーブル '{}' が import.dbml に含まれていません",
                            entity_name, field, table
                        ),
                    ));
                }
                continue;
            };

            // join の検証（規則 6 含む）
            if let Some(join) = &detailed.join {
                if let Some(alias) = &join.alias {
                    aliases.push(alias.clone());
                }

                // 規則 6: 同テーブルが異なる結合条件で複数参照 → alias 必須
                if let Some((existing_on, existing_alias)) = join_map.get(&join.table) {
                    if *existing_on != join.on && join.alias.is_none() && existing_alias.is_none() {
                        errors.push(ValidationError::Rule(
                            "persistence.join.alias".to_string(),
                            format!(
                                "エンティティ '{}' でテーブル '{}' が異なる結合条件で複数参照されていますが alias がありません",
                                entity_name, join.table
                            ),
                        ));
                    }
                } else {
                    join_map.insert(join.table.clone(), (join.on.clone(), join.alias.clone()));
                }

                // 規則 5: join.table がインポート済みか
                check_table_imported(
                    &join.table,
                    &imported,
                    &aliases,
                    entity_name,
                    "persistence.join.table",
                    errors,
                );
            }

            // join_chain の各テーブル（規則 5）
            if let Some(chain) = &detailed.join_chain {
                for entry in chain {
                    check_table_imported(
                        &entry.table,
                        &imported,
                        &aliases,
                        entity_name,
                        "persistence.join_chain.table",
                        errors,
                    );
                }
            }

            // 規則 7: aggregate に group_by が無く、root_table 主キーも推定できない場合 Warning
            if let Some(agg) = &detailed.aggregate
                && agg.group_by.is_none()
            {
                errors.push(ValidationError::Warning(
                    "persistence.aggregate.group_by".to_string(),
                    format!(
                        "{}.{} の aggregate ({}) に group_by がありません。省略時は root_table '{}' の主キーが推定適用されます",
                        entity_name, field, agg.r#type, pers.root_table
                    ),
                ));
            }
        }
    }
}

/// 参照テーブルが import.dbml の宣言に含まれるか確認する（alias は除外）
fn check_table_imported(
    table: &str,
    imported: &[String],
    aliases: &[String],
    entity_name: &str,
    rule: &str,
    errors: &mut Vec<ValidationError>,
) {
    if imported.is_empty() {
        return;
    }
    if aliases.iter().any(|a| a == table) {
        return;
    }
    if !imported.contains(&table.to_string()) {
        errors.push(ValidationError::Rule(
            rule.to_string(),
            format!(
                "エンティティ '{}' が参照するテーブル '{}' が import.dbml に含まれていません",
                entity_name, table
            ),
        ));
    }
}

/// 規則 8（構文部分）: relations の through/on が参照するテーブルが import.dbml 宣言に存在
fn validate_relations(doc: &UsmlDocument, errors: &mut Vec<ValidationError>) {
    let imported = parse_imported_tables(doc);
    if imported.is_empty() {
        return;
    }

    for (entity_name, entity) in &doc.domain.entities {
        for (rel_name, relation) in &entity.relations {
            // through テーブル
            if let Some(through) = &relation.through
                && !imported.contains(&through.table)
            {
                errors.push(ValidationError::Rule(
                    "relations.through".to_string(),
                    format!(
                        "{}.{} の through テーブル '{}' が import.dbml に含まれていません",
                        entity_name, rel_name, through.table
                    ),
                ));
            }

            // on / through.on で参照されるテーブル
            let mut on_exprs = vec![&relation.on];
            if let Some(through) = &relation.through {
                on_exprs.push(&through.on);
            }
            for expr in on_exprs {
                for (table, _col) in extract_table_refs(expr) {
                    if !imported.contains(&table) {
                        errors.push(ValidationError::Rule(
                            "relations.on".to_string(),
                            format!(
                                "{}.{} の結合式が参照するテーブル '{}' が import.dbml に含まれていません",
                                entity_name, rel_name, table
                            ),
                        ));
                    }
                }
            }
        }
    }
}

/// 規則 9: derived[].field が当該エンティティの fields に存在
fn validate_derived(doc: &UsmlDocument, errors: &mut Vec<ValidationError>) {
    for (entity_name, entity) in &doc.domain.entities {
        for derived in &entity.derived {
            if !entity.fields.contains_key(&derived.field) {
                errors.push(ValidationError::Rule(
                    "derived.field".to_string(),
                    format!(
                        "エンティティ '{}' の derived.field '{}' が fields に存在しません",
                        entity_name, derived.field
                    ),
                ));
            }
        }
    }
}

/// 規則 10: presentation[].target が response_mapping の field のいずれかに対応
fn validate_presentation_target(doc: &UsmlDocument, errors: &mut Vec<ValidationError>) {
    let field_names = collect_response_field_names(&doc.usecase.response_mapping);

    for pres in &doc.usecase.presentation {
        if !field_names.contains(&pres.target) {
            errors.push(ValidationError::Rule(
                "presentation.target".to_string(),
                format!(
                    "presentation.target '{}' が response_mapping のいずれの field にも対応していません",
                    pres.target
                ),
            ));
        }
    }
}

/// response_mapping のフィールド名を（配列要素は除き）トップレベルから収集する
fn collect_response_field_names(mappings: &[ResponseMapping]) -> Vec<String> {
    mappings.iter().map(|m| m.field.clone()).collect()
}

/// 規則 11（構文部分）, 13: filters の検証（resolve 不要部分）
///
/// - 規則 11: condition 内の :param がすべて filters[].param で宣言済み
/// - 規則 13: ORDER_BY の default_column が allowed_columns 外でない
fn validate_filters(doc: &UsmlDocument, errors: &mut Vec<ValidationError>) {
    let declared_params: Vec<&str> = doc
        .usecase
        .filters
        .iter()
        .map(|f| f.param.as_str())
        .collect();

    for filter in &doc.usecase.filters {
        // 規則 11: condition で使われる :param がすべて宣言済みか
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

        // 規則 13: ORDER_BY の default_column が allowed_columns 外でないか
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

/// 規則 14: entity fields の型が組み込み型または value_objects に存在
fn validate_field_types(doc: &UsmlDocument, errors: &mut Vec<ValidationError>) {
    for (entity_name, entity) in &doc.domain.entities {
        for (field, ty) in &entity.fields {
            if !is_builtin_type(ty) && !value_object_exists(&doc.domain.value_objects, ty) {
                errors.push(ValidationError::Rule(
                    "fields.type".to_string(),
                    format!(
                        "エンティティ '{}' のフィールド '{}' の型 '{}' が組み込み型でも value_objects でもありません",
                        entity_name, field, ty
                    ),
                ));
            }
        }
    }
}

// ============================================================
// resolve 時の規則
// ============================================================

/// 規則 3: response_mapping[].field が OpenAPI レスポンスフィールドに存在
fn validate_openapi_fields(
    mappings: &[ResponseMapping],
    openapi: &OpenapiResponse,
    errors: &mut Vec<ValidationError>,
) {
    for mapping in mappings {
        // 配列要素のフィールド名は要素スキーマ側であり、トップレベル照合の対象外とする
        if !openapi.has_field(&mapping.field) {
            errors.push(ValidationError::Rule(
                "response_mapping.field".to_string(),
                format!(
                    "フィールド '{}' が OpenAPI レスポンスのプロパティに存在しません",
                    mapping.field
                ),
            ));
        }
    }
}

/// 規則 5（カラム部分）: persistence の参照カラムが import.dbml に存在
///
/// alias を伴う join はそのエイリアスをテーブル名として join.table へ解決する。
fn validate_persistence_columns(
    doc: &UsmlDocument,
    ctx: &ResolveContext,
    errors: &mut Vec<ValidationError>,
) {
    for (entity_name, entity) in &doc.domain.entities {
        let pers = &entity.persistence;

        for (field, mapping) in &pers.columns {
            // alias -> 実テーブル名 の対応表（このカラム定義のスコープ内）
            let mut alias_map: HashMap<String, String> = HashMap::new();

            let detailed = match mapping {
                ColumnMapping::Simple(s) => {
                    check_source_column(s, &alias_map, ctx, entity_name, field, errors);
                    continue;
                }
                ColumnMapping::Detailed(d) => d,
            };

            // join / join_chain の alias とテーブルを登録し、結合式のカラムを検証
            if let Some(join) = &detailed.join {
                if let Some(alias) = &join.alias {
                    alias_map.insert(alias.clone(), join.table.clone());
                }
                check_on_columns(&join.on, &alias_map, ctx, entity_name, field, errors);
            }
            if let Some(chain) = &detailed.join_chain {
                for entry in chain {
                    check_on_columns(&entry.on, &alias_map, ctx, entity_name, field, errors);
                }
            }

            // source（実カラム）の存在確認
            check_source_column(
                &detailed.source,
                &alias_map,
                ctx,
                entity_name,
                field,
                errors,
            );
        }
    }
}

/// `table.column`（または `alias.column`）が DBML に存在するか確認する
fn check_source_column(
    source: &str,
    alias_map: &HashMap<String, String>,
    ctx: &ResolveContext,
    entity_name: &str,
    field: &str,
    errors: &mut Vec<ValidationError>,
) {
    let Some((table_ref, col)) = split_source_ref(source) else {
        return;
    };
    let real_table = alias_map
        .get(table_ref)
        .map(|s| s.as_str())
        .unwrap_or(table_ref);

    // テーブルが解決対象に含まれない場合は規則 5 構造側で別途報告済みのためスキップ
    if let Some(table) = ctx.table(real_table)
        && !table.has_column(col)
    {
        errors.push(ValidationError::Rule(
            "persistence.columns".to_string(),
            format!(
                "{}.{} の source カラム '{}' がテーブル '{}' に存在しません",
                entity_name, field, col, real_table
            ),
        ));
    }
}

/// 結合式中の各 `table.column` 参照が DBML に存在するか確認する
fn check_on_columns(
    on_expr: &str,
    alias_map: &HashMap<String, String>,
    ctx: &ResolveContext,
    entity_name: &str,
    field: &str,
    errors: &mut Vec<ValidationError>,
) {
    for (table_ref, col) in extract_table_refs(on_expr) {
        let real_table = alias_map.get(&table_ref).cloned().unwrap_or(table_ref);
        if let Some(table) = ctx.table(&real_table)
            && !table.has_column(&col)
        {
            errors.push(ValidationError::Rule(
                "persistence.join.on".to_string(),
                format!(
                    "{}.{} の結合式が参照するカラム '{}' がテーブル '{}' に存在しません",
                    entity_name, field, col, real_table
                ),
            ));
        }
    }
}

/// 規則 8（カラム部分）: relations の on / through が参照するカラムが DBML に存在
fn validate_relations_columns(
    doc: &UsmlDocument,
    ctx: &ResolveContext,
    errors: &mut Vec<ValidationError>,
) {
    for (entity_name, entity) in &doc.domain.entities {
        for (rel_name, relation) in &entity.relations {
            let mut exprs = vec![relation.on.clone()];
            if let Some(through) = &relation.through {
                exprs.push(through.on.clone());
            }
            for expr in &exprs {
                for (table, col) in extract_table_refs(expr) {
                    if let Some(t) = ctx.table(&table)
                        && !t.has_column(&col)
                    {
                        errors.push(ValidationError::Rule(
                            "relations.on".to_string(),
                            format!(
                                "{}.{} の結合式が参照するカラム '{}' がテーブル '{}' に存在しません",
                                entity_name, rel_name, col, table
                            ),
                        ));
                    }
                }
            }
        }
    }
}

/// 規則 11（resolve 部分）: filters[].param が OpenAPI パラメータに存在
fn validate_filter_params(
    doc: &UsmlDocument,
    openapi: &OpenapiResponse,
    errors: &mut Vec<ValidationError>,
) {
    for filter in &doc.usecase.filters {
        if !openapi.parameters.contains(&filter.param) {
            errors.push(ValidationError::Rule(
                "filters.param".to_string(),
                format!(
                    "filters.param '{}' が OpenAPI のパラメータに存在しません",
                    filter.param
                ),
            ));
        }
    }
}

/// 規則 12: presentation[].when[].param が OpenAPI に存在
fn validate_presentation_params(
    doc: &UsmlDocument,
    openapi: &OpenapiResponse,
    errors: &mut Vec<ValidationError>,
) {
    for pres in &doc.usecase.presentation {
        let Some(conditions) = &pres.condition else {
            continue;
        };
        for cond in conditions {
            if let Some(param) = &cond.param
                && !openapi.parameters.contains(param)
            {
                errors.push(ValidationError::Rule(
                    "presentation.when.param".to_string(),
                    format!(
                        "presentation '{}' の when.param '{}' が OpenAPI のパラメータに存在しません",
                        pres.target, param
                    ),
                ));
            }
        }
    }
}

// ============================================================
// 型整合規則（Warning）
// ============================================================

/// 組み込み型か
fn is_builtin_type(ty: &str) -> bool {
    matches!(ty, "string" | "integer" | "number" | "boolean" | "datetime")
}

/// value_objects に当該名の VO が存在するか
fn value_object_exists(vos: &[ValueObject], name: &str) -> bool {
    vos.iter().any(|v| v.name == name)
}

/// VO 名から VO を引く
fn find_value_object<'a>(vos: &'a [ValueObject], name: &str) -> Option<&'a ValueObject> {
    vos.iter().find(|v| v.name == name)
}

/// ドメインフィールド型名から「基底型」を解決する。
///
/// 組み込み型ならそのまま、VO 名なら VO の base を返す。未知なら None。
fn resolve_base_type(domain: &Domain, type_name: &str) -> Option<String> {
    if is_builtin_type(type_name) {
        return Some(type_name.to_string());
    }
    find_value_object(&domain.value_objects, type_name).map(|vo| vo.base.clone())
}

/// ドメインフィールド型名から VO の format を解決する（組み込み型は format なし）
fn resolve_format(domain: &Domain, type_name: &str) -> Option<String> {
    find_value_object(&domain.value_objects, type_name).and_then(|vo| vo.format.clone())
}

/// OpenAPI の JSON Schema type とドメイン基底型が整合するか（緩く判定）
fn openapi_type_matches(domain_base: &str, openapi_type: &str) -> bool {
    let normalized = match domain_base {
        "datetime" => return matches!(openapi_type, "string"),
        other => other,
    };
    match normalized {
        "string" => matches!(openapi_type, "string"),
        "integer" => matches!(openapi_type, "integer"),
        "number" => matches!(openapi_type, "number" | "integer"),
        "boolean" => matches!(openapi_type, "boolean"),
        _ => true,
    }
}

/// DB カラム型（col_type）とドメイン基底型が整合するか（緩く判定）。
///
/// DBML の型表記は `varchar(255)` のように修飾子を伴うため、先頭の英字部分のみ見る。
fn db_type_matches(domain_base: &str, col_type: &str) -> bool {
    // varchar(255) -> varchar、numeric(10,2) -> numeric
    let base = col_type
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .next()
        .unwrap_or(col_type)
        .to_lowercase();

    match domain_base {
        "integer" => matches!(
            base.as_str(),
            "int" | "integer" | "int4" | "int8" | "bigint" | "smallint" | "serial" | "bigserial"
        ),
        "number" => matches!(
            base.as_str(),
            "numeric" | "decimal" | "float" | "double" | "real" | "money" | "int" | "integer"
        ),
        "string" => matches!(
            base.as_str(),
            "varchar" | "char" | "text" | "string" | "citext" | "uuid"
        ),
        "boolean" => matches!(base.as_str(), "bool" | "boolean"),
        "datetime" => matches!(
            base.as_str(),
            "timestamp" | "timestamptz" | "datetime" | "date" | "time"
        ),
        _ => true,
    }
}

/// OpenAPI の format とドメイン VO の format が整合するか。
///
/// 双方に format がある場合のみ比較し、`date-time` ⇄ `datetime` のような表記揺れを吸収する。
fn format_matches(domain_format: &str, openapi_format: &str) -> bool {
    let normalize = |s: &str| s.replace('-', "").to_lowercase();
    normalize(domain_format) == normalize(openapi_format)
}

/// 規則 15: OpenAPI ⇄ Domain 型整合（Warning）
///
/// response_mapping の各（非配列）フィールドについて、OpenAPI フィールドの type/format と
/// ドメインフィールドの VO の base/format を緩く照合する。
fn validate_openapi_domain_types(
    doc: &UsmlDocument,
    openapi: &OpenapiResponse,
    errors: &mut Vec<ValidationError>,
) {
    for mapping in &doc.usecase.response_mapping {
        if mapping.r#type.as_deref() == Some("array") {
            continue;
        }
        let Some(source) = &mapping.source else {
            continue;
        };
        let Some((entity_name, member)) = split_domain_ref(source) else {
            continue;
        };
        let Some(entity) = doc.domain.entities.get(entity_name) else {
            continue;
        };
        let Some(field_type) = entity.fields.get(member) else {
            continue;
        };
        let Some(of) = openapi.field(&mapping.field) else {
            continue;
        };

        // type 整合
        if let (Some(domain_base), Some(api_type)) =
            (resolve_base_type(&doc.domain, field_type), &of.type_)
            && !openapi_type_matches(&domain_base, api_type)
        {
            errors.push(ValidationError::Warning(
                "type.openapi_domain".to_string(),
                format!(
                    "フィールド '{}': OpenAPI 型 '{}' とドメイン型 '{}' (base: {}) が整合しません",
                    mapping.field, api_type, field_type, domain_base
                ),
            ));
        }

        // format 整合（双方にある場合のみ）
        if let (Some(domain_format), Some(api_format)) =
            (resolve_format(&doc.domain, field_type), &of.format)
            && !format_matches(&domain_format, api_format)
        {
            errors.push(ValidationError::Warning(
                "format.openapi_domain".to_string(),
                format!(
                    "フィールド '{}': OpenAPI format '{}' とドメイン format '{}' が整合しません",
                    mapping.field, api_format, domain_format
                ),
            ));
        }
    }
}

/// 規則 16: Domain ⇄ DB 型整合（Warning）
///
/// persistence.columns の各フィールドについて、対応する DB カラムの col_type と
/// ドメインフィールドの VO の base を緩く照合する。
/// 集約フィールド（aggregate）は型が変質するため照合対象外とする。
fn validate_domain_db_types(
    doc: &UsmlDocument,
    ctx: &ResolveContext,
    errors: &mut Vec<ValidationError>,
) {
    for (entity_name, entity) in &doc.domain.entities {
        for (field, mapping) in &entity.persistence.columns {
            let Some(field_type) = entity.fields.get(field) else {
                continue;
            };
            let Some(domain_base) = resolve_base_type(&doc.domain, field_type) else {
                continue;
            };

            // source・alias を解決して照合対象の (table, col) を得る
            let (source, alias_map) = match mapping {
                ColumnMapping::Simple(s) => (s.as_str(), HashMap::new()),
                ColumnMapping::Detailed(d) => {
                    // 集約は型が変わるため対象外
                    if d.aggregate.is_some() {
                        continue;
                    }
                    let mut map: HashMap<String, String> = HashMap::new();
                    if let Some(join) = &d.join
                        && let Some(alias) = &join.alias
                    {
                        map.insert(alias.clone(), join.table.clone());
                    }
                    (d.source.as_str(), map)
                }
            };

            let Some((table_ref, col)) = split_source_ref(source) else {
                continue;
            };
            let real_table = alias_map
                .get(table_ref)
                .map(|s| s.as_str())
                .unwrap_or(table_ref);

            if let Some(table) = ctx.table(real_table)
                && let Some(column) = table.column(col)
                && !db_type_matches(&domain_base, &column.col_type)
            {
                errors.push(ValidationError::Warning(
                    "type.domain_db".to_string(),
                    format!(
                        "{}.{}: DBカラム '{}.{}' の型 '{}' とドメイン型 '{}' (base: {}) が整合しません",
                        entity_name, field, real_table, col, column.col_type, field_type, domain_base
                    ),
                ));
            }
        }
    }
}

// ============================================================
// ddml トレース規則（規則 17-19）
// ============================================================

/// persistence が参照する実データ列 `(table, column)` を収集する。
///
/// 各 `ColumnMapping` の Simple 値 / Detailed.source を、join.alias を実テーブルへ
/// 解決して返す。これは規則 5（[`validate_persistence_columns`]）が source として
/// 照合する列集合と一致し、ddml トレース検証（規則 17-19）の対象列集合となる。
fn collect_persistence_columns(doc: &UsmlDocument) -> Vec<(String, String)> {
    let mut cols: Vec<(String, String)> = Vec::new();
    for entity in doc.domain.entities.values() {
        for mapping in entity.persistence.columns.values() {
            let (source, alias_map) = match mapping {
                ColumnMapping::Simple(s) => (s.as_str(), HashMap::new()),
                ColumnMapping::Detailed(d) => {
                    let mut map: HashMap<String, String> = HashMap::new();
                    if let Some(join) = &d.join
                        && let Some(alias) = &join.alias
                    {
                        map.insert(alias.clone(), join.table.clone());
                    }
                    (d.source.as_str(), map)
                }
            };
            if let Some((table_ref, col)) = split_source_ref(source) {
                let real_table = alias_map
                    .get(table_ref)
                    .map(|s| s.as_str())
                    .unwrap_or(table_ref);
                let pair = (real_table.to_string(), col.to_string());
                if !cols.contains(&pair) {
                    cols.push(pair);
                }
            }
        }
    }
    cols
}

/// 規則 17-19: ddml 項目 ⇄ DB 列のトレース検証
///
/// - 規則 17 `ddml.trace.status`（Rule）: 参照列に対応する ddml 項目が存在するが
///   storage が confirmed でない → 未確定の設計項目を実装マッピングに使用
/// - 規則 18 `ddml.trace.column`（Warning）: 参照列がどの ddml 項目の schema にも無い
///   → 設計定義に無い列を使用
/// - 規則 19 `ddml.coverage`（Warning）: confirmed かつ schema 付きの ddml 項目が
///   persistence でどこからも参照されていない → 実装マッピング漏れの可能性
fn validate_ddml_trace(
    doc: &UsmlDocument,
    ctx: &ResolveContext,
    errors: &mut Vec<ValidationError>,
) {
    let referenced = collect_persistence_columns(doc);

    // 規則 17 & 18: 参照列ごとに ddml 項目を照合
    for (table, col) in &referenced {
        let matched: Vec<&DdmlItem> = ctx
            .ddml_items
            .iter()
            .filter(|it| {
                it.schema
                    .as_ref()
                    .is_some_and(|(t, c)| t == table && c == col)
            })
            .collect();

        if matched.is_empty() {
            errors.push(ValidationError::Warning(
                "ddml.trace.column".to_string(),
                format!(
                    "persistence が参照する列 '{}.{}' が ddml のどの項目の schema にも定義されていません（設計定義に無い列を使用）",
                    table, col
                ),
            ));
        } else {
            for it in matched {
                if it.storage_status != "confirmed" {
                    errors.push(ValidationError::Rule(
                        "ddml.trace.status".to_string(),
                        format!(
                            "列 '{}.{}' に対応する ddml 項目 '{} {}' の storage が未確定（{}）です（未確定の設計項目を実装マッピングに使用）",
                            table, col, it.item_id, it.item_name, it.storage_status
                        ),
                    ));
                }
            }
        }
    }

    // 規則 19: confirmed かつ schema 付きの項目のカバレッジ
    for it in &ctx.ddml_items {
        if it.storage_status != "confirmed" {
            continue;
        }
        if let Some((t, c)) = &it.schema
            && !referenced.iter().any(|(rt, rc)| rt == t && rc == c)
        {
            errors.push(ValidationError::Warning(
                "ddml.coverage".to_string(),
                format!(
                    "ddml 項目 '{} {}'（{}.{}）は confirmed かつ schema 付きですが、この usml の persistence でどこからも参照されていません（実装マッピング漏れの可能性）",
                    it.item_id, it.item_name, t, c
                ),
            ));
        }
    }
}

// ============================================================
// テスト
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser;
    use crate::resolver::{DbmlColumn, DbmlTable, DdmlItem, OpenapiField, OpenapiResponse};

    // ---- ヘルパ ----

    fn has_rule(errors: &[ValidationError], rule: &str) -> bool {
        errors
            .iter()
            .any(|e| matches!(e, ValidationError::Rule(r, _) if r == rule))
    }

    fn has_warning(errors: &[ValidationError], rule: &str) -> bool {
        errors
            .iter()
            .any(|e| matches!(e, ValidationError::Warning(r, _) if r == rule))
    }

    fn hard_errors(errors: &[ValidationError]) -> Vec<&ValidationError> {
        errors
            .iter()
            .filter(|e| matches!(e, ValidationError::Rule(..)))
            .collect()
    }

    /// §6.1 の正常系サンプル
    fn valid_users_doc() -> &'static str {
        r#"
version: "0.2"
import:
  openapi: ./api.yaml#paths["/users"].get.responses["200"]
  dbml:
    - ./schema.dbml#tables["users"]
    - ./schema.dbml#tables["profiles"]
domain:
  value_objects:
    - { name: UserId, base: integer }
    - { name: Url, base: string, format: uri }
    - { name: Email, base: string, format: email }
  entities:
    User:
      fields:
        id: UserId
        name: string
        email: Email
        avatarUrl: Url
        displayName: string
        status: string
        createdAt: datetime
      persistence:
        root_table: users
        columns:
          id: users.id
          name: users.name
          email: users.email
          status: users.status
          createdAt: users.created_at
          avatarUrl:
            source: profiles.avatar_url
            join: { table: profiles, on: users.id = profiles.user_id }
          displayName:
            source: profiles.display_name
            join: { table: profiles, on: users.id = profiles.user_id }
      derived:
        - field: displayName
          type: COALESCE
          sources: [profiles.display_name, users.name]
          fallback: "anonymous"
usecase:
  name: ユーザー一覧取得
  root: User
  response_mapping:
    - { field: id, source: User.id }
    - { field: name, source: User.name }
    - { field: email, source: User.email }
    - { field: avatar_url, source: User.avatarUrl }
    - { field: display_name, source: User.displayName }
  filters:
    - { param: status, maps_to: WHERE, condition: "User.status = :status" }
    - { param: page, maps_to: PAGINATION, strategy: offset, page_size: 20 }
  presentation:
    - target: email
      type: MASK
      mask_pattern: "***@***.***"
      condition:
        - { param: viewer_role, operator: "!=", value: "admin" }
"#
    }

    /// §6.2 の正常系サンプル（関連・集約・多対多）
    fn valid_posts_doc() -> &'static str {
        r#"
version: "0.2"
import:
  openapi: ./api.yaml#paths["/posts/{post_id}"].get.responses["200"]
  dbml:
    - ./schema.dbml#tables["posts"]
    - ./schema.dbml#tables["users"]
    - ./schema.dbml#tables["comments"]
    - ./schema.dbml#tables["likes"]
    - ./schema.dbml#tables["tags"]
    - ./schema.dbml#tables["post_tags"]
domain:
  value_objects:
    - { name: PostId, base: integer }
    - { name: CommentId, base: integer }
    - { name: TagId, base: integer }
  entities:
    Post:
      fields:
        id: PostId
        title: string
        body: string
        bodyContent: string
        authorName: string
        likeCount: integer
        status: string
      persistence:
        root_table: posts
        columns:
          id: posts.id
          title: posts.title
          body: posts.body
          status: posts.status
          authorName:
            source: users.name
            join: { table: users, on: posts.user_id = users.id }
          likeCount:
            source: likes.id
            join: { table: likes, on: posts.id = likes.post_id }
            aggregate: { type: COUNT, group_by: posts.id }
      derived:
        - field: bodyContent
          type: CONDITIONAL_SOURCE
          when:
            - { source: posts.status, operator: "==", value: "draft" }
          then_source: posts.preview_text
          else_source: posts.body
      relations:
        comments:
          target: Comment
          kind: has_many
          on: posts.id = comments.post_id
        tags:
          target: Tag
          kind: many_to_many
          through: { table: post_tags, on: "posts.id = post_tags.post_id" }
          on: post_tags.tag_id = tags.id
    Comment:
      fields:
        id: CommentId
        body: string
        authorName: string
        createdAt: datetime
      persistence:
        root_table: comments
        columns:
          id: comments.id
          body: comments.body
          createdAt: comments.created_at
          authorName:
            source: comment_author.name
            join: { table: users, alias: comment_author, on: comments.user_id = users.id }
    Tag:
      fields:
        id: TagId
        name: string
      persistence:
        root_table: tags
        columns:
          id: tags.id
          name: tags.name
usecase:
  name: 投稿詳細取得
  root: Post
  response_mapping:
    - { field: id, source: Post.id }
    - { field: title, source: Post.title }
    - { field: body, source: Post.bodyContent }
    - { field: author_name, source: Post.authorName }
    - { field: like_count, source: Post.likeCount }
    - field: tags
      type: array
      source: Post.tags
      fields:
        - { field: id, source: Tag.id }
        - { field: name, source: Tag.name }
    - field: comments
      type: array
      source: Post.comments
      fields:
        - { field: id, source: Comment.id }
        - { field: body, source: Comment.body }
        - { field: author_name, source: Comment.authorName }
        - { field: created_at, source: Comment.createdAt }
  filters:
    - { param: post_id, maps_to: WHERE, condition: "Post.id = :post_id" }
"#
    }

    // ---- 正常系 ----

    #[test]
    fn test_valid_users_doc_no_rule_errors() {
        let doc = parser::parse(valid_users_doc()).unwrap();
        let errors = validate(&doc);
        assert!(
            hard_errors(&errors).is_empty(),
            "エラーがありました: {:?}",
            hard_errors(&errors)
        );
    }

    #[test]
    fn test_valid_posts_doc_no_rule_errors() {
        let doc = parser::parse(valid_posts_doc()).unwrap();
        let errors = validate(&doc);
        assert!(
            hard_errors(&errors).is_empty(),
            "エラーがありました: {:?}",
            hard_errors(&errors)
        );
    }

    // ---- 規則 1 ----

    #[test]
    fn test_rule1_root_not_in_entities() {
        let yaml = r#"
version: "0.2"
import:
  dbml: ['./schema.dbml#tables["users"]']
domain:
  entities:
    User:
      fields: { id: integer }
      persistence:
        root_table: users
        columns: { id: users.id }
usecase:
  name: テスト
  root: NonExistent
  response_mapping:
    - { field: id, source: User.id }
"#;
        let doc = parser::parse(yaml).unwrap();
        let errors = validate(&doc);
        assert!(has_rule(&errors, "usecase.root"));
    }

    // ---- 規則 2 ----

    #[test]
    fn test_rule2_db_column_direct_reference_rejected() {
        // source が小文字テーブル名（DBカラム直接参照）
        let yaml = r#"
version: "0.2"
import:
  dbml: ['./schema.dbml#tables["users"]']
domain:
  entities:
    User:
      fields: { id: integer }
      persistence:
        root_table: users
        columns: { id: users.id }
usecase:
  name: テスト
  root: User
  response_mapping:
    - { field: id, source: users.id }
"#;
        let doc = parser::parse(yaml).unwrap();
        let errors = validate(&doc);
        assert!(has_rule(&errors, "response_mapping.source"));
    }

    #[test]
    fn test_rule2_field_not_in_entity() {
        let yaml = r#"
version: "0.2"
import:
  dbml: ['./schema.dbml#tables["users"]']
domain:
  entities:
    User:
      fields: { id: integer }
      persistence:
        root_table: users
        columns: { id: users.id }
usecase:
  name: テスト
  root: User
  response_mapping:
    - { field: nope, source: User.nonexistent }
"#;
        let doc = parser::parse(yaml).unwrap();
        let errors = validate(&doc);
        assert!(has_rule(&errors, "response_mapping.source"));
    }

    // ---- 規則 4 ----

    #[test]
    fn test_rule4_array_source_not_relation() {
        // 配列 source が relations でなく fields を指している
        let yaml = r#"
version: "0.2"
import:
  dbml: ['./schema.dbml#tables["posts"]']
domain:
  entities:
    Post:
      fields: { id: integer, title: string }
      persistence:
        root_table: posts
        columns: { id: posts.id, title: posts.title }
usecase:
  name: テスト
  root: Post
  response_mapping:
    - field: items
      type: array
      source: Post.title
      fields:
        - { field: id, source: Post.id }
"#;
        let doc = parser::parse(yaml).unwrap();
        let errors = validate(&doc);
        assert!(has_rule(&errors, "response_mapping.source"));
    }

    #[test]
    fn test_rule4_array_sub_field_wrong_entity() {
        let yaml = r#"
version: "0.2"
import:
  dbml: ['./schema.dbml#tables["posts"]', './schema.dbml#tables["comments"]']
domain:
  entities:
    Post:
      fields: { id: integer }
      persistence:
        root_table: posts
        columns: { id: posts.id }
      relations:
        comments:
          target: Comment
          kind: has_many
          on: posts.id = comments.post_id
    Comment:
      fields: { id: integer, body: string }
      persistence:
        root_table: comments
        columns: { id: comments.id, body: comments.body }
usecase:
  name: テスト
  root: Post
  response_mapping:
    - field: comments
      type: array
      source: Post.comments
      fields:
        - { field: id, source: Post.id }
"#;
        let doc = parser::parse(yaml).unwrap();
        let errors = validate(&doc);
        // 要素 source が Comment でなく Post を指している
        assert!(has_rule(&errors, "response_mapping.source"));
    }

    // ---- 規則 5（構文） ----

    #[test]
    fn test_rule5_join_table_not_imported() {
        let yaml = r#"
version: "0.2"
import:
  dbml: ['./schema.dbml#tables["posts"]']
domain:
  entities:
    Post:
      fields: { id: integer, authorName: string }
      persistence:
        root_table: posts
        columns:
          id: posts.id
          authorName:
            source: users.name
            join: { table: users, on: posts.user_id = users.id }
usecase:
  name: テスト
  root: Post
  response_mapping:
    - { field: id, source: Post.id }
    - { field: author_name, source: Post.authorName }
"#;
        let doc = parser::parse(yaml).unwrap();
        let errors = validate(&doc);
        // users が import.dbml に無い
        assert!(has_rule(&errors, "persistence.join.table"));
    }

    // ---- 規則 6 ----

    #[test]
    fn test_rule6_duplicate_join_without_alias() {
        let yaml = r#"
version: "0.2"
import:
  dbml: ['./schema.dbml#tables["posts"]', './schema.dbml#tables["users"]']
domain:
  entities:
    Post:
      fields: { id: integer, authorName: string, editorName: string }
      persistence:
        root_table: posts
        columns:
          id: posts.id
          authorName:
            source: users.name
            join: { table: users, on: posts.user_id = users.id }
          editorName:
            source: users.name
            join: { table: users, on: posts.editor_id = users.id }
usecase:
  name: テスト
  root: Post
  response_mapping:
    - { field: id, source: Post.id }
"#;
        let doc = parser::parse(yaml).unwrap();
        let errors = validate(&doc);
        assert!(has_rule(&errors, "persistence.join.alias"));
    }

    #[test]
    fn test_rule6_duplicate_join_with_alias_ok() {
        let yaml = r#"
version: "0.2"
import:
  dbml: ['./schema.dbml#tables["posts"]', './schema.dbml#tables["users"]']
domain:
  entities:
    Post:
      fields: { id: integer, authorName: string, editorName: string }
      persistence:
        root_table: posts
        columns:
          id: posts.id
          authorName:
            source: author.name
            join: { table: users, alias: author, on: posts.user_id = users.id }
          editorName:
            source: editor.name
            join: { table: users, alias: editor, on: posts.editor_id = users.id }
usecase:
  name: テスト
  root: Post
  response_mapping:
    - { field: id, source: Post.id }
"#;
        let doc = parser::parse(yaml).unwrap();
        let errors = validate(&doc);
        assert!(!has_rule(&errors, "persistence.join.alias"));
    }

    // ---- 規則 7 ----

    #[test]
    fn test_rule7_aggregate_without_group_by_warns() {
        let yaml = r#"
version: "0.2"
import:
  dbml: ['./schema.dbml#tables["posts"]', './schema.dbml#tables["likes"]']
domain:
  entities:
    Post:
      fields: { id: integer, likeCount: integer }
      persistence:
        root_table: posts
        columns:
          id: posts.id
          likeCount:
            source: likes.id
            join: { table: likes, on: posts.id = likes.post_id }
            aggregate: { type: COUNT }
usecase:
  name: テスト
  root: Post
  response_mapping:
    - { field: id, source: Post.id }
    - { field: like_count, source: Post.likeCount }
"#;
        let doc = parser::parse(yaml).unwrap();
        let errors = validate(&doc);
        assert!(has_warning(&errors, "persistence.aggregate.group_by"));
    }

    // ---- 規則 8 ----

    #[test]
    fn test_rule8_through_table_not_imported() {
        let yaml = r#"
version: "0.2"
import:
  dbml: ['./schema.dbml#tables["posts"]', './schema.dbml#tables["tags"]']
domain:
  entities:
    Post:
      fields: { id: integer }
      persistence:
        root_table: posts
        columns: { id: posts.id }
      relations:
        tags:
          target: Tag
          kind: many_to_many
          through: { table: post_tags, on: posts.id = post_tags.post_id }
          on: post_tags.tag_id = tags.id
    Tag:
      fields: { id: integer }
      persistence:
        root_table: tags
        columns: { id: tags.id }
usecase:
  name: テスト
  root: Post
  response_mapping:
    - { field: id, source: Post.id }
"#;
        let doc = parser::parse(yaml).unwrap();
        let errors = validate(&doc);
        // post_tags が import.dbml に無い
        assert!(has_rule(&errors, "relations.through"));
    }

    // ---- 規則 9 ----

    #[test]
    fn test_rule9_derived_field_not_in_fields() {
        let yaml = r#"
version: "0.2"
import:
  dbml: ['./schema.dbml#tables["users"]']
domain:
  entities:
    User:
      fields: { id: integer }
      persistence:
        root_table: users
        columns: { id: users.id }
      derived:
        - field: ghostField
          type: COALESCE
          sources: [users.a, users.b]
usecase:
  name: テスト
  root: User
  response_mapping:
    - { field: id, source: User.id }
"#;
        let doc = parser::parse(yaml).unwrap();
        let errors = validate(&doc);
        assert!(has_rule(&errors, "derived.field"));
    }

    // ---- 規則 10 ----

    #[test]
    fn test_rule10_presentation_target_not_in_mapping() {
        let yaml = r#"
version: "0.2"
import:
  dbml: ['./schema.dbml#tables["users"]']
domain:
  entities:
    User:
      fields: { id: integer }
      persistence:
        root_table: users
        columns: { id: users.id }
usecase:
  name: テスト
  root: User
  response_mapping:
    - { field: id, source: User.id }
  presentation:
    - target: nonexistent
      type: MASK
      mask_pattern: "***"
"#;
        let doc = parser::parse(yaml).unwrap();
        let errors = validate(&doc);
        assert!(has_rule(&errors, "presentation.target"));
    }

    // ---- 規則 11（構文） ----

    #[test]
    fn test_rule11_undeclared_param_in_condition() {
        let yaml = r#"
version: "0.2"
import:
  dbml: ['./schema.dbml#tables["users"]']
domain:
  entities:
    User:
      fields: { id: integer, status: string }
      persistence:
        root_table: users
        columns: { id: users.id, status: users.status }
usecase:
  name: テスト
  root: User
  response_mapping:
    - { field: id, source: User.id }
  filters:
    - { param: status, maps_to: WHERE, condition: "User.status = :status AND User.role = :role" }
"#;
        let doc = parser::parse(yaml).unwrap();
        let errors = validate(&doc);
        // :role が未宣言
        assert!(has_rule(&errors, "filters.condition"));
    }

    // ---- 規則 13 ----

    #[test]
    fn test_rule13_default_column_not_in_allowed() {
        let yaml = r#"
version: "0.2"
import:
  dbml: ['./schema.dbml#tables["users"]']
domain:
  entities:
    User:
      fields: { id: integer }
      persistence:
        root_table: users
        columns: { id: users.id }
usecase:
  name: テスト
  root: User
  response_mapping:
    - { field: id, source: User.id }
  filters:
    - param: sort
      maps_to: ORDER_BY
      default_column: User.secret
      allowed_columns: [User.createdAt, User.name]
"#;
        let doc = parser::parse(yaml).unwrap();
        let errors = validate(&doc);
        assert!(has_rule(&errors, "filters.allowed_columns"));
    }

    // ---- 規則 14 ----

    #[test]
    fn test_rule14_unknown_field_type() {
        let yaml = r#"
version: "0.2"
import:
  dbml: ['./schema.dbml#tables["users"]']
domain:
  value_objects:
    - { name: UserId, base: integer }
  entities:
    User:
      fields: { id: UserId, weird: MysteryType }
      persistence:
        root_table: users
        columns: { id: users.id, weird: users.weird }
usecase:
  name: テスト
  root: User
  response_mapping:
    - { field: id, source: User.id }
"#;
        let doc = parser::parse(yaml).unwrap();
        let errors = validate(&doc);
        assert!(has_rule(&errors, "fields.type"));
    }

    // ---- 規則 3（resolve）----

    #[test]
    fn test_rule3_openapi_field_missing() {
        let openapi = OpenapiResponse {
            fields: vec![OpenapiField {
                name: "id".to_string(),
                type_: Some("integer".to_string()),
                format: None,
            }],
            parameters: vec![],
        };
        let yaml = r#"
version: "0.2"
import: {}
domain:
  entities:
    User:
      fields: { id: integer, name: string }
      persistence:
        root_table: users
        columns: { id: users.id, name: users.name }
usecase:
  name: テスト
  root: User
  response_mapping:
    - { field: id, source: User.id }
    - { field: missing_field, source: User.name }
"#;
        let doc = parser::parse(yaml).unwrap();
        let mut errors = Vec::new();
        validate_openapi_fields(&doc.usecase.response_mapping, &openapi, &mut errors);
        assert!(has_rule(&errors, "response_mapping.field"));
    }

    // ---- 規則 5（カラム / resolve）----

    #[test]
    fn test_rule5_column_missing_in_dbml() {
        let ctx = ResolveContext {
            openapi: None,
            dbml_tables: vec![DbmlTable {
                name: "users".to_string(),
                columns: vec![DbmlColumn {
                    name: "id".to_string(),
                    col_type: "integer".to_string(),
                }],
            }],
            ..Default::default()
        };
        let yaml = r#"
version: "0.2"
import:
  dbml: ['./schema.dbml#tables["users"]']
domain:
  entities:
    User:
      fields: { id: integer, phone: string }
      persistence:
        root_table: users
        columns: { id: users.id, phone: users.phone }
usecase:
  name: テスト
  root: User
  response_mapping:
    - { field: id, source: User.id }
"#;
        let doc = parser::parse(yaml).unwrap();
        let mut errors = Vec::new();
        validate_persistence_columns(&doc, &ctx, &mut errors);
        // users.phone が DBML に無い
        assert!(has_rule(&errors, "persistence.columns"));
    }

    #[test]
    fn test_rule5_alias_column_resolved() {
        // alias を実テーブルに解決して source カラムを照合できること（正常系）
        let ctx = ResolveContext {
            openapi: None,
            dbml_tables: vec![
                DbmlTable {
                    name: "comments".to_string(),
                    columns: vec![
                        DbmlColumn {
                            name: "id".to_string(),
                            col_type: "integer".to_string(),
                        },
                        DbmlColumn {
                            name: "user_id".to_string(),
                            col_type: "integer".to_string(),
                        },
                    ],
                },
                DbmlTable {
                    name: "users".to_string(),
                    columns: vec![
                        DbmlColumn {
                            name: "id".to_string(),
                            col_type: "integer".to_string(),
                        },
                        DbmlColumn {
                            name: "name".to_string(),
                            col_type: "varchar(255)".to_string(),
                        },
                    ],
                },
            ],
            ..Default::default()
        };
        let yaml = r#"
version: "0.2"
import:
  dbml: ['./schema.dbml#tables["comments"]', './schema.dbml#tables["users"]']
domain:
  entities:
    Comment:
      fields: { id: integer, authorName: string }
      persistence:
        root_table: comments
        columns:
          id: comments.id
          authorName:
            source: comment_author.name
            join: { table: users, alias: comment_author, on: comments.user_id = users.id }
usecase:
  name: テスト
  root: Comment
  response_mapping:
    - { field: id, source: Comment.id }
    - { field: author_name, source: Comment.authorName }
"#;
        let doc = parser::parse(yaml).unwrap();
        let mut errors = Vec::new();
        validate_persistence_columns(&doc, &ctx, &mut errors);
        assert!(
            !has_rule(&errors, "persistence.columns"),
            "alias 経由のカラム解決でエラー: {:?}",
            errors
        );
    }

    // ---- 規則 11（resolve）----

    #[test]
    fn test_rule11_filter_param_not_in_openapi() {
        let openapi = OpenapiResponse {
            fields: vec![],
            parameters: vec!["status".to_string()],
        };
        let yaml = r#"
version: "0.2"
import: {}
domain:
  entities:
    User:
      fields: { id: integer }
      persistence:
        root_table: users
        columns: { id: users.id }
usecase:
  name: テスト
  root: User
  response_mapping:
    - { field: id, source: User.id }
  filters:
    - { param: undeclared, maps_to: WHERE, condition: "User.id = :undeclared" }
"#;
        let doc = parser::parse(yaml).unwrap();
        let mut errors = Vec::new();
        validate_filter_params(&doc, &openapi, &mut errors);
        assert!(has_rule(&errors, "filters.param"));
    }

    // ---- 規則 12（resolve）----

    #[test]
    fn test_rule12_presentation_param_not_in_openapi() {
        let openapi = OpenapiResponse {
            fields: vec![],
            parameters: vec!["status".to_string()],
        };
        let yaml = r#"
version: "0.2"
import: {}
domain:
  entities:
    User:
      fields: { id: integer, email: string }
      persistence:
        root_table: users
        columns: { id: users.id, email: users.email }
usecase:
  name: テスト
  root: User
  response_mapping:
    - { field: id, source: User.id }
    - { field: email, source: User.email }
  presentation:
    - target: email
      type: MASK
      mask_pattern: "***"
      condition:
        - { param: viewer_role, operator: "!=", value: "admin" }
"#;
        let doc = parser::parse(yaml).unwrap();
        let mut errors = Vec::new();
        validate_presentation_params(&doc, &openapi, &mut errors);
        // viewer_role が OpenAPI パラメータに無い
        assert!(has_rule(&errors, "presentation.when.param"));
    }

    // ---- 規則 15（型整合 Warning）----

    #[test]
    fn test_rule15_openapi_domain_type_mismatch() {
        // ドメインは integer、OpenAPI は string
        let openapi = OpenapiResponse {
            fields: vec![OpenapiField {
                name: "id".to_string(),
                type_: Some("string".to_string()),
                format: None,
            }],
            parameters: vec![],
        };
        let yaml = r#"
version: "0.2"
import: {}
domain:
  value_objects:
    - { name: UserId, base: integer }
  entities:
    User:
      fields: { id: UserId }
      persistence:
        root_table: users
        columns: { id: users.id }
usecase:
  name: テスト
  root: User
  response_mapping:
    - { field: id, source: User.id }
"#;
        let doc = parser::parse(yaml).unwrap();
        let mut errors = Vec::new();
        validate_openapi_domain_types(&doc, &openapi, &mut errors);
        assert!(has_warning(&errors, "type.openapi_domain"));
    }

    #[test]
    fn test_rule15_format_mismatch() {
        // ドメイン VO format=uri、OpenAPI format=email
        let openapi = OpenapiResponse {
            fields: vec![OpenapiField {
                name: "avatar_url".to_string(),
                type_: Some("string".to_string()),
                format: Some("email".to_string()),
            }],
            parameters: vec![],
        };
        let yaml = r#"
version: "0.2"
import: {}
domain:
  value_objects:
    - { name: Url, base: string, format: uri }
  entities:
    User:
      fields: { avatarUrl: Url }
      persistence:
        root_table: users
        columns: { avatarUrl: users.avatar_url }
usecase:
  name: テスト
  root: User
  response_mapping:
    - { field: avatar_url, source: User.avatarUrl }
"#;
        let doc = parser::parse(yaml).unwrap();
        let mut errors = Vec::new();
        validate_openapi_domain_types(&doc, &openapi, &mut errors);
        assert!(has_warning(&errors, "format.openapi_domain"));
    }

    #[test]
    fn test_rule15_type_match_no_warning() {
        // datetime ⇄ string は整合扱い
        let openapi = OpenapiResponse {
            fields: vec![OpenapiField {
                name: "created_at".to_string(),
                type_: Some("string".to_string()),
                format: Some("date-time".to_string()),
            }],
            parameters: vec![],
        };
        let yaml = r#"
version: "0.2"
import: {}
domain:
  value_objects:
    - { name: Timestamp, base: datetime, format: datetime }
  entities:
    User:
      fields: { createdAt: Timestamp }
      persistence:
        root_table: users
        columns: { createdAt: users.created_at }
usecase:
  name: テスト
  root: User
  response_mapping:
    - { field: created_at, source: User.createdAt }
"#;
        let doc = parser::parse(yaml).unwrap();
        let mut errors = Vec::new();
        validate_openapi_domain_types(&doc, &openapi, &mut errors);
        assert!(!has_warning(&errors, "type.openapi_domain"));
        assert!(!has_warning(&errors, "format.openapi_domain"));
    }

    // ---- 規則 16（型整合 Warning）----

    #[test]
    fn test_rule16_domain_db_type_mismatch() {
        // ドメインは integer、DB カラムは varchar
        let ctx = ResolveContext {
            openapi: None,
            dbml_tables: vec![DbmlTable {
                name: "users".to_string(),
                columns: vec![DbmlColumn {
                    name: "id".to_string(),
                    col_type: "varchar(255)".to_string(),
                }],
            }],
            ..Default::default()
        };
        let yaml = r#"
version: "0.2"
import:
  dbml: ['./schema.dbml#tables["users"]']
domain:
  value_objects:
    - { name: UserId, base: integer }
  entities:
    User:
      fields: { id: UserId }
      persistence:
        root_table: users
        columns: { id: users.id }
usecase:
  name: テスト
  root: User
  response_mapping:
    - { field: id, source: User.id }
"#;
        let doc = parser::parse(yaml).unwrap();
        let mut errors = Vec::new();
        validate_domain_db_types(&doc, &ctx, &mut errors);
        assert!(has_warning(&errors, "type.domain_db"));
    }

    #[test]
    fn test_rule16_domain_db_type_match_no_warning() {
        // integer ⇄ serial、string ⇄ varchar は整合
        let ctx = ResolveContext {
            openapi: None,
            dbml_tables: vec![DbmlTable {
                name: "users".to_string(),
                columns: vec![
                    DbmlColumn {
                        name: "id".to_string(),
                        col_type: "serial".to_string(),
                    },
                    DbmlColumn {
                        name: "name".to_string(),
                        col_type: "varchar(255)".to_string(),
                    },
                ],
            }],
            ..Default::default()
        };
        let yaml = r#"
version: "0.2"
import:
  dbml: ['./schema.dbml#tables["users"]']
domain:
  value_objects:
    - { name: UserId, base: integer }
  entities:
    User:
      fields: { id: UserId, name: string }
      persistence:
        root_table: users
        columns: { id: users.id, name: users.name }
usecase:
  name: テスト
  root: User
  response_mapping:
    - { field: id, source: User.id }
    - { field: name, source: User.name }
"#;
        let doc = parser::parse(yaml).unwrap();
        let mut errors = Vec::new();
        validate_domain_db_types(&doc, &ctx, &mut errors);
        assert!(!has_warning(&errors, "type.domain_db"));
    }

    // ---- ヘルパ単体 ----

    #[test]
    fn test_db_type_matches_loose() {
        assert!(db_type_matches("integer", "serial"));
        assert!(db_type_matches("integer", "int4"));
        assert!(db_type_matches("string", "varchar(255)"));
        assert!(db_type_matches("string", "text"));
        assert!(db_type_matches("datetime", "timestamp"));
        assert!(db_type_matches("datetime", "date"));
        assert!(!db_type_matches("integer", "varchar(255)"));
        assert!(!db_type_matches("boolean", "integer"));
    }

    #[test]
    fn test_openapi_type_matches_loose() {
        assert!(openapi_type_matches("datetime", "string"));
        assert!(openapi_type_matches("number", "integer"));
        assert!(openapi_type_matches("integer", "integer"));
        assert!(!openapi_type_matches("integer", "string"));
    }

    // ---- 規則 17-19（ddml トレース）----

    /// 受注 3 列（orders.id / orders.order_no / orders.note）を参照する最小 usml
    fn ddml_trace_doc() -> &'static str {
        r#"
version: "0.2"
import:
  ddml:
    - ./order.ddml.yaml
domain:
  entities:
    Order:
      fields: { id: integer, orderNo: string, note: string }
      persistence:
        root_table: orders
        columns:
          id: orders.id
          orderNo: orders.order_no
          note: orders.note
usecase:
  name: 受注一覧
  root: Order
  response_mapping:
    - { field: id, source: Order.id }
    - { field: order_no, source: Order.orderNo }
    - { field: note, source: Order.note }
"#
    }

    fn ddml_item(id: &str, name: &str, status: &str, schema: Option<(&str, &str)>) -> DdmlItem {
        DdmlItem {
            item_id: id.to_string(),
            item_name: name.to_string(),
            storage_status: status.to_string(),
            schema: schema.map(|(t, c)| (t.to_string(), c.to_string())),
        }
    }

    fn ddml_ctx(items: Vec<DdmlItem>) -> ResolveContext {
        ResolveContext {
            ddml_present: true,
            ddml_items: items,
            ..Default::default()
        }
    }

    #[test]
    fn test_rule17_trace_status_unconfirmed() {
        // note は hypothesis → ddml.trace.status（エラー）
        // id は ddml に無い → ddml.trace.column（警告）
        let ctx = ddml_ctx(vec![
            ddml_item(
                "ITM-001",
                "受注番号",
                "confirmed",
                Some(("orders", "order_no")),
            ),
            ddml_item("ITM-002", "備考", "hypothesis", Some(("orders", "note"))),
        ]);
        let doc = parser::parse(ddml_trace_doc()).unwrap();
        let mut errors = Vec::new();
        validate_ddml_trace(&doc, &ctx, &mut errors);
        assert!(has_rule(&errors, "ddml.trace.status"));
        assert!(has_warning(&errors, "ddml.trace.column"));
    }

    #[test]
    fn test_rule17_all_confirmed_no_error() {
        // 全列が confirmed かつ schema で網羅 → 3 ルールいずれも発動しない
        let ctx = ddml_ctx(vec![
            ddml_item("ITM-000", "受注ID", "confirmed", Some(("orders", "id"))),
            ddml_item(
                "ITM-001",
                "受注番号",
                "confirmed",
                Some(("orders", "order_no")),
            ),
            ddml_item("ITM-002", "備考", "confirmed", Some(("orders", "note"))),
        ]);
        let doc = parser::parse(ddml_trace_doc()).unwrap();
        let mut errors = Vec::new();
        validate_ddml_trace(&doc, &ctx, &mut errors);
        assert!(!has_rule(&errors, "ddml.trace.status"));
        assert!(!has_warning(&errors, "ddml.trace.column"));
        assert!(!has_warning(&errors, "ddml.coverage"));
    }

    #[test]
    fn test_rule18_trace_column_not_in_ddml() {
        // id / note に対応する ddml 項目が無い（order_no のみ定義）→ trace.column 警告
        let ctx = ddml_ctx(vec![ddml_item(
            "ITM-001",
            "受注番号",
            "confirmed",
            Some(("orders", "order_no")),
        )]);
        let doc = parser::parse(ddml_trace_doc()).unwrap();
        let mut errors = Vec::new();
        validate_ddml_trace(&doc, &ctx, &mut errors);
        assert!(has_warning(&errors, "ddml.trace.column"));
        // order_no は定義済みなので status エラーは無い
        assert!(!has_rule(&errors, "ddml.trace.status"));
    }

    #[test]
    fn test_rule19_coverage_unreferenced() {
        // extra_col は confirmed + schema だが usml から未参照 → coverage 警告。
        // memo は confirmed だが schema 無し → coverage 対象外。
        let ctx = ddml_ctx(vec![
            ddml_item("ITM-000", "受注ID", "confirmed", Some(("orders", "id"))),
            ddml_item(
                "ITM-001",
                "受注番号",
                "confirmed",
                Some(("orders", "order_no")),
            ),
            ddml_item("ITM-002", "備考", "confirmed", Some(("orders", "note"))),
            ddml_item(
                "ITM-003",
                "追加列",
                "confirmed",
                Some(("orders", "extra_col")),
            ),
            ddml_item("ITM-004", "メモ", "confirmed", None),
        ]);
        let doc = parser::parse(ddml_trace_doc()).unwrap();
        let mut errors = Vec::new();
        validate_ddml_trace(&doc, &ctx, &mut errors);
        assert!(has_warning(&errors, "ddml.coverage"));
        // 参照列はすべて confirmed で網羅 → status/column は発動しない
        assert!(!has_rule(&errors, "ddml.trace.status"));
        assert!(!has_warning(&errors, "ddml.trace.column"));
        // coverage 警告は extra_col の 1 件のみ（memo は schema 無しで対象外）
        let coverage_count = errors
            .iter()
            .filter(|e| matches!(e, ValidationError::Warning(r, _) if r == "ddml.coverage"))
            .count();
        assert_eq!(coverage_count, 1);
    }

    #[test]
    fn test_ddml_backward_compat_no_import() {
        // ddml_present=false（import.ddml 無し）のときは 3 ルールとも一切発動しない
        let doc = parser::parse(ddml_trace_doc()).unwrap();
        let ctx = ResolveContext::default();
        let mut errors = Vec::new();
        validate_with_context(&doc, &ctx, &mut errors);
        assert!(!has_rule(&errors, "ddml.trace.status"));
        assert!(!has_warning(&errors, "ddml.trace.column"));
        assert!(!has_warning(&errors, "ddml.coverage"));
    }

    #[test]
    fn test_collect_persistence_columns_resolves_alias() {
        // join.alias 経由の source が実テーブルに解決されて列集合へ入ること
        let yaml = r#"
version: "0.2"
import:
  ddml: ['./x.ddml.yaml']
domain:
  entities:
    Comment:
      fields: { id: integer, authorName: string }
      persistence:
        root_table: comments
        columns:
          id: comments.id
          authorName:
            source: comment_author.name
            join: { table: users, alias: comment_author, on: comments.user_id = users.id }
usecase:
  name: テスト
  root: Comment
  response_mapping:
    - { field: id, source: Comment.id }
    - { field: author_name, source: Comment.authorName }
"#;
        let doc = parser::parse(yaml).unwrap();
        let cols = collect_persistence_columns(&doc);
        assert!(cols.contains(&("comments".to_string(), "id".to_string())));
        // alias comment_author → 実テーブル users に解決
        assert!(cols.contains(&("users".to_string(), "name".to_string())));
        assert!(!cols.iter().any(|(t, _)| t == "comment_author"));
    }
}
