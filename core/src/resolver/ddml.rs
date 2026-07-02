use std::collections::HashMap;
use std::fs;
use std::path::Path;

use serde::Deserialize;

use super::{DdmlItem, ResolverError};

/// ddml の辞書（ddml.dict.yaml）から抽出した論理名→物理テーブルの対応。
///
/// 契約 §1.5 の entities（論理データ資産）のうち、`table`（物理名）を
/// 持つものだけを保持する。usml は entity 経由の物理テーブル解決にのみ
/// 使うため、sources / storages は読み飛ばす。
#[derive(Debug, Clone, Default)]
pub struct DdmlDict {
    /// entity 名 → 物理テーブル名（table を持つ entity のみ）
    entity_tables: HashMap<String, String>,
}

impl DdmlDict {
    /// entity 名に対応する物理テーブル名を返す（table 未確定なら None）
    fn table_of(&self, entity: &str) -> Option<&str> {
        self.entity_tables.get(entity).map(|s| s.as_str())
    }
}

/// ddml ファイル（.ddml.yaml）を読み込み、項目情報を抽出する。
///
/// ddml クレートには依存せず、usml が必要とする最小フィールドのみを
/// serde 構造体で受ける。未知フィールドは無視する（deny しない = usml の従来方針）。
///
/// 契約 §1.5.4 に従い、対象ファイルと同じディレクトリ → その親ディレクトリの順で
/// `ddml.dict.yaml` を探し、entity 経由の物理テーブル解決に用いる。
/// 辞書が読めない / 解析不能なときは警告として文字列を返し、空辞書で続行する
/// （既存の import 解決失敗と同じ流儀）。
pub fn resolve_ddml(file_path: &str) -> Result<(Vec<DdmlItem>, Vec<String>), ResolverError> {
    let content = fs::read_to_string(file_path)
        .map_err(|e| ResolverError::IoError(file_path.to_string(), e))?;

    let (dict, warnings) = load_dict_for(file_path);
    let items = parse_ddml_content_with_dict(&content, file_path, &dict)?;
    Ok((items, warnings))
}

/// ddml 文字列をパースして項目情報を抽出する（辞書なし = v0.2 互換）。
///
/// 物理テーブルは schema.table のみから解決する。entity を使うには
/// [`parse_ddml_content_with_dict`] を用いること。
pub fn parse_ddml_content(content: &str, source: &str) -> Result<Vec<DdmlItem>, ResolverError> {
    parse_ddml_content_with_dict(content, source, &DdmlDict::default())
}

/// ddml 文字列を辞書付きでパースして項目情報を抽出する。
///
/// 物理テーブルの解決順（契約 §1.5.2）: `entity.table`（辞書）> `schema.table`（v0.2 互換）。
/// 両方あっても entity を優先する（不一致検証は ddml 側 E016 の責務なので usml では検証しない）。
/// column は従来どおり schema.column。table と column が揃ったときのみ schema を Some にする。
pub fn parse_ddml_content_with_dict(
    content: &str,
    source: &str,
    dict: &DdmlDict,
) -> Result<Vec<DdmlItem>, ResolverError> {
    let root: DdmlRoot = serde_yaml::from_str(content)
        .map_err(|e| ResolverError::DdmlParseError(source.to_string(), e.to_string()))?;

    let mut items = Vec::new();
    for feature in &root.features {
        for raw in &feature.items {
            let Some(item_id) = raw.id.clone() else {
                // id を持たない項目はトレースの対象にできないためスキップする
                continue;
            };
            let item_name = raw.name.clone().unwrap_or_default();

            // storage.status。storage も status も省略時は hypothesis 扱い（契約 §1.1）
            let storage_status = raw
                .storage
                .as_ref()
                .and_then(|s| s.status.clone())
                .unwrap_or_else(|| "hypothesis".to_string());

            let schema = raw.storage.as_ref().and_then(|s| resolve_schema(s, dict));

            items.push(DdmlItem {
                item_id,
                item_name,
                storage_status,
                schema,
            });
        }
    }

    Ok(items)
}

/// storage から (table, column) を解決する。
///
/// table: entity.table（辞書）> schema.table。column: schema.column。
/// 両方が揃い、いずれも空でないときのみ Some を返す。
fn resolve_schema(storage: &DdmlStorageRaw, dict: &DdmlDict) -> Option<(String, String)> {
    // column は schema.column から
    let column = storage
        .schema
        .as_ref()
        .and_then(|sc| sc.column.as_ref())
        .filter(|c| !c.is_empty())?;

    // table は entity.table を優先し、無ければ schema.table
    let table = storage
        .entity
        .as_ref()
        .filter(|e| !e.is_empty())
        .and_then(|e| dict.table_of(e))
        .map(|t| t.to_string())
        .or_else(|| {
            storage
                .schema
                .as_ref()
                .and_then(|sc| sc.table.as_ref())
                .filter(|t| !t.is_empty())
                .cloned()
        })?;

    Some((table, column.clone()))
}

