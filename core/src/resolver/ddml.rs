use std::fs;

use serde::Deserialize;

use super::{DdmlItem, ResolverError};

/// ddml ファイル（.ddml.yaml）を読み込み、項目情報を抽出する。
///
/// ddml クレートには依存せず、usml が必要とする最小フィールドのみを
/// serde 構造体で受ける。未知フィールドは無視する（deny しない = usml の従来方針）。
pub fn resolve_ddml(file_path: &str) -> Result<Vec<DdmlItem>, ResolverError> {
    let content = fs::read_to_string(file_path)
        .map_err(|e| ResolverError::IoError(file_path.to_string(), e))?;

    parse_ddml_content(&content, file_path)
}

/// ddml 文字列をパースして項目情報を抽出する。
pub fn parse_ddml_content(content: &str, source: &str) -> Result<Vec<DdmlItem>, ResolverError> {
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

            // storage.schema が table/column を両方持つときのみ (table, column) を抽出する
            let schema = raw
                .storage
                .as_ref()
                .and_then(|s| s.schema.as_ref())
                .and_then(|sc| match (&sc.table, &sc.column) {
                    (Some(t), Some(c)) if !t.is_empty() && !c.is_empty() => {
                        Some((t.clone(), c.clone()))
                    }
                    _ => None,
                });

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

// ============================================================
// 最小 serde 構造体（ddml v0.1 / v0.2 の抽出に必要な部分のみ）
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
    #[serde(default)]
    schema: Option<DdmlSchemaRaw>,
}

#[derive(Debug, Deserialize)]
struct DdmlSchemaRaw {
    #[serde(default)]
    table: Option<String>,
    #[serde(default)]
    column: Option<String>,
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
}
