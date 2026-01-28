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
    pub columns: Vec<String>,
}

/// OpenAPI から抽出されたレスポンス情報
#[derive(Debug, Clone)]
pub struct OpenapiResponse {
    /// レスポンスのフィールド名一覧
    pub fields: Vec<String>,
    /// パラメータ名一覧
    pub parameters: Vec<String>,
}
