use thiserror::Error;

use crate::ast::UsmlDocument;

#[derive(Debug, Error)]
pub enum ParseError {
    #[error("YAML parse error: {0}")]
    YamlError(#[from] serde_yaml::Error),

    #[error("invalid version: expected '0.1', got '{0}'")]
    InvalidVersion(String),
}

/// USML ドキュメントを YAML 文字列からパースする
pub fn parse(input: &str) -> Result<UsmlDocument, ParseError> {
    let doc: UsmlDocument = serde_yaml::from_str(input)?;

    if doc.version != "0.1" {
        return Err(ParseError::InvalidVersion(doc.version));
    }

    Ok(doc)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_minimal_document() {
        let yaml = r#"
version: "0.1"
import:
  openapi: ./api.yaml#paths["/users"].get.responses["200"]
  dbml:
    - ./schema.dbml#tables["users"]
usecase:
  name: ユーザー一覧取得
  response_mapping:
    - field: id
      source: users.id
    - field: name
      source: users.name
"#;
        let doc = parse(yaml).expect("parse should succeed");
        assert_eq!(doc.version, "0.1");
        assert_eq!(doc.usecase.name, "ユーザー一覧取得");
        assert_eq!(doc.usecase.response_mapping.len(), 2);
        assert_eq!(doc.usecase.response_mapping[0].field, "id");
        assert_eq!(
            doc.usecase.response_mapping[0].source.as_deref(),
            Some("users.id")
        );
    }

    #[test]
    fn test_invalid_version() {
        let yaml = r#"
version: "9.9"
import: {}
usecase:
  name: test
  response_mapping: []
"#;
        let result = parse(yaml);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ParseError::InvalidVersion(_)));
    }

    #[test]
    fn test_document_with_join() {
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
        type: LEFT JOIN
"#;
        let doc = parse(yaml).expect("parse should succeed");
        let avatar = &doc.usecase.response_mapping[1];
        assert_eq!(avatar.field, "avatar_url");
        let join = avatar.join.as_ref().expect("join should exist");
        assert_eq!(join.table, "profiles");
        assert_eq!(join.on, "users.id = profiles.user_id");
    }

    #[test]
    fn test_document_with_aggregate() {
        let yaml = r#"
version: "0.1"
import:
  openapi: ./api.yaml#paths["/posts"].get.responses["200"]
  dbml:
    - ./schema.dbml#tables["posts"]
    - ./schema.dbml#tables["likes"]
usecase:
  name: 投稿一覧
  response_mapping:
    - field: like_count
      source: likes.id
      join:
        table: likes
        on: posts.id = likes.post_id
      aggregate:
        type: COUNT
        group_by: posts.id
"#;
        let doc = parse(yaml).expect("parse should succeed");
        let like_count = &doc.usecase.response_mapping[0];
        let agg = like_count
            .aggregate
            .as_ref()
            .expect("aggregate should exist");
        assert_eq!(agg.r#type, "COUNT");
        assert_eq!(agg.group_by.as_deref(), Some("posts.id"));
    }

    #[test]
    fn test_document_with_filters_and_transforms() {
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
    - field: display_name
      source: profiles.display_name
  filters:
    - param: status
      maps_to: WHERE
      condition: users.status = :status
    - param: page
      maps_to: PAGINATION
      strategy: offset
      page_size: 20
  transforms:
    - target: display_name
      type: COALESCE
      sources:
        - profiles.display_name
        - users.name
      fallback: "anonymous"
"#;
        let doc = parse(yaml).expect("parse should succeed");
        assert_eq!(doc.usecase.filters.len(), 2);
        assert_eq!(doc.usecase.filters[0].param, "status");
        assert_eq!(doc.usecase.filters[1].maps_to, "PAGINATION");
        assert_eq!(doc.usecase.transforms.len(), 1);
        assert_eq!(doc.usecase.transforms[0].target, "display_name");
        assert_eq!(doc.usecase.transforms[0].r#type, "COALESCE");
    }
}
