use thiserror::Error;

use crate::ast::UsmlDocument;

#[derive(Debug, Error)]
pub enum ParseError {
    #[error("YAML parse error: {0}")]
    YamlError(#[from] serde_yaml::Error),

    #[error("invalid version: expected '0.2', got '{0}'")]
    InvalidVersion(String),
}

/// USML ドキュメントを YAML 文字列からパースする
pub fn parse(input: &str) -> Result<UsmlDocument, ParseError> {
    let doc: UsmlDocument = serde_yaml::from_str(input)?;

    if doc.version != "0.2" {
        return Err(ParseError::InvalidVersion(doc.version));
    }

    Ok(doc)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::ColumnMapping;

    /// 最小ドキュメント: version 0.2 + domain.entities 1個 + usecase
    #[test]
    fn test_minimal_document() {
        let yaml = r#"
version: "0.2"
import:
  openapi: ./api.yaml#paths["/users"].get.responses["200"]
  dbml:
    - ./schema.dbml#tables["users"]
domain:
  entities:
    User:
      fields:
        id: integer
        name: string
      persistence:
        root_table: users
        columns:
          id: users.id
          name: users.name
usecase:
  name: ユーザー一覧取得
  root: User
  response_mapping:
    - field: id
      source: User.id
    - field: name
      source: User.name
"#;
        let doc = parse(yaml).expect("parse should succeed");
        assert_eq!(doc.version, "0.2");
        assert_eq!(doc.usecase.name, "ユーザー一覧取得");
        assert_eq!(doc.usecase.root, "User");

        // domain.entities が宣言順で1個
        assert_eq!(doc.domain.entities.len(), 1);
        let user = doc.domain.entities.get("User").expect("User entity");
        assert_eq!(user.persistence.root_table, "users");

        // fields は宣言順を保持
        let field_names: Vec<&String> = user.fields.keys().collect();
        assert_eq!(field_names, vec!["id", "name"]);
        assert_eq!(user.fields.get("id").map(String::as_str), Some("integer"));

        // response_mapping
        assert_eq!(doc.usecase.response_mapping.len(), 2);
        assert_eq!(doc.usecase.response_mapping[0].field, "id");
        assert_eq!(
            doc.usecase.response_mapping[0].source.as_deref(),
            Some("User.id")
        );
    }

    /// 旧バージョン "0.1" は v0.2 パーサーで拒否される
    #[test]
    fn test_rejects_v01() {
        let yaml = r#"
version: "0.1"
import: {}
domain:
  entities:
    User:
      persistence:
        root_table: users
usecase:
  name: test
  root: User
  response_mapping: []
"#;
        let result = parse(yaml);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ParseError::InvalidVersion(v) if v == "0.1"
        ));
    }

    /// 未知バージョン "9.9" もエラー
    #[test]
    fn test_invalid_version() {
        let yaml = r#"
version: "9.9"
import: {}
domain:
  entities:
    User:
      persistence:
        root_table: users
usecase:
  name: test
  root: User
  response_mapping: []
"#;
        let result = parse(yaml);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ParseError::InvalidVersion(v) if v == "9.9"
        ));
    }

    /// value_objects のパース（base のみ / format 付きの両方）
    #[test]
    fn test_value_objects() {
        let yaml = r#"
version: "0.2"
import: {}
domain:
  value_objects:
    - { name: UserId, base: integer }
    - { name: Url, base: string, format: uri }
    - { name: Email, base: string, format: email }
  entities:
    User:
      fields:
        id: UserId
      persistence:
        root_table: users
        columns:
          id: users.id
usecase:
  name: test
  root: User
  response_mapping:
    - { field: id, source: User.id }
"#;
        let doc = parse(yaml).expect("parse should succeed");
        let vos = &doc.domain.value_objects;
        assert_eq!(vos.len(), 3);

        assert_eq!(vos[0].name, "UserId");
        assert_eq!(vos[0].base, "integer");
        assert_eq!(vos[0].format, None);

        assert_eq!(vos[1].name, "Url");
        assert_eq!(vos[1].base, "string");
        assert_eq!(vos[1].format.as_deref(), Some("uri"));

        assert_eq!(vos[2].format.as_deref(), Some("email"));
    }

    /// persistence.columns: Simple 形式と Detailed 形式（join 付き）の混在判別
    #[test]
    fn test_column_mapping_simple_and_detailed() {
        let yaml = r#"
version: "0.2"
import: {}
domain:
  entities:
    User:
      fields:
        id: integer
        name: string
        avatarUrl: string
      persistence:
        root_table: users
        columns:
          id: users.id
          name: users.name
          avatarUrl:
            source: profiles.avatar_url
            join:
              table: profiles
              on: users.id = profiles.user_id
              type: LEFT JOIN
usecase:
  name: test
  root: User
  response_mapping:
    - { field: id, source: User.id }
"#;
        let doc = parse(yaml).expect("parse should succeed");
        let cols = &doc.domain.entities.get("User").unwrap().persistence.columns;

        // Simple: id / name
        match cols.get("id").expect("id column") {
            ColumnMapping::Simple(s) => assert_eq!(s, "users.id"),
            ColumnMapping::Detailed(_) => panic!("id should be Simple"),
        }
        match cols.get("name").expect("name column") {
            ColumnMapping::Simple(s) => assert_eq!(s, "users.name"),
            ColumnMapping::Detailed(_) => panic!("name should be Simple"),
        }

        // Detailed: avatarUrl（join 付き）
        match cols.get("avatarUrl").expect("avatarUrl column") {
            ColumnMapping::Detailed(d) => {
                assert_eq!(d.source, "profiles.avatar_url");
                let join = d.join.as_ref().expect("join should exist");
                assert_eq!(join.table, "profiles");
                assert_eq!(join.on, "users.id = profiles.user_id");
                assert_eq!(join.r#type.as_deref(), Some("LEFT JOIN"));
                assert_eq!(join.alias, None);
                assert!(d.aggregate.is_none());
                assert!(d.join_chain.is_none());
            }
            ColumnMapping::Simple(_) => panic!("avatarUrl should be Detailed"),
        }
    }

    /// Detailed カラムの aggregate（COUNT + group_by）
    #[test]
    fn test_column_mapping_with_aggregate() {
        let yaml = r#"
version: "0.2"
import: {}
domain:
  entities:
    Post:
      fields:
        id: integer
        likeCount: integer
      persistence:
        root_table: posts
        columns:
          id: posts.id
          likeCount:
            source: likes.id
            join: { table: likes, on: posts.id = likes.post_id }
            aggregate: { type: COUNT, group_by: posts.id }
usecase:
  name: test
  root: Post
  response_mapping:
    - { field: like_count, source: Post.likeCount }
"#;
        let doc = parse(yaml).expect("parse should succeed");
        let cols = &doc.domain.entities.get("Post").unwrap().persistence.columns;
        match cols.get("likeCount").expect("likeCount column") {
            ColumnMapping::Detailed(d) => {
                assert_eq!(d.source, "likes.id");
                let agg = d.aggregate.as_ref().expect("aggregate should exist");
                assert_eq!(agg.r#type, "COUNT");
                assert_eq!(agg.group_by.as_deref(), Some("posts.id"));
            }
            ColumnMapping::Simple(_) => panic!("likeCount should be Detailed"),
        }
    }

    /// derived: COALESCE（sources + fallback）
    #[test]
    fn test_derived_coalesce() {
        let yaml = r#"
version: "0.2"
import: {}
domain:
  entities:
    User:
      fields:
        displayName: string
      persistence:
        root_table: users
        columns:
          displayName: users.name
      derived:
        - field: displayName
          type: COALESCE
          sources: [profiles.display_name, users.name]
          fallback: "anonymous"
usecase:
  name: test
  root: User
  response_mapping:
    - { field: display_name, source: User.displayName }
"#;
        let doc = parse(yaml).expect("parse should succeed");
        let derived = &doc.domain.entities.get("User").unwrap().derived;
        assert_eq!(derived.len(), 1);
        assert_eq!(derived[0].field, "displayName");
        assert_eq!(derived[0].r#type, "COALESCE");
        assert_eq!(
            derived[0].sources.as_deref(),
            Some(
                [
                    "profiles.display_name".to_string(),
                    "users.name".to_string()
                ]
                .as_slice()
            )
        );
        assert_eq!(derived[0].fallback.as_deref(), Some("anonymous"));
    }

    /// derived: CONDITIONAL_SOURCE（when / then_source / else_source）
    #[test]
    fn test_derived_conditional_source() {
        let yaml = r#"
version: "0.2"
import: {}
domain:
  entities:
    Post:
      fields:
        bodyContent: string
      persistence:
        root_table: posts
        columns:
          bodyContent: posts.body
      derived:
        - field: bodyContent
          type: CONDITIONAL_SOURCE
          when:
            - { source: posts.status, operator: "==", value: "draft" }
          then_source: posts.preview_text
          else_source: posts.body
usecase:
  name: test
  root: Post
  response_mapping:
    - { field: body, source: Post.bodyContent }
"#;
        let doc = parse(yaml).expect("parse should succeed");
        let derived = &doc.domain.entities.get("Post").unwrap().derived[0];
        assert_eq!(derived.r#type, "CONDITIONAL_SOURCE");
        assert_eq!(derived.then_source.as_deref(), Some("posts.preview_text"));
        assert_eq!(derived.else_source.as_deref(), Some("posts.body"));
        let when = derived.when.as_ref().expect("when should exist");
        assert_eq!(when.len(), 1);
        assert_eq!(when[0].source.as_deref(), Some("posts.status"));
        assert_eq!(when[0].operator, "==");
        assert_eq!(when[0].value, "draft");
    }

    /// relations: has_many と many_to_many(through 付き)
    #[test]
    fn test_relations() {
        let yaml = r#"
version: "0.2"
import: {}
domain:
  entities:
    Post:
      persistence:
        root_table: posts
        columns:
          id: posts.id
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
usecase:
  name: test
  root: Post
  response_mapping:
    - { field: id, source: Post.id }
"#;
        let doc = parse(yaml).expect("parse should succeed");
        let relations = &doc.domain.entities.get("Post").unwrap().relations;
        assert_eq!(relations.len(), 2);

        // 宣言順を保持
        let rel_names: Vec<&String> = relations.keys().collect();
        assert_eq!(rel_names, vec!["comments", "tags"]);

        // has_many（through なし）
        let comments = relations.get("comments").unwrap();
        assert_eq!(comments.target, "Comment");
        assert_eq!(comments.kind, "has_many");
        assert_eq!(comments.on, "posts.id = comments.post_id");
        assert!(comments.through.is_none());

        // many_to_many（through あり）
        let tags = relations.get("tags").unwrap();
        assert_eq!(tags.target, "Tag");
        assert_eq!(tags.kind, "many_to_many");
        assert_eq!(tags.on, "post_tags.tag_id = tags.id");
        let through = tags.through.as_ref().expect("through should exist");
        assert_eq!(through.table, "post_tags");
        assert_eq!(through.on, "posts.id = post_tags.post_id");
    }

    /// usecase.response_mapping の配列フィールド（type: array + 入れ子 fields）
    #[test]
    fn test_response_mapping_array() {
        let yaml = r#"
version: "0.2"
import: {}
domain:
  entities:
    Post:
      persistence:
        root_table: posts
        columns:
          id: posts.id
usecase:
  name: test
  root: Post
  response_mapping:
    - { field: id, source: Post.id }
    - field: tags
      type: array
      source: Post.tags
      fields:
        - { field: id, source: Tag.id }
        - { field: name, source: Tag.name }
"#;
        let doc = parse(yaml).expect("parse should succeed");
        let tags = &doc.usecase.response_mapping[1];
        assert_eq!(tags.field, "tags");
        assert_eq!(tags.r#type.as_deref(), Some("array"));
        assert_eq!(tags.source.as_deref(), Some("Post.tags"));

        let nested = tags.fields.as_ref().expect("nested fields should exist");
        assert_eq!(nested.len(), 2);
        assert_eq!(nested[0].field, "id");
        assert_eq!(nested[0].source.as_deref(), Some("Tag.id"));
        assert_eq!(nested[1].field, "name");
        assert_eq!(nested[1].source.as_deref(), Some("Tag.name"));
    }

    /// presentation: MASK（mask_pattern + condition の when）と CASE（when/else）
    #[test]
    fn test_presentation_mask_and_case() {
        let yaml = r#"
version: "0.2"
import: {}
domain:
  entities:
    User:
      persistence:
        root_table: users
        columns:
          id: users.id
usecase:
  name: test
  root: User
  response_mapping:
    - { field: email, source: User.email }
    - { field: status_label, source: User.status }
  presentation:
    - target: email
      type: MASK
      mask_pattern: "***@***.***"
      condition:
        - { param: viewer_role, operator: "!=", value: "admin" }
    - target: status_label
      type: CASE
      source: User.status
      when:
        - { value: "active", then: "有効" }
        - { value: "inactive", then: "無効" }
      else: "不明"
"#;
        let doc = parse(yaml).expect("parse should succeed");
        let presentations = &doc.usecase.presentation;
        assert_eq!(presentations.len(), 2);

        // MASK
        let mask = &presentations[0];
        assert_eq!(mask.target, "email");
        assert_eq!(mask.r#type, "MASK");
        assert_eq!(mask.mask_pattern.as_deref(), Some("***@***.***"));
        let cond = mask.condition.as_ref().expect("condition should exist");
        assert_eq!(cond.len(), 1);
        assert_eq!(cond[0].param.as_deref(), Some("viewer_role"));
        assert_eq!(cond[0].operator, "!=");
        assert_eq!(cond[0].value, "admin");

        // CASE
        let case = &presentations[1];
        assert_eq!(case.target, "status_label");
        assert_eq!(case.r#type, "CASE");
        assert_eq!(case.source.as_deref(), Some("User.status"));
        let when = case.when.as_ref().expect("when should exist");
        assert_eq!(when.len(), 2);
        assert_eq!(when[0].value, "active");
        assert_eq!(when[0].then, "有効");
        assert_eq!(when[1].value, "inactive");
        assert_eq!(when[1].then, "無効");
        assert_eq!(case.else_value.as_deref(), Some("不明"));
    }

    /// 仕様書 §6.2 の完全サンプル（多対多・集約・alias・派生・入れ子配列を一括検証）
    #[test]
    fn test_full_sample_post_detail() {
        let yaml = r#"
version: "0.2"
import:
  openapi: ./api.yaml#paths["/posts/{post_id}"].get.responses["200"]
  dbml:
    - ./schema.dbml#tables["posts"]
    - ./schema.dbml#tables["post_tags"]
domain:
  value_objects:
    - { name: PostId, base: integer }
  entities:
    Post:
      fields:
        id: PostId
        bodyContent: string
        authorName: string
        likeCount: integer
      persistence:
        root_table: posts
        columns:
          id: posts.id
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
        id: integer
        authorName: string
      persistence:
        root_table: comments
        columns:
          id: comments.id
          authorName:
            source: comment_author.name
            join: { table: users, alias: comment_author, on: comments.user_id = users.id }
    Tag:
      persistence:
        root_table: tags
        columns:
          id: tags.id
          name: tags.name
usecase:
  name: 投稿詳細取得
  summary: 投稿本文・著者・コメント・いいねCount・タグを返す
  root: Post
  response_mapping:
    - { field: id, source: Post.id }
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
        - { field: author_name, source: Comment.authorName }
  filters:
    - { param: post_id, maps_to: WHERE, condition: "Post.id = :post_id" }
"#;
        let doc = parse(yaml).expect("parse should succeed");

        // 3エンティティが宣言順
        let entity_names: Vec<&String> = doc.domain.entities.keys().collect();
        assert_eq!(entity_names, vec!["Post", "Comment", "Tag"]);

        // Comment の alias 付き join
        let comment_cols = &doc
            .domain
            .entities
            .get("Comment")
            .unwrap()
            .persistence
            .columns;
        match comment_cols.get("authorName").unwrap() {
            ColumnMapping::Detailed(d) => {
                let join = d.join.as_ref().unwrap();
                assert_eq!(join.alias.as_deref(), Some("comment_author"));
            }
            ColumnMapping::Simple(_) => panic!("Comment.authorName should be Detailed"),
        }

        // 入れ子配列が2つ
        let arrays: Vec<&str> = doc
            .usecase
            .response_mapping
            .iter()
            .filter(|m| m.r#type.as_deref() == Some("array"))
            .map(|m| m.field.as_str())
            .collect();
        assert_eq!(arrays, vec!["tags", "comments"]);
    }
}
