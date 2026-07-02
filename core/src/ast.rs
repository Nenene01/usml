use indexmap::IndexMap;
use serde::Deserialize;

/// USML ドキュメントのルート（v0.2）
///
/// v0.2 ではドメイン層を第一級概念として導入し、3つの境界を分離する:
/// - Interface(OpenAPI) ⇄ Domain : `usecase.response_mapping`
/// - Domain ⇄ Infrastructure(DB) : `domain.entities[].persistence`
/// - Domain 内導出 / Interface 表示変換 : `domain.entities[].derived` / `usecase.presentation`
#[derive(Debug, Deserialize)]
pub struct UsmlDocument {
    pub version: String,
    pub import: Import,
    /// v0.2 で新設・必須。ドメインモデル定義
    pub domain: Domain,
    pub usecase: Usecase,
}

/// 外部仕様ファイルへの参照
#[derive(Debug, Deserialize)]
pub struct Import {
    #[serde(default)]
    pub openapi: Option<String>,
    #[serde(default)]
    pub dbml: Option<Vec<String>>,
    /// ddml（項目定義の正本）への参照。fragment 不要の素のパスのリスト（1ファイル=1業務）。
    /// 省略時は ddml トレース検証を行わない（完全後方互換）。
    #[serde(default)]
    pub ddml: Option<Vec<String>>,
}

// ============================================================
// domain セクション
// ============================================================

/// ドメイン層定義
#[derive(Debug, Deserialize)]
pub struct Domain {
    /// 値オブジェクト定義（型整合検証の基礎）
    #[serde(default)]
    pub value_objects: Vec<ValueObject>,
    /// エンティティ定義（宣言順を保持）
    #[serde(default)]
    pub entities: IndexMap<String, Entity>,
}

/// 値オブジェクト（VO）
#[derive(Debug, Deserialize)]
pub struct ValueObject {
    pub name: String,
    /// 基底型: string / integer / number / boolean / datetime
    pub base: String,
    /// 意味的フォーマット（OpenAPI の format と照合可能）
    #[serde(default)]
    pub format: Option<String>,
}

/// ドメインエンティティ
#[derive(Debug, Deserialize)]
pub struct Entity {
    /// フィールド名 → 型名（組み込み型または ValueObject 名）。宣言順を保持
    #[serde(default)]
    pub fields: IndexMap<String, String>,
    /// Domain ⇄ DB 永続化マッピング
    pub persistence: Persistence,
    /// ドメイン内の導出ロジック（COALESCE / CONCAT / CONDITIONAL_SOURCE）
    #[serde(default)]
    pub derived: Vec<Derived>,
    /// エンティティ間の関連
    #[serde(default)]
    pub relations: IndexMap<String, Relation>,
}

/// Domain ⇄ DB の永続化マッピング
#[derive(Debug, Deserialize)]
pub struct Persistence {
    /// エンティティの基点テーブル
    pub root_table: String,
    /// ドメインフィールド名 → DBカラムマッピング。宣言順を保持
    #[serde(default)]
    pub columns: IndexMap<String, ColumnMapping>,
}

/// カラムマッピング: 単純対応（文字列）か詳細対応（JOIN/集約付き）
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum ColumnMapping {
    /// `id: users.id` のような単純対応
    Simple(String),
    /// `source` + `join` / `join_chain` / `aggregate` の詳細対応
    Detailed(DetailedColumn),
}

