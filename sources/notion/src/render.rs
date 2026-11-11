//! Pure, deterministic normalization. Unknown block types retain text and children.
use crate::{
    config::{rich, title},
    memcastle::source::types::{CanonicalDocument, RawDocument, Segment, SourceKind},
};
use serde_json::{Value, json};

pub(crate) fn revision_value(value: &Value) -> Value {
    let mut value = value.clone();
    stable_files(&mut value);
    value
}

fn stable_files(value: &mut Value) {
    match value {
        Value::Object(object) => {
            if object.get("type").and_then(Value::as_str) == Some("file")
                && let Some(file) = object.get_mut("file").and_then(Value::as_object_mut)
            {
                file.remove("expiry_time");
                if let Some(url) = file.get_mut("url")
                    && let Some(text) = url.as_str()
                {
                    *url = json!(text.split('?').next().unwrap_or(text));
                }
            }
            for value in object.values_mut() {
                stable_files(value);
            }
        }
        Value::Array(values) => {
            for value in values {
                stable_files(value);
            }
        }
        _ => {}
    }
}

pub(crate) fn provenance(document: &Value) -> Value {
    let document = revision_value(document);
    let blocks = document["blocks"]
        .as_array()
        .map(|blocks| blocks.iter().map(block_metadata).collect::<Vec<_>>())
        .unwrap_or_default();
    revision_value(
        &json!({"format":1,"lifecycle":"live","page":document["page"],"hierarchy":document["hierarchy"],"blocks":blocks,"selection":"included"}),
    )
}

fn block_metadata(block: &Value) -> Value {
    let kind = block["type"].as_str().unwrap_or("unknown");
    let payload = &block[kind];
    let children = block["children"]
        .as_array()
        .map(|v| v.iter().map(block_metadata).collect::<Vec<_>>())
        .unwrap_or_default();
    json!({"id":block["id"],"type":kind,"parent":block["parent"],"created_time":block["created_time"],"last_edited_time":block["last_edited_time"],"name":payload["name"],"source":payload.get("file").or_else(|| payload.get("external")),"references":references(payload),"children":children})
}

fn references(value: &Value) -> Vec<Value> {
    match value {
        Value::Object(object) => {
            let mut result = Vec::new();
            for key in [
                "mention",
                "link",
                "href",
                "url",
                "page_id",
                "database_id",
                "synced_from",
            ] {
                if let Some(value) = object.get(key) {
                    result.push(json!({"kind":key,"target":value}));
                }
            }
            for value in object.values() {
                result.extend(references(value));
            }
            result
        }
        Value::Array(values) => values.iter().flat_map(references).collect(),
        _ => vec![],
    }
}

