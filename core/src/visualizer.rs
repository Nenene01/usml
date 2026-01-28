use std::collections::{HashMap, HashSet};
use std::fmt::Write;

use crate::ast::{ResponseMapping, Transform, UsmlDocument};

struct FieldEntry {
    field: String,
    badges: Vec<String>,
    join_lines: Vec<String>,
    transforms: Vec<String>,
    depth: usize,
}

pub fn generate_html(doc: &UsmlDocument) -> String {
    let transform_map = build_transform_map(&doc.usecase.transforms);
    let mut table_order = extract_import_tables(doc);
    let mut table_seen: HashSet<String> = table_order.iter().cloned().collect();
    let mut table_counts: HashMap<String, usize> = table_order
        .iter()
        .cloned()
        .map(|table| (table, 0))
        .collect();
    let mut entries = Vec::new();

    collect_entries(
        &doc.usecase.response_mapping,
        0,
        &transform_map,
        &mut entries,
        &mut table_counts,
        &mut table_order,
        &mut table_seen,
    );

    let mut html = String::new();
    html.push_str("<!DOCTYPE html>\n<html>\n<head>\n<meta charset=\"utf-8\">\n");
    html.push_str("<title>USML Data Flow Visualizer</title>\n");
    html.push_str("<style>\n");
    html.push_str(
        "body { font-family: 'Inter', 'Helvetica Neue', Arial, sans-serif; background: #f5f7fa; color: #1f2a37; margin: 0; padding: 24px; }\n",
    );
    html.push_str(".container { max-width: 1200px; margin: 0 auto; }\n");
    html.push_str("h1 { margin-bottom: 8px; font-size: 1.8rem; }\n");
    html.push_str(".summary { margin-top: 0; color: #556070; }\n");
    html.push_str(
        ".grid { display: grid; grid-template-columns: repeat(3, 1fr); gap: 16px; align-items: start; }\n",
    );
    html.push_str(".column h2 { font-size: 1.1rem; margin-bottom: 12px; }\n");
    html.push_str(
        ".card { border-radius: 12px; padding: 12px 16px; margin-bottom: 12px; box-shadow: 0 4px 12px rgba(15, 23, 42, 0.08); }\n",
    );
    html.push_str(".response-card { background: #e8f4fd; }\n");
    html.push_str(".join-card { background: #fff8e1; }\n");
    html.push_str(".table-card { background: #f0faf0; }\n");
    html.push_str(
        ".badge { display: inline-block; background: #6c757d; color: #fff; border-radius: 999px; font-size: 0.72rem; padding: 2px 8px; margin-right: 4px; }\n",
    );
    html.push_str(".field-name { font-weight: 600; margin-bottom: 6px; }\n");
    html.push_str(".field-name.small { font-weight: 500; font-size: 0.9rem; color: #394150; }\n");
    html.push_str(".join-line, .transform-line { font-size: 0.9rem; margin-top: 4px; }\n");
    html.push_str(".empty { color: #6b7280; font-size: 0.9rem; }\n");
    html.push_str(".depth-1 { margin-left: 12px; }\n");
    html.push_str(".depth-2 { margin-left: 24px; }\n");
    html.push_str(".depth-3 { margin-left: 36px; }\n");
    html.push_str(".depth-4 { margin-left: 48px; }\n");
    html.push_str("</style>\n</head>\n<body>\n");

    write!(
        &mut html,
        "<div class=\"container\"><h1>{}</h1>",
        escape_html(&doc.usecase.name)
    )
    .unwrap();
    if let Some(summary) = &doc.usecase.summary {
        write!(
            &mut html,
            "<p class=\"summary\">{}</p>",
            escape_html(summary)
        )
        .unwrap();
    }
    html.push_str("<div class=\"grid\">\n");

    html.push_str("<div class=\"column\">\n<h2>Response Fields</h2>\n");
    if entries.is_empty() {
        html.push_str("<div class=\"empty\">No response mappings.</div>");
    } else {
        for entry in &entries {
            let depth_class = depth_class(entry.depth);
            write!(
                &mut html,
                "<div class=\"card response-card{}\">",
                depth_class
            )
            .unwrap();
            write!(
                &mut html,
                "<div class=\"field-name\">{}</div>",
                escape_html(&entry.field)
            )
            .unwrap();
            if !entry.badges.is_empty() {
                html.push_str("<div>");
                for badge in &entry.badges {
                    write!(
                        &mut html,
                        "<span class=\"badge\">{}</span>",
                        escape_html(badge)
                    )
                    .unwrap();
                }
                html.push_str("</div>");
            }
            html.push_str("</div>\n");
        }
    }
    html.push_str("</div>\n");

    html.push_str("<div class=\"column\">\n<h2>Joins &amp; Transforms</h2>\n");
    if entries.is_empty() {
        html.push_str("<div class=\"empty\">No joins or transforms.</div>");
    } else {
        for entry in &entries {
            let depth_class = depth_class(entry.depth);
            write!(
                &mut html,
                "<div class=\"card join-card{}\">",
                depth_class
            )
            .unwrap();
            write!(
                &mut html,
                "<div class=\"field-name small\">{}</div>",
                escape_html(&entry.field)
            )
            .unwrap();
            if entry.join_lines.is_empty() && entry.transforms.is_empty() {
                html.push_str("<div class=\"empty\">No join/transform.</div>");
            } else {
                for join_line in &entry.join_lines {
                    write!(
                        &mut html,
                        "<div class=\"join-line\">{}</div>",
                        escape_html(join_line)
                    )
                    .unwrap();
                }
                if !entry.transforms.is_empty() {
                    html.push_str("<div class=\"transform-line\">Transforms:</div>");
                    html.push_str("<div>");
                    for transform in &entry.transforms {
                        write!(
                            &mut html,
                            "<span class=\"badge\">{}</span>",
                            escape_html(transform)
                        )
                        .unwrap();
                    }
                    html.push_str("</div>");
                }
            }
            html.push_str("</div>\n");
        }
    }
    html.push_str("</div>\n");

    html.push_str("<div class=\"column\">\n<h2>Tables</h2>\n");
    if table_order.is_empty() {
        html.push_str("<div class=\"empty\">No tables imported.</div>");
    } else {
        for table in &table_order {
            let count = table_counts.get(table).copied().unwrap_or(0);
            write!(
                &mut html,
                "<div class=\"card table-card\"><div class=\"field-name\">{}</div><div class=\"join-line\">Referenced by {} field{}</div></div>\n",
                escape_html(table),
                count,
                if count == 1 { "" } else { "s" }
            )
            .unwrap();
        }
    }
    html.push_str("</div>\n</div>\n</div>\n</body>\n</html>\n");
    html
}

