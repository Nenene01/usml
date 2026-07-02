pub mod dbml;
pub mod openapi;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ResolverError {
    #[error("ファイル読み込みエラー '{0}': {1}")]
    IoError(String, std::io::Error),

    #[error("DBML パースエラー '{0}': {1}")]
    DbmlParseError(String, String),

    #[error("OpenAPI パースエラー '{0}': {1}")]
    OpenapiParseError(String, String),

    #[error("参照先が見つかりません: '{0}'")]
    NotFound(String),
}

/// DBML から抽出されたテーブル情報
#[derive(Debug, Clone)]
pub struct DbmlTable {
    pub name: String,
    pub columns: Vec<DbmlColumn>,
}

impl DbmlTable {
    /// カラム名で検索する
    pub fn column(&self, name: &str) -> Option<&DbmlColumn> {
        self.columns.iter().find(|c| c.name == name)
    }

    /// カラム名が存在するか
    pub fn has_column(&self, name: &str) -> bool {
        self.columns.iter().any(|c| c.name == name)
    }
}

/// DBML カラム情報（型整合検証のため型を保持）
#[derive(Debug, Clone)]
pub struct DbmlColumn {
    pub name: String,
    /// DBML 上の型表記（例: "integer", "varchar(255)", "timestamp"）
    pub col_type: String,
}

/// OpenAPI から抽出されたレスポンス情報
#[derive(Debug, Clone)]
pub struct OpenapiResponse {
    /// レスポンスのフィールド一覧（型情報付き）
    pub fields: Vec<OpenapiField>,
    /// パラメータ名一覧
    pub parameters: Vec<String>,
}

impl OpenapiResponse {
    /// フィールド名で検索する
    pub fn field(&self, name: &str) -> Option<&OpenapiField> {
        self.fields.iter().find(|f| f.name == name)
    }

    /// フィールド名が存在するか
    pub fn has_field(&self, name: &str) -> bool {
        self.fields.iter().any(|f| f.name == name)
    }
}

/// OpenAPI レスポンスフィールド情報（型整合検証のため型を保持）
#[derive(Debug, Clone)]
pub struct OpenapiField {
    pub name: String,
    /// JSON Schema の type（例: "string", "integer"）
    pub type_: Option<String>,
    /// JSON Schema の format（例: "uri", "email", "date-time"）
    pub format: Option<String>,
}
