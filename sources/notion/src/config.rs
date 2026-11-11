//! Options use Core's scalar/list boundary; compound property filters are JSON strings.
use globset::{Glob, GlobMatcher};
use regex::Regex;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

pub(crate) fn id(input: &str) -> Result<String, String> {
    let compact = input.replace('-', "");
    if compact.len() != 32 || !compact.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("Notion IDs must be 32 hexadecimal digits (hyphens optional)".into());
    }
    let s = compact.to_ascii_lowercase();
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &s[..8],
        &s[8..12],
        &s[12..16],
        &s[16..20],
        &s[20..]
    ))
}

pub(crate) struct Config {
    pub account: String,
    pub pages: BTreeSet<String>,
    pub roots: BTreeSet<String>,
    pub databases: BTreeSet<String>,
    pub data_sources: BTreeSet<String>,
    pub exclude_pages: BTreeSet<String>,
    pub include_blocks: BTreeSet<String>,
    pub exclude_blocks: BTreeSet<String>,
    pub workspace: bool,
    pub refresh: u64,
    pub fingerprint: String,
    pub prefix: Option<String>,
    pub glob: Option<GlobMatcher>,
    pub regex: Option<Regex>,
    pub properties: Option<Value>,
}

impl Config {
    pub fn parse(options: &[(String, String)]) -> Result<Self, String> {
        let mut values = BTreeMap::<String, String>::new();
        for (key, value) in options {
            if ![
                "account",
                "pages",
                "roots",
                "databases",
                "data_sources",
                "exclude_pages",
                "include_blocks",
                "exclude_blocks",
                "workspace",
                "refresh_seconds",
                "rescan",
                "title_prefix",
                "title_glob",
                "title_regex",
                "property_filter",
            ]
            .contains(&key.as_str())
            {
                return Err(format!("unknown Notion option: {key}"));
            }
            if values.insert(key.clone(), value.clone()).is_some() {
                return Err(format!("duplicate option: {key}"));
            }
        }
        let ids = |key: &str| -> Result<BTreeSet<String>, String> {
            let Some(value) = values.get(key) else {
                return Ok(BTreeSet::new());
            };
            if value.trim().is_empty() {
                return Ok(BTreeSet::new());
            }
            let list: Vec<String> = if value.starts_with('[') {
                serde_json::from_str(value).map_err(|_| format!("{key} must be a list of IDs"))?
            } else {
                value.split(',').map(|s| s.trim().to_string()).collect()
            };
            list.iter().map(|s| id(s)).collect()
        };
        let account = values
            .get("account")
            .ok_or("set a stable non-secret account namespace in the source configuration")?
            .clone();
        if account.is_empty()
            || account.len() > 100
            || !account
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
        {
            return Err("account must contain letters, digits, -, _, .".into());
        }
        let workspace = match values.get("workspace").map(String::as_str) {
            None | Some("false") => false,
            Some("true") => true,
            _ => return Err("workspace must be true or false".into()),
        };
        let refresh = values
            .get("refresh_seconds")
            .map_or(Ok(3600), |v| v.parse::<u64>())
            .map_err(|_| "refresh_seconds must be an integer")?;
        if !(60..=2_592_000).contains(&refresh) {
            return Err("refresh_seconds must be between 60 and 2592000".into());
        }
        let properties = values
            .get("property_filter")
            .map(|v| {
                serde_json::from_str::<Value>(v)
                    .map_err(|_| "property_filter must be valid JSON".to_string())
            })
            .transpose()?;
        if let Some(filter) = &properties {
            validate_filter(filter, 0)?;
        }
        let result = Self {
            account,
            pages: ids("pages")?,
            roots: ids("roots")?,
            databases: ids("databases")?,
            data_sources: ids("data_sources")?,
            exclude_pages: ids("exclude_pages")?,
            include_blocks: ids("include_blocks")?,
            exclude_blocks: ids("exclude_blocks")?,
            workspace,
            refresh,
            fingerprint: crate::hash(&serde_json::to_string(&values).unwrap()),
            prefix: values.get("title_prefix").cloned(),
            glob: values
                .get("title_glob")
                .map(|v| {
                    Glob::new(v)
                        .map(|g| g.compile_matcher())
                        .map_err(|_| "invalid title_glob")
                })
                .transpose()?,
            regex: values
                .get("title_regex")
                .map(|v| Regex::new(v).map_err(|_| "invalid title_regex"))
                .transpose()?,
            properties,
        };
        if !workspace
            && result.pages.is_empty()
            && result.roots.is_empty()
            && result.databases.is_empty()
            && result.data_sources.is_empty()
        {
            return Err(
                "select pages, roots, databases or data_sources; workspace=true is explicit opt-in"
                    .into(),
            );
        }
        Ok(result)
    }