/// 対象 .ddml.yaml のパスから辞書 `ddml.dict.yaml` を探索して読み込む。
///
/// 探索順（契約 §1.5.4）: 同じディレクトリ → その親ディレクトリ。最初に見つかった
/// ものを使う。無ければ空辞書。読めない / 解析不能な辞書は警告文字列を返し、
/// 空辞書で続行する。
fn load_dict_for(file_path: &str) -> (DdmlDict, Vec<String>) {
    let path = Path::new(file_path);
    let same_dir = path.parent();
    let parent_dir = same_dir.and_then(|d| d.parent());

    for dir in [same_dir, parent_dir].into_iter().flatten() {
        let candidate = dir.join("ddml.dict.yaml");
        if !candidate.is_file() {
            continue;
        }
        return match load_dict(&candidate) {
            Ok(dict) => (dict, Vec::new()),
            Err(msg) => (DdmlDict::default(), vec![msg]),
        };
    }

    (DdmlDict::default(), Vec::new())
}

/// 辞書ファイルを読み込んでパースする。失敗時は警告メッセージを Err で返す。
fn load_dict(path: &Path) -> Result<DdmlDict, String> {
    let display = path.to_string_lossy().to_string();
    let content = fs::read_to_string(path)
        .map_err(|e| format!("辞書の読み込みに失敗しました '{}': {}", display, e))?;
    let root: DictRoot = serde_yaml::from_str(&content)
        .map_err(|e| format!("辞書の解析に失敗しました '{}': {}", display, e))?;

    let mut entity_tables = HashMap::new();
    for entity in root.entities {
        if let Some(table) = entity.table.filter(|t| !t.is_empty()) {
            entity_tables.insert(entity.name, table);
        }
    }
    Ok(DdmlDict { entity_tables })
}

// ============================================================
// 最小 serde 構造体（ddml v0.1 / v0.2 / v0.3 の抽出に必要な部分のみ）
// ============================================================

#[derive(Debug, Deserialize)]
struct DdmlRoot {
    #[serde(default)]
    features: Vec<DdmlFeature>,
}

#[derive(Debug, Deserialize)]
struct DdmlFeature {
    #[serde(default)]
    items: Vec<DdmlItemRaw>,
}

#[derive(Debug, Deserialize)]
struct DdmlItemRaw {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    storage: Option<DdmlStorageRaw>,
}

#[derive(Debug, Deserialize)]
struct DdmlStorageRaw {
    #[serde(default)]
    status: Option<String>,
    /// v0.3: 論理名（辞書 entities の name を参照）。物理テーブル解決に使う
    #[serde(default)]
    entity: Option<String>,
    #[serde(default)]
    schema: Option<DdmlSchemaRaw>,
}

#[derive(Debug, Deserialize)]
struct DdmlSchemaRaw {
    /// v0.3 では table は entity 側に移り省略可。schema.table は v0.2 互換で残る
    #[serde(default)]
    table: Option<String>,
    #[serde(default)]
    column: Option<String>,
}

// ============================================================
// 辞書 ddml.dict.yaml の最小 serde 構造体（entities のみ）
// ============================================================

#[derive(Debug, Deserialize)]
struct DictRoot {
    #[serde(default)]
    entities: Vec<DictEntity>,
}

#[derive(Debug, Deserialize)]
struct DictEntity {
    name: String,
    #[serde(default)]
    table: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
ddml: "0.2"
business:
  id: BIZ-001
  name: 受注管理
features:
  - id: FTR-001
    name: 受注登録
    items:
      - id: ITM-001
        name: 受注番号
        definition:
          text: 受注を一意に識別する番号
          status: confirmed
        storage:
          text: orders.order_no varchar(16)
          status: confirmed
          schema:
            table: orders
            column: order_no
            type: varchar(16)
      - id: ITM-002
        name: 受注日
        storage:
          status: hypothesis
          schema:
            table: orders
            column: ordered_on
      - id: ITM-003
        name: 備考
        storage:
          status: confirmed
"#;

    #[test]
    fn test_parse_ddml_content_basic() {
        let items = parse_ddml_content(SAMPLE, "test.ddml.yaml").expect("パースに失敗しました");
        assert_eq!(items.len(), 3);

        let itm1 = items.iter().find(|i| i.item_id == "ITM-001").unwrap();
        assert_eq!(itm1.item_name, "受注番号");
        assert_eq!(itm1.storage_status, "confirmed");
        assert_eq!(
            itm1.schema,
            Some(("orders".to_string(), "order_no".to_string()))
        );

        // status 省略なしの hypothesis + schema あり
        let itm2 = items.iter().find(|i| i.item_id == "ITM-002").unwrap();
        assert_eq!(itm2.storage_status, "hypothesis");
        assert_eq!(
            itm2.schema,
            Some(("orders".to_string(), "ordered_on".to_string()))
        );

        // schema 無し（confirmed だが未構造化）
        let itm3 = items.iter().find(|i| i.item_id == "ITM-003").unwrap();
        assert_eq!(itm3.storage_status, "confirmed");
        assert_eq!(itm3.schema, None);
    }