fn build_transform_map(transforms: &[Transform]) -> HashMap<String, Vec<String>> {
    let mut map = HashMap::new();
    for transform in transforms {
        map.entry(transform.target.clone())
            .or_insert_with(Vec::new)
            .push(transform.r#type.clone());
    }
    map
}

fn extract_import_tables(doc: &UsmlDocument) -> Vec<String> {
    let mut tables = Vec::new();
    let mut seen = HashSet::new();
    if let Some(dbmls) = &doc.import.dbml {
        for entry in dbmls {
            if let Some(table) = extract_table_name(entry) {
                if seen.insert(table.clone()) {
                    tables.push(table);
                }
            }
        }
    }
    tables
}

fn extract_table_name(value: &str) -> Option<String> {
    let marker = "#tables[\"";
    let start = value.find(marker)? + marker.len();
    let remainder = &value[start..];
    let end = remainder.find("\"]")?;
    Some(remainder[..end].to_string())
}

fn extract_table_identifier(value: &str) -> Option<String> {
    let token = value.split_whitespace().next()?;
    let table = token.split('.').next()?;
    if table.is_empty() {
        None
    } else {
        Some(table.to_string())
    }
}

fn collect_entries(
    mappings: &[ResponseMapping],
    depth: usize,
    transform_map: &HashMap<String, Vec<String>>,
    entries: &mut Vec<FieldEntry>,
    table_counts: &mut HashMap<String, usize>,
    table_order: &mut Vec<String>,
    table_seen: &mut HashSet<String>,
) {
    for mapping in mappings {
        let mut badges = Vec::new();
        if let Some(aggregate) = &mapping.aggregate {
            badges.push(aggregate.r#type.clone());
        }
        if mapping.r#type.as_deref() == Some("array") {
            badges.push("array".to_string());
        }

        let mut join_lines = Vec::new();
        if let Some(join) = &mapping.join {
            let join_type = join.r#type.as_deref().unwrap_or("JOIN");
            let mut line = format!("{} {} ON {}", join_type, join.table, join.on);
            if let Some(alias) = &join.alias {
                line = format!("{} AS {}", line, alias);
            }
            join_lines.push(line);
        }
        if let Some(chain) = &mapping.join_chain {
            if !chain.is_empty() {
                let chain_line = chain
                    .iter()
                    .map(|entry| format!("JOIN {} ON {}", entry.table, entry.on))
                    .collect::<Vec<_>>()
                    .join(" → ");
                join_lines.push(chain_line);
            }
        }

        let transforms = transform_map
            .get(&mapping.field)
            .cloned()
            .unwrap_or_default();

        entries.push(FieldEntry {
            field: mapping.field.clone(),
            badges,
            join_lines,
            transforms,
            depth,
        });

        let mut tables_for_field = HashSet::new();
        if let Some(source) = &mapping.source {
            if let Some(table) = extract_table_identifier(source) {
                tables_for_field.insert(table);
            }
        }
        if let Some(source_table) = &mapping.source_table {
            if let Some(table) = extract_table_identifier(source_table) {
                tables_for_field.insert(table);
            }
        }
        if let Some(join) = &mapping.join {
            if let Some(table) = extract_table_identifier(&join.table) {
                tables_for_field.insert(table);
            }
        }
        if let Some(chain) = &mapping.join_chain {
            for entry in chain {
                if let Some(table) = extract_table_identifier(&entry.table) {
                    tables_for_field.insert(table);
                }
            }
        }

        for table in tables_for_field {
            let count = table_counts.entry(table.clone()).or_insert(0);
            *count += 1;
            if table_seen.insert(table.clone()) {
                table_order.push(table);
            }
        }

        if let Some(fields) = &mapping.fields {
            collect_entries(
                fields,
                depth + 1,
                transform_map,
                entries,
                table_counts,
                table_order,
                table_seen,
            );
        }
    }
}

fn depth_class(depth: usize) -> String {
    if depth == 0 {
        String::new()
    } else {
        format!(" depth-{}", depth.min(4))
    }
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{Aggregate, Import, Join, ResponseMapping, Transform, Usecase, UsmlDocument};

    #[test]
    fn test_generate_html_contains_sections() {
        let doc = UsmlDocument {
            version: "0.1".to_string(),
            import: Import {
                openapi: None,
                dbml: Some(vec!["./schema.dbml#tables[\"users\"]".to_string()]),
            },
            usecase: Usecase {
                name: "Users".to_string(),
                summary: None,
                response_mapping: vec![ResponseMapping {
                    field: "id".to_string(),
                    source: Some("users.id".to_string()),
                    r#type: None,
                    source_table: None,
                    join: None,
                    join_chain: None,
                    aggregate: None,
                    fields: None,
                }],
                filters: Vec::new(),
                transforms: Vec::new(),
            },
        };

        let html = generate_html(&doc);
        assert!(html.contains("Response Fields"));
        assert!(html.contains("Joins &amp; Transforms"));
        assert!(html.contains("Tables"));
    }

    #[test]
    fn test_generate_html_includes_join_and_badges() {
        let doc = UsmlDocument {
            version: "0.1".to_string(),
            import: Import {
                openapi: None,
                dbml: Some(vec![
                    "./schema.dbml#tables[\"users\"]".to_string(),
                    "./schema.dbml#tables[\"profiles\"]".to_string(),
                ]),
            },
            usecase: Usecase {
                name: "Profiles".to_string(),
                summary: None,
                response_mapping: vec![ResponseMapping {
                    field: "profile_count".to_string(),
                    source: Some("profiles.id".to_string()),
                    r#type: Some("array".to_string()),
                    source_table: None,
                    join: Some(Join {
                        table: "profiles".to_string(),
                        on: "users.id = profiles.user_id".to_string(),
                        r#type: Some("LEFT JOIN".to_string()),
                        alias: None,
                    }),
                    join_chain: None,
                    aggregate: Some(Aggregate {
                        r#type: "COUNT".to_string(),
                        group_by: None,
                    }),
                    fields: None,
                }],
                filters: Vec::new(),
                transforms: vec![Transform {
                    target: "profile_count".to_string(),
                    r#type: "COALESCE".to_string(),
                    source: None,
                    sources: None,
                    fallback: None,
                    separator: None,
                    when: None,
                    else_value: None,
                    mask_pattern: None,
                    condition: None,
                    then_source: None,
                    else_source: None,
                }],
            },
        };

        let html = generate_html(&doc);
        assert!(html.contains("LEFT JOIN profiles ON users.id = profiles.user_id"));
        assert!(html.contains("COUNT"));
        assert!(html.contains("array"));
        assert!(html.contains("COALESCE"));
        assert!(html.contains("profiles"));
    }
}