    pub fn matches(&self, page: &Value) -> bool {
        let title = title(page);
        self.prefix.as_ref().is_none_or(|p| title.starts_with(p))
            && self.glob.as_ref().is_none_or(|g| g.is_match(&title))
            && self.regex.as_ref().is_none_or(|r| r.is_match(&title))
            && self
                .properties
                .as_ref()
                .is_none_or(|p| filter_matches(p, &page["properties"]))
    }
}

pub(crate) fn rich(value: &Value) -> String {
    value
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|v| {
                    v.get("plain_text")
                        .and_then(Value::as_str)
                        .or_else(|| v.pointer("/text/content").and_then(Value::as_str))
                        .unwrap_or("")
                })
                .collect()
        })
        .unwrap_or_default()
}

pub(crate) fn title(page: &Value) -> String {
    page["properties"]
        .as_object()
        .and_then(|p| p.values().find(|v| v["type"] == "title"))
        .map(|v| rich(&v["title"]))
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "Untitled".into())
}

fn validate_filter(f: &Value, depth: usize) -> Result<(), String> {
    let invalid = || {
        "unsupported property_filter: use and/or or property + title/rich_text/status/select/multi_select/checkbox/number conditions".to_string()
    };
    if depth > 12 {
        return Err("property_filter exceeds nesting limit".into());
    }
    let object = f.as_object().ok_or_else(invalid)?;
    if object.len() == 1 {
        for op in ["and", "or"] {
            if let Some(v) = object.get(op) {
                let list = v
                    .as_array()
                    .filter(|v| !v.is_empty() && v.len() <= 100)
                    .ok_or_else(invalid)?;
                for item in list {
                    validate_filter(item, depth + 1)?;
                }
                return Ok(());
            }
        }
    }
    if object.len() != 2 || f["property"].as_str().is_none_or(str::is_empty) {
        return Err(invalid());
    }
    let (kind, condition) = object
        .iter()
        .find(|(k, _)| k.as_str() != "property")
        .ok_or_else(invalid)?;
    if ![
        "title",
        "rich_text",
        "status",
        "select",
        "multi_select",
        "checkbox",
        "number",
    ]
    .contains(&kind.as_str())
    {
        return Err(invalid());
    }
    let conditions = condition
        .as_object()
        .filter(|c| c.len() == 1)
        .ok_or_else(invalid)?;
    let (op, value) = conditions.iter().next().unwrap();
    let allowed: &[&str] = match kind.as_str() {
        "number" => &[
            "equals",
            "does_not_equal",
            "greater_than",
            "less_than",
            "greater_than_or_equal_to",
            "less_than_or_equal_to",
            "is_empty",
            "is_not_empty",
        ],
        "checkbox" => &["equals", "does_not_equal"],
        "multi_select" => &["contains", "does_not_contain", "is_empty", "is_not_empty"],
        "select" | "status" => &["equals", "does_not_equal", "is_empty", "is_not_empty"],
        _ => &[
            "equals",
            "does_not_equal",
            "contains",
            "does_not_contain",
            "starts_with",
            "ends_with",
            "is_empty",
            "is_not_empty",
        ],
    };
    if !allowed.contains(&op.as_str())
        || if op.starts_with("is_") || kind == "checkbox" {
            !value.is_boolean()
        } else if kind == "number" {
            !value.is_number()
        } else {
            !value.is_string()
        }
    {
        return Err(invalid());
    }
    Ok(())
}