    #[test]
    fn test_parse_ddml_status_defaults_to_hypothesis() {
        // storage 自体が無い項目は hypothesis 扱い
        let yaml = r#"
ddml: "0.1"
features:
  - id: FTR-001
    name: 機能
    items:
      - id: ITM-001
        name: 項目
"#;
        let items = parse_ddml_content(yaml, "t.ddml.yaml").unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].storage_status, "hypothesis");
        assert_eq!(items[0].schema, None);
    }

    #[test]
    fn test_parse_ddml_ignores_unknown_fields() {
        // 未知のトップレベル / 項目フィールドがあっても無視して抽出できる
        let yaml = r#"
ddml: "0.2"
business: { id: BIZ-9, name: X, extra: ignored }
unknown_top: 123
features:
  - id: FTR-001
    name: 機能
    weird: true
    items:
      - id: ITM-001
        name: 項目
        outputs: [a, b]
        questions: [{ text: なぜ }]
        storage:
          status: confirmed
          schema: { table: t, column: c, type: varchar(8), constraints: [pk] }
"#;
        let items = parse_ddml_content(yaml, "t.ddml.yaml").unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].schema, Some(("t".to_string(), "c".to_string())));
    }

    // ---- v0.3: 辞書 + entity 経由の物理テーブル解決 ----

    /// entity を持つ v0.3 形式。table は schema に無く entity 経由で解決する。
    const V03_SAMPLE: &str = r#"
ddml: "0.3"
features:
  - id: FTR-001
    name: 受注登録
    items:
      - id: ITM-001
        name: 受注番号
        storage:
          status: confirmed
          entity: 受注情報
          schema:
            column: order_no
            type: varchar(16)
      - id: ITM-002
        name: 顧客名
        storage:
          status: confirmed
          entity: 顧客情報
          schema:
            column: customer_name