/// JOIN・集約を伴う詳細カラムマッピング
#[derive(Debug, Deserialize)]
pub struct DetailedColumn {
    pub source: String,
    #[serde(default)]
    pub join: Option<Join>,
    /// 多段結合
    #[serde(default)]
    pub join_chain: Option<Vec<JoinChainEntry>>,
    /// 集約（COUNT / SUM / AVG / MIN / MAX）
    #[serde(default)]
    pub aggregate: Option<Aggregate>,
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

/// ドメイン内の導出ロジック
#[derive(Debug, Deserialize)]
pub struct Derived {
    /// 導出対象のドメインフィールド名
    pub field: String,
    /// COALESCE / CONCAT / CONDITIONAL_SOURCE
    pub r#type: String,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub sources: Option<Vec<String>>,
    /// COALESCE の固定フォールバック値
    #[serde(default)]
    pub fallback: Option<String>,
    /// CONCAT の区切り文字
    #[serde(default)]
    pub separator: Option<String>,
    /// CONDITIONAL_SOURCE の適用条件
    #[serde(default)]
    pub when: Option<Vec<TransformCondition>>,
    /// CONDITIONAL_SOURCE で条件マッチ時のソース
    #[serde(default)]
    pub then_source: Option<String>,
    /// CONDITIONAL_SOURCE で条件非マッチ時のソース
    #[serde(default)]
    pub else_source: Option<String>,
}

/// エンティティ間の関連
#[derive(Debug, Deserialize)]
pub struct Relation {
    /// 関連先エンティティ名
    pub target: String,
    /// has_many / has_one / belongs_to / many_to_many
    pub kind: String,
    /// 結合条件
    pub on: String,
    /// 多対多の中間テーブル
    #[serde(default)]
    pub through: Option<Through>,
}

/// 多対多の中間テーブル定義
#[derive(Debug, Deserialize)]
pub struct Through {
    pub table: String,
    pub on: String,
}

// ============================================================
// usecase セクション
// ============================================================

/// ユースケース定義
#[derive(Debug, Deserialize)]
pub struct Usecase {
    pub name: String,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub output: Option<String>,
    /// ルートエンティティ名（domain.entities に存在する必要がある）
    pub root: String,
    /// Domain ⇄ Interface の射影（source はドメイン語彙 `Entity.field`）
    pub response_mapping: Vec<ResponseMapping>,
    #[serde(default)]
    pub filters: Vec<Filter>,
    /// Interface の表示変換（MASK / CASE）
    #[serde(default)]
    pub presentation: Vec<Presentation>,
}

/// レスポンスフィールドとドメインフィールドの対応（Domain ⇄ Interface）
#[derive(Debug, Deserialize)]
pub struct ResponseMapping {
    pub field: String,
    /// ドメイン語彙 `<Entity>.<field>`、または配列時は関連名 `<Entity>.<relation>`
    #[serde(default)]
    pub source: Option<String>,
    /// `array` の場合は配列レスポンス
    #[serde(default)]
    pub r#type: Option<String>,
    /// 配列要素のマッピング（関連先エンティティの射影）
    #[serde(default)]
    pub fields: Option<Vec<ResponseMapping>>,
}

/// リクエストパラメータのDBクエリへの対応
#[derive(Debug, Deserialize)]
pub struct Filter {
    pub param: String,
    pub maps_to: String,
    /// WHERE 条件式（ドメイン語彙を推奨）
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

/// Interface の表示変換（旧 transforms の表示系）
#[derive(Debug, Deserialize)]
pub struct Presentation {
    /// 対象レスポンスフィールド名（response_mapping の field）
    pub target: String,
    /// MASK / CASE
    pub r#type: String,
    /// 単一ソース（CASE の評価対象など）
    #[serde(default)]
    pub source: Option<String>,
    /// MASK のパターン
    #[serde(default)]
    pub mask_pattern: Option<String>,
    /// CASE の分岐
    #[serde(default)]
    pub when: Option<Vec<CaseWhen>>,
    /// CASE のデフォルト値
    #[serde(default, rename = "else")]
    pub else_value: Option<String>,
    /// 変換の適用条件（閲覧者文脈など。複数列記で AND）
    #[serde(default)]
    pub condition: Option<Vec<TransformCondition>>,
}

/// CASE 分岐の各エントリ
#[derive(Debug, Deserialize)]
pub struct CaseWhen {
    pub value: String,
    pub then: String,
}

/// 条件付き変換の条件（derived / presentation 共用）
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