fn filter_matches(f: &Value, properties: &Value) -> bool {
    if let Some(items) = f["and"].as_array() {
        return items.iter().all(|v| filter_matches(v, properties));
    }
    if let Some(items) = f["or"].as_array() {
        return items.iter().any(|v| filter_matches(v, properties));
    }
    let name = f["property"].as_str().unwrap_or("");
    let Some(property) = properties
        .get(name)
        .or_else(|| properties.as_object()?.values().find(|v| v["id"] == name))
    else {
        return false;
    };
    let (kind, condition) = f
        .as_object()
        .unwrap()
        .iter()
        .find(|(k, _)| k.as_str() != "property")
        .unwrap();
    if property["type"] != kind.as_str() {
        return false;
    }
    let (op, target) = condition.as_object().unwrap().iter().next().unwrap();
    let value = &property[kind];
    let text = match kind.as_str() {
        "title" | "rich_text" => rich(value),
        "select" | "status" => value["name"].as_str().unwrap_or("").into(),
        _ => String::new(),
    };
    let empty = value.is_null()
        || (matches!(kind.as_str(), "title" | "rich_text" | "select" | "status")
            && text.is_empty())
        || value.as_array().is_some_and(Vec::is_empty);
    if op == "is_empty" {
        return empty == target.as_bool().unwrap();
    }
    if op == "is_not_empty" {
        return empty != target.as_bool().unwrap();
    }
    if empty {
        return false;
    }
    if kind == "number" {
        let Some(a) = value.as_f64() else {
            return false;
        };
        let b = target.as_f64().unwrap();
        return match op.as_str() {
            "equals" => a == b,
            "does_not_equal" => a != b,
            "greater_than" => a > b,
            "less_than" => a < b,
            "greater_than_or_equal_to" => a >= b,
            "less_than_or_equal_to" => a <= b,
            _ => false,
        };
    }
    if kind == "checkbox" {
        return (value == target) == (op == "equals");
    }
    let target = target.as_str().unwrap();
    let contains = if kind == "multi_select" {
        value
            .as_array()
            .is_some_and(|v| v.iter().any(|v| v["name"] == target))
    } else {
        text.contains(target)
    };
    match op.as_str() {
        "equals" => text == target,
        "does_not_equal" => text != target,
        "contains" => contains,
        "does_not_contain" => !contains,
        "starts_with" => text.starts_with(target),
        "ends_with" => text.ends_with(target),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn options(extra: &[(&str, &str)]) -> Vec<(String, String)> {
        [("account", "work"), ("workspace", "true")]
            .into_iter()
            .chain(extra.iter().copied())
            .map(|(k, v)| (k.into(), v.into()))
            .collect()
    }
    #[test]
    fn scope_is_explicit_and_options_are_strict() {
        assert!(Config::parse(&[]).is_err());
        assert!(Config::parse(&[("account".into(), "work".into())]).is_err());
        for pair in [
            ("workspace", "yes"),
            ("refresh_seconds", "0"),
            ("refresh_seconds", "many"),
            ("title_regex", "["),
            ("title_glob", "["),
            ("pages", "not-an-id"),
            ("unknown", "x"),
            ("account", "secret\nheader"),
        ] {
            let mut opts = options(&[]);
            opts.retain(|(k, _)| k != pair.0);
            opts.push((pair.0.into(), pair.1.into()));
            assert!(Config::parse(&opts).is_err(), "{}", pair.0);
        }
        assert!(Config::parse(&options(&[("account", "duplicate")])).is_err());
        let list = r#"["AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA","bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb"]"#;
        let config = Config::parse(&options(&[("roots", list)])).unwrap();
        assert_eq!(config.roots.len(), 2);
        assert!(
            Config::parse(&options(&[("include_blocks", ""), ("exclude_pages", "[]")]))
                .unwrap()
                .include_blocks
                .is_empty()
        );
        assert!(
            config
                .roots
                .contains("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa")
        );
    }
    #[test]
    fn title_and_property_filters_intersect() {
        let config = Config::parse(&options(&[("title_prefix", "Design"), ("title_glob", "*RFC*"), ("title_regex", "[0-9]+$"), ("property_filter", r#"{"and":[{"property":"Status","status":{"equals":"Ready"}},{"property":"Tags","multi_select":{"contains":"docs"}}]}"#)])).unwrap();
        let mut page = json!({"properties":{"Name":{"type":"title","title":[{"plain_text":"Design RFC 42"}]},"Status":{"type":"status","status":{"name":"Ready"}},"Tags":{"type":"multi_select","multi_select":[{"name":"docs"}]}}});
        assert!(config.matches(&page));
        page["properties"]["Status"]["status"]["name"] = json!("Draft");
        assert!(!config.matches(&page));
        page["properties"]["Status"]["status"]["name"] = json!("Ready");
        page["properties"]["Name"]["title"][0]["plain_text"] = json!("Design RFC");
        assert!(!config.matches(&page));
    }
    #[test]
    fn predicate_types_operators_and_missing_properties() {
        let props = json!({"N":{"id":"n","type":"number","number":4},"C":{"type":"checkbox","checkbox":true},"Text":{"type":"rich_text","rich_text":[{"text":{"content":"hello world"}}]},"Empty":{"type":"select","select":null}});
        for f in [
            json!({"property":"n","number":{"greater_than":3}}),
            json!({"property":"C","checkbox":{"equals":true}}),
            json!({"property":"Text","rich_text":{"ends_with":"world"}}),
            json!({"property":"Empty","select":{"is_empty":true}}),
            json!({"or":[{"property":"absent","number":{"equals":4}},{"property":"N","number":{"less_than_or_equal_to":4}}]}),
        ] {
            validate_filter(&f, 0).unwrap();
            assert!(filter_matches(&f, &props), "{f}");
        }
        for f in [
            json!({"property":"absent","number":{"does_not_equal":4}}),
            json!({"property":"N","number":{"less_than":4}}),
            json!({"property":"C","checkbox":{"does_not_equal":true}}),
        ] {
            assert!(!filter_matches(&f, &props));
        }
        for f in [
            json!({}),
            json!({"and":[]}),
            json!({"property":"N","formula":{"equals":1}}),
            json!({"property":"N","number":{"equals":"4"}}),
            json!({"property":"C","checkbox":{"contains":true}}),
        ] {
            assert!(validate_filter(&f, 0).is_err());
        }
    }
}