"#;

    fn dict_from(yaml: &str) -> DdmlDict {
        let root: DictRoot = serde_yaml::from_str(yaml).unwrap();
        let mut entity_tables = HashMap::new();
        for e in root.entities {
            if let Some(t) = e.table.filter(|t| !t.is_empty()) {
                entity_tables.insert(e.name, t);
            }
        }
        DdmlDict { entity_tables }
    }

    #[test]
    fn test_entity_resolves_table_from_dict() {
        // 辞書に受注情報→orders があれば entity 経由で (orders, order_no) を解決する
        let dict = dict_from(
            r#"
ddml: "0.3"
entities:
  - name: 受注情報
    table: orders
  - name: 顧客情報
"#,
        );
        let items = parse_ddml_content_with_dict(V03_SAMPLE, "t.ddml.yaml", &dict).unwrap();

        let itm1 = items.iter().find(|i| i.item_id == "ITM-001").unwrap();
        assert_eq!(
            itm1.schema,
            Some(("orders".to_string(), "order_no".to_string()))
        );

        // 顧客情報は table 未確定 → schema.table も無い → 抽出 None（トレース対象外）
        let itm2 = items.iter().find(|i| i.item_id == "ITM-002").unwrap();
        assert_eq!(itm2.schema, None);
    }

    #[test]
    fn test_entity_table_takes_precedence_over_schema_table() {
        // entity.table と schema.table が両方あるときは entity を優先（不一致検証は ddml E016 の責務）
        let dict = dict_from(
            r#"
ddml: "0.3"
entities:
  - name: 受注情報
    table: orders
"#,
        );
        let yaml = r#"
ddml: "0.3"
features:
  - id: FTR-001
    name: 機能
    items:
      - id: ITM-001
        name: 受注番号
        storage:
          status: confirmed
          entity: 受注情報
          schema:
            table: legacy_orders
            column: order_no
"#;
        let items = parse_ddml_content_with_dict(yaml, "t.ddml.yaml", &dict).unwrap();
        assert_eq!(
            items[0].schema,
            Some(("orders".to_string(), "order_no".to_string()))
        );
    }

    #[test]
    fn test_v02_schema_table_backward_compat_without_dict() {
        // 辞書なし（空辞書）でも schema.table 直書きの v0.2 形式はそのまま解決する
        let dict = DdmlDict::default();
        let items = parse_ddml_content_with_dict(SAMPLE, "t.ddml.yaml", &dict).unwrap();
        let itm1 = items.iter().find(|i| i.item_id == "ITM-001").unwrap();
        assert_eq!(
            itm1.schema,
            Some(("orders".to_string(), "order_no".to_string()))
        );
    }

    #[test]
    fn test_entity_without_table_falls_back_to_none() {
        // entity はあるが辞書に table 未確定、schema.table も無い → None
        let dict = dict_from(
            r#"
ddml: "0.3"
entities:
  - name: 顧客情報
"#,
        );
        let yaml = r#"
ddml: "0.3"
features:
  - id: FTR-001
    name: 機能
    items:
      - id: ITM-001
        name: 顧客名
        storage:
          status: confirmed
          entity: 顧客情報
          schema:
            column: customer_name
"#;
        let items = parse_ddml_content_with_dict(yaml, "t.ddml.yaml", &dict).unwrap();
        assert_eq!(items[0].schema, None);
    }

    #[test]
    fn test_unknown_entity_falls_back_to_schema_table() {
        // entity が辞書に無ければ schema.table にフォールバック（usml では E015 検証しない）
        let dict = DdmlDict::default();
        let yaml = r#"
ddml: "0.3"
features:
  - id: FTR-001
    name: 機能
    items:
      - id: ITM-001
        name: 受注番号
        storage:
          status: confirmed
          entity: 未登録
          schema:
            table: orders
            column: order_no
"#;
        let items = parse_ddml_content_with_dict(yaml, "t.ddml.yaml", &dict).unwrap();
        assert_eq!(
            items[0].schema,
            Some(("orders".to_string(), "order_no".to_string()))
        );
    }

    // ---- 辞書探索（同ディレクトリ → 親ディレクトリ）----

    #[test]
    fn test_resolve_ddml_finds_dict_in_same_dir() {
        let tmp = std::env::temp_dir().join(format!("usml_dict_same_{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(
            tmp.join("ddml.dict.yaml"),
            "ddml: \"0.3\"\nentities:\n  - name: 受注情報\n    table: orders\n",
        )
        .unwrap();
        let ddml_path = tmp.join("order.ddml.yaml");
        fs::write(&ddml_path, V03_SAMPLE).unwrap();

        let (items, warnings) = resolve_ddml(ddml_path.to_str().unwrap()).unwrap();
        assert!(warnings.is_empty());
        let itm1 = items.iter().find(|i| i.item_id == "ITM-001").unwrap();
        assert_eq!(
            itm1.schema,
            Some(("orders".to_string(), "order_no".to_string()))
        );
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_resolve_ddml_finds_dict_in_parent_dir() {
        let root = std::env::temp_dir().join(format!("usml_dict_parent_{}", std::process::id()));
        let sub = root.join("docs");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&sub).unwrap();
        // 辞書は親（root）に、.ddml.yaml は子（docs）に置く
        fs::write(
            root.join("ddml.dict.yaml"),
            "ddml: \"0.3\"\nentities:\n  - name: 受注情報\n    table: orders\n",
        )
        .unwrap();
        let ddml_path = sub.join("order.ddml.yaml");
        fs::write(&ddml_path, V03_SAMPLE).unwrap();

        let (items, warnings) = resolve_ddml(ddml_path.to_str().unwrap()).unwrap();
        assert!(warnings.is_empty());
        let itm1 = items.iter().find(|i| i.item_id == "ITM-001").unwrap();
        assert_eq!(
            itm1.schema,
            Some(("orders".to_string(), "order_no".to_string()))
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn test_resolve_ddml_no_dict_is_empty() {
        // 辞書が無ければ空辞書。entity は解決されず schema.table のみ効く
        let tmp = std::env::temp_dir().join(format!("usml_dict_none_{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        let ddml_path = tmp.join("order.ddml.yaml");
        fs::write(&ddml_path, V03_SAMPLE).unwrap();

        let (items, warnings) = resolve_ddml(ddml_path.to_str().unwrap()).unwrap();
        assert!(warnings.is_empty());
        // entity は解決できず、schema.table も無いので全て None
        assert!(items.iter().all(|i| i.schema.is_none()));
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_resolve_ddml_broken_dict_warns_and_continues() {
        // 解析不能な辞書は警告を返し、空辞書で続行する
        let tmp = std::env::temp_dir().join(format!("usml_dict_broken_{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("ddml.dict.yaml"), "entities: [ this is : broken").unwrap();
        let ddml_path = tmp.join("order.ddml.yaml");
        fs::write(&ddml_path, V03_SAMPLE).unwrap();

        let (items, warnings) = resolve_ddml(ddml_path.to_str().unwrap()).unwrap();
        assert_eq!(warnings.len(), 1, "解析不能な辞書は警告 1 件を返すべき");
        // 空辞書扱いなので entity 未解決 = schema None
        assert!(items.iter().all(|i| i.schema.is_none()));
        let _ = fs::remove_dir_all(&tmp);
    }
}
