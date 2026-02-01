use serde::Deserialize;

/// USML ドキュメントのルート
#[derive(Debug, Deserialize)]
pub struct UsmlDocument {
    pub version: String,
    pub import: Import,
    pub usecase: Usecase,
}

/// 外部仕様ファイルへの参照
#[derive(Debug, Deserialize)]
pub struct Import {
    pub openapi: Option<String>,
    pub dbml: Option<Vec<String>>,
}

/// ユースケース定義
#[derive(Debug, Deserialize)]
pub struct Usecase {
    pub name: String,
    pub summary: Option<String>,
    #[serde(default)]
    pub output: Option<String>,
    pub response_mapping: Vec<ResponseMapping>,
    #[serde(default)]
    pub filters: Vec<Filter>,
    #[serde(default)]
    pub transforms: Vec<Transform>,
}

/// レスポンスフィールドとDBカラムの対応
#[derive(Debug, Deserialize)]
pub struct ResponseMapping {
    pub field: String,
    #[serde(default)]
    pub source: Option<String>,
    /// `array` の場合は配列レスポンス
    #[serde(default)]
    pub r#type: Option<String>,
    /// 配列要素の生成テーブル
    #[serde(default)]
    pub source_table: Option<String>,
    #[serde(default)]
    pub join: Option<Join>,
    /// 多段結合
    #[serde(default)]
    pub join_chain: Option<Vec<JoinChainEntry>>,
    /// 集約
    #[serde(default)]
    pub aggregate: Option<Aggregate>,
    /// 配列のサブフィールド
    #[serde(default)]
    pub fields: Option<Vec<ResponseMapping>>,
}

/// テーブル結合定義
#[derive(Debug, Deserialize)]
pub struct Join {
    pub table: String,
    pub on: String,
    #[serde(default)]
    pub r#type: Option<String>,
    #[serde(default)]
    pub alias: Option<String>,
}

/// 多段結合の各エントリ
#[derive(Debug, Deserialize)]
pub struct JoinChainEntry {
    pub table: String,
    pub on: String,
}

/// 集約定義
#[derive(Debug, Deserialize)]
pub struct Aggregate {
    pub r#type: String,
    #[serde(default)]
    pub group_by: Option<String>,
}

/// リクエストパラメータのDBクエリへの対応
#[derive(Debug, Deserialize)]
pub struct Filter {
    pub param: String,
    pub maps_to: String,
    /// WHERE 条件式
    #[serde(default)]
    pub condition: Option<String>,
    /// ページネーション戦略
    #[serde(default)]
    pub strategy: Option<String>,
    #[serde(default)]
    pub page_size: Option<u32>,
    #[serde(default)]
    pub limit_param: Option<String>,
    #[serde(default)]
    pub max_page_size: Option<u32>,
    #[serde(default)]
    pub cursor_field: Option<String>,
    /// ソートのデフォルトカラム
    #[serde(default)]
    pub default_column: Option<String>,
    #[serde(default)]
    pub default_direction: Option<String>,
    #[serde(default)]
    pub allowed_columns: Option<Vec<String>>,
    #[serde(default)]
    pub allowed_directions: Option<Vec<String>>,
}

/// 変換・加工定義
#[derive(Debug, Deserialize)]
pub struct Transform {
    pub target: String,
    pub r#type: String,
    /// 単一ソース
    #[serde(default)]
    pub source: Option<String>,
    /// 複数ソース（COALESCE, CONCAT など）
    #[serde(default)]
    pub sources: Option<Vec<String>>,
    /// COALESCE 時の固定フォールバック値
    #[serde(default)]
    pub fallback: Option<String>,
    /// CONCAT 時の区切り文字
    #[serde(default)]
    pub separator: Option<String>,
    /// CASE 時の分岐
    #[serde(default)]
    pub when: Option<Vec<CaseWhen>>,
    /// CASE 時のデフォルト値
    #[serde(default)]
    pub else_value: Option<String>,
    /// MASK 時のパターン
    #[serde(default)]
    pub mask_pattern: Option<String>,
    /// 条件付き変換の適用条件
    #[serde(default)]
    pub condition: Option<Vec<TransformCondition>>,
    /// CONDITIONAL_SOURCE 時の条件マッチ時のソース
    #[serde(default)]
    pub then_source: Option<String>,
    /// CONDITIONAL_SOURCE 時の条件非マッチ時のソース
    #[serde(default)]
    pub else_source: Option<String>,
}

/// CASE 分岐の各エントリ
#[derive(Debug, Deserialize)]
pub struct CaseWhen {
    pub value: String,
    pub then: String,
}

/// 条件付き変換の条件
#[derive(Debug, Deserialize)]
pub struct TransformCondition {
    /// リクエストパラメータを参照
    #[serde(default)]
    pub param: Option<String>,
    /// レスポンスフィールドを参照
    #[serde(default)]
    pub field: Option<String>,
    /// DBカラムを参照
    #[serde(default)]
    pub source: Option<String>,
    pub operator: String,
    pub value: String,
}