fn text(value: &Value) -> String {
    value
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    let content = item
                        .get("plain_text")
                        .and_then(Value::as_str)
                        .or_else(|| item.pointer("/text/content").and_then(Value::as_str))
                        .map(str::to_string)
                        .unwrap_or_else(|| {
                            if let Some(id) =
                                item.pointer("/mention/page/id").and_then(Value::as_str)
                            {
                                format!("https://www.notion.so/{}", id.replace('-', ""))
                            } else if let Some(expression) =
                                item.pointer("/equation/expression").and_then(Value::as_str)
                            {
                                expression.into()
                            } else {
                                String::new()
                            }
                        });
                    let href = item
                        .get("href")
                        .and_then(Value::as_str)
                        .or_else(|| item.pointer("/text/link/url").and_then(Value::as_str));
                    match href {
                        Some(href) if href != content => format!("[{content}]({href})"),
                        _ => content,
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

fn block_text(block: &Value, depth: usize, output: &mut String) {
    let kind = block["type"].as_str().unwrap_or("unknown");
    let p = &block[kind];
    let value = text(&p["rich_text"]);
    let line = match kind {
        "heading_1" => format!("# {value}"),
        "heading_2" => format!("## {value}"),
        "heading_3" => format!("### {value}"),
        "bulleted_list_item" => format!("- {value}"),
        "numbered_list_item" => format!("1. {value}"),
        "to_do" => format!(
            "- [{}] {value}",
            if p["checked"] == true { "x" } else { " " }
        ),
        "code" => format!("```{}\n{value}\n```", p["language"].as_str().unwrap_or("")),
        "quote" => format!("> {value}"),
        "divider" => "---".into(),
        "equation" => p["expression"].as_str().unwrap_or("").into(),
        "child_page" | "child_database" => format!(
            "[{}](https://www.notion.so/{})",
            p["title"].as_str().unwrap_or("Untitled"),
            block["id"].as_str().unwrap_or("").replace('-', "")
        ),
        "link_to_page" => format!(
            "https://www.notion.so/{}",
            p.get(p["type"].as_str().unwrap_or(""))
                .and_then(Value::as_str)
                .unwrap_or("")
                .replace('-', "")
        ),
        "table_row" => p["cells"]
            .as_array()
            .map(|cells| {
                format!(
                    "| {} |",
                    cells.iter().map(text).collect::<Vec<_>>().join(" | ")
                )
            })
            .unwrap_or_default(),
        "image" | "file" | "pdf" | "audio" | "video" => {
            let url = p
                .pointer("/external/url")
                .or_else(|| p.pointer("/file/url"))
                .and_then(Value::as_str)
                .unwrap_or("");
            // Do not index ephemeral authorization query strings.
            let url = if p["type"] == "file" {
                url.split('?').next().unwrap_or(url)
            } else {
                url
            };
            format!(
                "{}: {} ({url})",
                kind,
                p["name"].as_str().unwrap_or("attachment")
            )
        }
        "bookmark" | "embed" | "link_preview" => p["url"].as_str().unwrap_or("").into(),
        _ => value,
    };
    if !line.is_empty() {
        output.push_str(&"  ".repeat(depth));
        output.push_str(&line);
        output.push('\n');
    }
    let caption = text(&p["caption"]);
    if !caption.is_empty() {
        output.push_str(&caption);
        output.push('\n');
    }
    if let Some(children) = block["children"].as_array() {
        for child in children {
            block_text(child, depth + 1, output);
        }
    }
}

pub(crate) fn normalize(raw: RawDocument) -> Result<CanonicalDocument, String> {
    let metadata: Value =
        serde_json::from_str(&raw.metadata).map_err(|_| "invalid Notion metadata")?;
    let tombstone = metadata["lifecycle"] == "tombstone";
    let mut tags = Vec::new();
    let mut body = String::new();
    let (name, uri) = if tombstone {
        (None, None)
    } else {
        let document: Value =
            serde_json::from_str(&raw.body).map_err(|_| "invalid Notion document")?;
        let page = &document["page"];
        let name = title(page);
        body.push_str(&name);
        body.push('\n');
        if let Some(properties) = page["properties"].as_object() {
            for (key, property) in properties {
                let kind = property["type"].as_str().unwrap_or("");
                let value = &property[kind];
                let summary = match kind {
                    "title" => continue,
                    "rich_text" => rich(value),
                    "status" | "select" => value["name"].as_str().unwrap_or("").into(),
                    "multi_select" => value
                        .as_array()
                        .map(|items| {
                            items
                                .iter()
                                .filter_map(|v| v["name"].as_str())
                                .inspect(|v| {
                                    tags.push((*v).to_string());
                                })
                                .collect::<Vec<_>>()
                                .join(", ")
                        })
                        .unwrap_or_default(),
                    "url" | "email" | "phone_number" => value.as_str().unwrap_or("").into(),
                    "number" | "checkbox" | "date" => {
                        if value.is_null() {
                            String::new()
                        } else {
                            value.to_string()
                        }
                    }
                    "relation" => value
                        .as_array()
                        .map(|v| {
                            v.iter()
                                .filter_map(|v| v["id"].as_str())
                                .map(|id| format!("https://www.notion.so/{}", id.replace('-', "")))
                                .collect::<Vec<_>>()
                                .join(", ")
                        })
                        .unwrap_or_default(),
                    _ => String::new(),
                };
                if !summary.is_empty() {
                    body.push_str(&format!("{key}: {summary}\n"));
                }
            }
        }
        if let Some(blocks) = document["blocks"].as_array() {
            for block in blocks {
                block_text(block, 0, &mut body);
            }
        }
        (Some(name), page["url"].as_str().map(str::to_string))
    };
    tags.sort();
    tags.dedup();
    Ok(CanonicalDocument {
        title: name,
        room: None,
        name: None,
        kind: SourceKind::Other,
        uri,
        tags,
        segments: if tombstone {
            vec![]
        } else {
            vec![Segment { text: body }]
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn normalizes_page_properties_provenance_and_supported_blocks() {
        let rich = |s: &str| json!([{"plain_text":s}]);
        let mut blocks = Vec::new();
        for kind in [
            "heading_1",
            "heading_2",
            "heading_3",
            "paragraph",
            "bulleted_list_item",
            "numbered_list_item",
            "to_do",
            "quote",
            "callout",
            "toggle",
            "code",
            "future",
        ] {
            blocks.push(json!({"id":kind,"type":kind,kind:{"rich_text":rich(kind),"language":"rust","checked":true,"caption":rich("caption")}}));
        }
        blocks.extend([
            json!({"type":"equation","equation":{"expression":"x = y"}}),
            json!({"type":"divider","divider":{}}),
            json!({"id":"child","type":"child_page","child_page":{"title":"Child"}}),
            json!({"type":"link_to_page","link_to_page":{"type":"page_id","page_id":"target"}}),
            json!({"type":"bookmark","bookmark":{"url":"https://example.org/bookmark"}}),
            json!({"type":"file","file":{"type":"file","name":"notes.pdf","file":{"url":"https://files.example/notes.pdf?secret=rotating","expiry_time":"today"}}}),
            json!({"type":"image","image":{"type":"external","external":{"url":"https://example.org/image"}}}),
        ]);
        let document = json!({"page":{"id":"page","url":"https://www.notion.so/page","properties":{
            "Title":{"type":"title","title":rich("Reference")},
            "Text":{"type":"rich_text","rich_text":rich("Description")},
            "Tags":{"type":"multi_select","multi_select":[{"name":"docs"},{"name":"docs"}]},
            "Status":{"type":"status","status":{"name":"Ready"}},
            "URL":{"type":"url","url":"https://example.org"},
            "Number":{"type":"number","number":12},"Empty":{"type":"number","number":null},
            "Relation":{"type":"relation","relation":[{"id":"related-page"}]}
        }},"hierarchy":[{"id":"database","title":"Projects"}],"blocks":blocks});
        let metadata = provenance(&document);
        assert_eq!(metadata["hierarchy"][0]["title"], "Projects");
        assert!(!metadata.to_string().contains("secret=rotating"));
        let canonical = normalize(RawDocument {
            external_id: "page".into(),
            revision: "v1".into(),
            body: document.to_string(),
            metadata: metadata.to_string(),
            occurred_at: None,
        })
        .unwrap();
        assert_eq!(canonical.title.as_deref(), Some("Reference"));
        assert_eq!(canonical.uri.as_deref(), Some("https://www.notion.so/page"));
        assert_eq!(canonical.tags, vec!["docs"]);
        let output = &canonical.segments[0].text;
        for expected in [
            "Text: Description",
            "Status: Ready",
            "Number: 12",
            "# heading_1",
            "- bulleted_list_item",
            "1. numbered_list_item",
            "[x] to_do",
            "> quote",
            "```rust",
            "x = y",
            "Child",
            "https://www.notion.so/target",
            "notes.pdf",
            "https://example.org/bookmark",
            "future",
        ] {
            assert!(output.contains(expected), "{expected}: {output}");
        }
        assert!(!output.contains("secret=rotating"));
    }
    #[test]
    fn renders_unknown_children_links_and_tables() {
        let blocks = json!([
            {"id":"one","type":"future","future":{"rich_text":[{"text":{"content":"Future","link":{"url":"https://example.org"}}}]},"children":[{"type":"heading_2","heading_2":{"rich_text":[{"plain_text":"Nested"}]}}]},
            {"type":"table_row","table_row":{"cells":[[{"plain_text":"A"}],[{"plain_text":"B"}]]}}
        ]);
        let mut output = String::new();
        for block in blocks.as_array().unwrap() {
            block_text(block, 0, &mut output);
        }
        assert!(output.contains("[Future](https://example.org)"));
        assert!(output.contains("## Nested"));
        assert!(output.contains("| A | B |"));
    }
    #[test]
    fn signed_url_rotation_is_not_a_revision() {
        let first = json!({"type":"file","file":{"url":"https://files.example/path?token=one","expiry_time":"now"}});
        let second = json!({"type":"file","file":{"url":"https://files.example/path?token=two","expiry_time":"later"}});
        assert_eq!(revision_value(&first), revision_value(&second));
        assert_ne!(
            revision_value(
                &json!({"type":"external","external":{"url":"https://example.org/?page=1"}})
            ),
            revision_value(
                &json!({"type":"external","external":{"url":"https://example.org/?page=2"}})
            )
        );
    }
    #[test]
    fn tombstones_have_no_searchable_segments_and_are_stable() {
        let first = crate::tombstone("one", "unavailable");
        let second = crate::tombstone("one", "unavailable");
        assert_eq!(first.revision, second.revision);
        assert!(normalize(first).unwrap().segments.is_empty());
    }
}
