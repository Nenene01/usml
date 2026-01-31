use std::fs;

use super::{OpenapiResponse, ResolverError};

pub fn resolve_openapi(
    file_path: &str,
    path: &str,
    method: &str,
    status_code: &str,
) -> Result<OpenapiResponse, ResolverError> {
    let content = fs::read_to_string(file_path)
        .map_err(|e| ResolverError::IoError(file_path.to_string(), e))?;

    parse_openapi_content(&content, file_path, path, method, status_code)
}

pub fn parse_openapi_content(
    content: &str,
    source: &str,
    path: &str,
    method: &str,
    status_code: &str,
) -> Result<OpenapiResponse, ResolverError> {
    let spec: openapi3_parser::open_api::OpenApiSpec = serde_yaml::from_str(content)
        .map_err(|e| ResolverError::OpenapiParseError(source.to_string(), format!("{}", e)))?;

    let paths = spec.paths.as_ref().ok_or_else(|| {
        ResolverError::NotFound("OpenAPI に paths が定義されていません".to_string())
    })?;

    let path_item = paths
        .get(path)
        .ok_or_else(|| ResolverError::NotFound(format!("パス {} が見つかりません", path)))?;

    let operation = match method {
        "get" => &path_item.get,
        "post" => &path_item.post,
        "put" => &path_item.put,
        "delete" => &path_item.delete,
        "patch" => &path_item.patch,
        _ => {
            return Err(ResolverError::NotFound(format!(
                "メソッド {} は未対応です",
                method
            )));
        }
    }
    .as_ref()
    .ok_or_else(|| {
        ResolverError::NotFound(format!(
            "パス {} に メソッド {} が定義されていません",
            path, method
        ))
    })?;

    let parameters: Vec<String> = operation
        .parameters
        .as_ref()
        .map(|params| params.iter().filter_map(|p| p.name.clone()).collect())
        .unwrap_or_default();

    let responses = operation.responses.as_ref().ok_or_else(|| {
        ResolverError::NotFound(format!(
            "パス {} .{} に responses が定義されていません",
            path, method
        ))
    })?;

    let response_map = responses.responses.as_ref().ok_or_else(|| {
        ResolverError::NotFound(format!(
            "パス {} .{} に レスポンスが定義されていません",
            path, method
        ))
    })?;

    let response = response_map.get(status_code).ok_or_else(|| {
        ResolverError::NotFound(format!(
            "パス {} .{} のレスポンス {} が見つかりません",
            path, method, status_code
        ))
    })?;

    let fields = extract_response_fields(response);

    Ok(OpenapiResponse { fields, parameters })
}

fn extract_response_fields(response: &openapi3_parser::open_api::Response) -> Vec<String> {
    if let Some(content) = &response.content
        && let Some(media_type) = content.get("application/json")
        && let Some(schema) = &media_type.schema
    {
        return extract_fields_from_schema(schema);
    }
    Vec::new()
}

fn extract_fields_from_schema(schema: &openapi3_parser::open_api::Schema) -> Vec<String> {
    if let Some(type_str) = &schema.type_
        && type_str == "object"
        && let Some(props) = &schema.properties
    {
        return props.keys().cloned().collect();
    }
    Vec::new()
}

pub fn parse_openapi_ref(reference: &str) -> Option<(&str, &str, &str, &str)> {
    let (path, fragment) = reference.split_once('#')?;
    let without_paths = fragment.strip_prefix("paths[\"")?;
    let (api_path, rest) = without_paths.split_once("\"].")?;
    let (method, rest) = rest.split_once(".responses[\"")?;
    let status_code = rest.strip_suffix("\"]")?;
    Some((path, api_path, method, status_code))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_openapi_ref() {
        let (file, path, method, status) =
            parse_openapi_ref("./api.yaml#paths[\"/users\"].get.responses[\"200\"]").unwrap();
        assert_eq!(file, "./api.yaml");
        assert_eq!(path, "/users");
        assert_eq!(method, "get");
        assert_eq!(status, "200");
    }

    #[test]
    fn test_parse_openapi_ref_with_path_param() {
        let (file, path, method, status) =
            parse_openapi_ref("./api.yaml#paths[\"/posts/{post_id}\"].get.responses[\"200\"]")
                .unwrap();
        assert_eq!(file, "./api.yaml");
        assert_eq!(path, "/posts/{post_id}");
        assert_eq!(method, "get");
        assert_eq!(status, "200");
    }

    #[test]
    fn test_parse_openapi_ref_invalid() {
        assert!(parse_openapi_ref("invalid").is_none());
        assert!(parse_openapi_ref("./api.yaml").is_none());
        assert!(parse_openapi_ref("./api.yaml#paths[\"/users\"].get").is_none());
    }

    #[test]
    fn test_parse_openapi_content_basic() {
        let yaml = r#"
openapi: "3.0.0"
info:
  title: Test API
  version: "1.0"
paths:
  /users:
    get:
      summary: Test
      parameters:
        - name: status
          in: query
          schema:
            type: string
        - name: page
          in: query
          schema:
            type: integer
      responses:
        "200":
          description: OK
          content:
            application/json:
              schema:
                type: object
                properties:
                  id:
                    type: integer
                  name:
                    type: string
                  email:
                    type: string
"#;
        let result = parse_openapi_content(yaml, "test.yaml", "/users", "get", "200").unwrap();
        assert_eq!(result.parameters.len(), 2);
        assert!(result.parameters.contains(&"status".to_string()));
        assert!(result.parameters.contains(&"page".to_string()));
        assert_eq!(result.fields.len(), 3);
        assert!(result.fields.contains(&"id".to_string()));
        assert!(result.fields.contains(&"name".to_string()));
        assert!(result.fields.contains(&"email".to_string()));
    }

    #[test]
    fn test_parse_openapi_content_path_not_found() {
        let yaml = r#"
openapi: "3.0.0"
info:
  title: Test API
  version: "1.0"
paths:
  /users:
    get:
      responses:
        "200":
          description: OK
"#;
        let result = parse_openapi_content(yaml, "test.yaml", "/posts", "get", "200");
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ResolverError::NotFound(_)));
    }
}
