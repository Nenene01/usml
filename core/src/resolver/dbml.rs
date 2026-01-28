use std::fs;

use super::{DbmlTable, ResolverError};

/// DBML ファイルを読み込み、テーブル・カラム情報を抽出する
pub fn resolve_dbml(file_path: &str) -> Result<Vec<DbmlTable>, ResolverError> {
    let content =
        fs::read_to_string(file_path).map_err(|e| ResolverError::IoError(file_path.to_string(), e))?;

    parse_dbml_content(&content, file_path)
}

/// DBML 文字列をパースしてテーブル情報を抽出する
pub fn parse_dbml_content(content: &str, source: &str) -> Result<Vec<DbmlTable>, ResolverError> {
    let ast = dbml_rs::parse_dbml(content)
        .map_err(|e| ResolverError::DbmlParseError(source.to_string(), format!("{:?}", e)))?;

    let mut tables = Vec::new();

    for table in ast.tables() {
        let columns: Vec<String> = table.cols.iter().map(|c| c.name.to_string.clone()).collect();
        tables.push(DbmlTable {
            name: table.ident.name.to_string.clone(),
            columns,
        });
    }

    Ok(tables)
}

/// DBML import 参照文字列から対象テーブル名を抽出する
/// 例: `./schema.dbml#tables["users"]` → `("./schema.dbml", "users")`
pub fn parse_dbml_ref(reference: &str) -> Option<(&str, &str)> {
    let (path, fragment) = reference.split_once('#')?;
    let table_name = fragment
        .strip_prefix("tables[\"")?
        .strip_suffix("\"]")?;
    Some((path, table_name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_dbml_ref() {
        let (path, table) = parse_dbml_ref("./schema.dbml#tables[\"users\"]").unwrap();
        assert_eq!(path, "./schema.dbml");
        assert_eq!(table, "users");
    }

    #[test]
    fn test_parse_dbml_ref_nested_path() {
        let (path, table) =
            parse_dbml_ref("../shared/db.dbml#tables[\"post_tags\"]").unwrap();
        assert_eq!(path, "../shared/db.dbml");
        assert_eq!(table, "post_tags");
    }

    #[test]
    fn test_parse_dbml_ref_invalid() {
        assert!(parse_dbml_ref("invalid_string").is_none());
        assert!(parse_dbml_ref("./schema.dbml").is_none());
        assert!(parse_dbml_ref("./schema.dbml#columns[\"id\"]").is_none());
    }

    #[test]
    fn test_parse_dbml_content_basic() {
        let dbml = r#"
Project test_db {
  database_type: 'PostgreSQL'
}

Table users {
    id integer [pk, increment]
    name varchar [not null]
    email varchar [unique, not null]
    created_at timestamp [default: `now()`]
}

Table profiles {
    id integer [pk, increment]
    user_id integer [ref: > users.id]
    avatar_url varchar
    bio text
}
"#;
        let tables = parse_dbml_content(dbml, "test.dbml").expect("パースに失敗しました");
        assert_eq!(tables.len(), 2);

        let users = tables.iter().find(|t| t.name == "users").unwrap();
        assert_eq!(users.columns.len(), 4);
        assert!(users.columns.contains(&"id".to_string()));
        assert!(users.columns.contains(&"name".to_string()));
        assert!(users.columns.contains(&"email".to_string()));
        assert!(users.columns.contains(&"created_at".to_string()));

        let profiles = tables.iter().find(|t| t.name == "profiles").unwrap();
        assert_eq!(profiles.columns.len(), 4);
        assert!(profiles.columns.contains(&"user_id".to_string()));
        assert!(profiles.columns.contains(&"avatar_url".to_string()));
    }

    #[test]
    fn test_parse_dbml_content_with_relations() {
        let dbml = r#"
Project test_db {
  database_type: 'PostgreSQL'
}

Table users {
    id integer [pk, increment]
    name varchar [not null]
    email varchar [unique, not null]
}

Table posts {
    id integer [pk, increment]
    user_id integer [ref: > users.id]
    title varchar [not null]
    body text
    status varchar(255) [default: 'draft']
}

Table comments {
    id integer [pk, increment]
    post_id integer [ref: > posts.id]
    user_id integer [ref: > users.id]
    body text [not null]
}

Table likes {
    id integer [pk, increment]
    post_id integer [ref: > posts.id]
    user_id integer [ref: > users.id]
}
"#;
        let tables = parse_dbml_content(dbml, "test.dbml").expect("パースに失敗しました");
        assert_eq!(tables.len(), 4);

        let posts = tables.iter().find(|t| t.name == "posts").unwrap();
        assert!(posts.columns.contains(&"status".to_string()));

        let comments = tables.iter().find(|t| t.name == "comments").unwrap();
        assert!(comments.columns.contains(&"post_id".to_string()));
        assert!(comments.columns.contains(&"user_id".to_string()));
    }
}
