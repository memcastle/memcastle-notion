//! Fixed-origin HTTPS transport. Credentials only cross the host boundary on stdin.
use crate::config::{Config, id};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    time::{Duration, Instant},
};

const MAX_ITEMS: usize = 20_000;
#[cfg(test)]
type Transport = Box<dyn Fn(&str, Option<&Value>) -> Result<Vec<u8>, String>>;
pub(crate) struct Client {
    token: String,
    started: Instant,
    #[cfg(test)]
    transport: Option<Transport>,
}

impl Client {
    pub fn new() -> Result<Self, String> {
        #[cfg(target_arch = "wasm32")]
        let token = crate::memcastle::source::host::source_credential()
            .map_err(|_| "select a Notion integration credential through a source configuration")?;
        #[cfg(not(target_arch = "wasm32"))]
        let token = String::new();
        if token.is_empty() || token.contains(['\r', '\n', '\0']) {
            return Err("missing or invalid selected Notion credential".into());
        }
        Ok(Self {
            token,
            started: Instant::now(),
            #[cfg(test)]
            transport: None,
        })
    }

    #[cfg(test)]
    pub(crate) fn fixture(
        transport: impl Fn(&str, Option<&Value>) -> Result<Vec<u8>, String> + 'static,
    ) -> Self {
        Self {
            token: "fixture-selected-token".into(),
            started: Instant::now(),
            transport: Some(Box::new(transport)),
        }
    }

    pub fn get_optional(&self, path: &str) -> Result<Option<Value>, String> {
        self.request(path, None)
    }

    pub fn request(&self, path: &str, body: Option<&Value>) -> Result<Option<Value>, String> {
        if !path.starts_with('/')
            || path.starts_with("//")
            || !path
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"/-_?=&%.".contains(&b))
        {
            return Err("invalid Notion API path".into());
        }
        let mut config = format!(
            "url = \"https://api.notion.com/v1{path}\"\nheader = \"Authorization: Bearer {}\"\nheader = \"Notion-Version: 2026-03-11\"\nheader = \"Content-Type: application/json\"\n",
            escape(&self.token)
        );
        if let Some(body) = body {
            config.push_str(&format!(
                "request = \"POST\"\ndata = \"{}\"\n",
                escape(&body.to_string())
            ));
        }
        for attempt in 0..3 {
            let remaining = 40u64.saturating_sub(self.started.elapsed().as_secs());
            if remaining == 0 {
                return Err(
                    "Notion acquisition exceeded its time budget; retry or narrow scope".into(),
                );
            }
            let args = vec![
                "-q".into(),
                "--silent".into(),
                "--show-error".into(),
                "--config".into(),
                "-".into(),
                "--proto".into(),
                "=https".into(),
                "--max-redirs".into(),
                "0".into(),
                "--max-time".into(),
                remaining.min(15).to_string(),
                "--max-filesize".into(),
                "8388608".into(),
                "--include".into(),
            ];
            #[cfg(test)]
            let output = if let Some(transport) = &self.transport {
                transport(path, body)
            } else {
                process(&args, config.as_bytes())
            };
            #[cfg(not(test))]
            let output = process(&args, config.as_bytes());
            let output = match output {
                Ok(output) => output,
                Err(_) if attempt < 2 && remaining > 18 => { std::thread::sleep(Duration::from_secs(1 << attempt)); continue; }
                Err(_) => return Err("Notion HTTPS request failed; install curl on the daemon PATH and check connectivity".into()),
            };
            let (status, retry, body) = response(&output)?;
            if status == 200 {
                return serde_json::from_str(body)
                    .map(Some)
                    .map_err(|_| "Notion returned malformed JSON".into());
            }
            if status == 404 {
                return Ok(None);
            }
            if matches!(status, 401 | 403) {
                return Err(
                    "Notion refused the request; check selected credential and read capabilities"
                        .into(),
                );
            }
            if matches!(status, 301 | 302 | 303 | 307 | 308) {
                return Err("Notion redirected a request; refusing to forward credentials".into());
            }
            if matches!(status, 429 | 529 | 500 | 502 | 503 | 504) {
                let delay = retry.unwrap_or(1 << attempt);
                let jitter = u64::from(self.started.elapsed().subsec_nanos() % 250);
                if attempt < 2
                    && delay.saturating_add(2)
                        < 40u64.saturating_sub(self.started.elapsed().as_secs())
                {
                    std::thread::sleep(Duration::from_millis(
                        delay.saturating_mul(1000).saturating_add(jitter),
                    ));
                    continue;
                }
                return Err(format!(
                    "Notion temporarily unavailable (HTTP {status}); retry after at least {delay} seconds"
                ));
            }
            return Err(format!(
                "Notion API returned HTTP {status}; check scope and API compatibility"
            ));
        }
        unreachable!()
    }

    pub fn list(
        &self,
        path: &str,
        body: Option<Value>,
        cursor: Option<&str>,
    ) -> Result<Value, String> {
        let response = if let Some(mut body) = body {
            body["page_size"] = json!(100);
            if let Some(cursor) = cursor {
                body["start_cursor"] = json!(cursor);
            }
            self.request(path, Some(&body))?
        } else {
            let suffix = cursor
                .map(|s| format!("&start_cursor={}", encode(s)))
                .unwrap_or_default();
            self.request(&format!("{path}?page_size=100{suffix}"), None)?
        }
        .ok_or("Notion container became unavailable during traversal; retry reconciliation")?;
        if response["results"].as_array().is_none_or(|v| v.len() > 100)
            || !response["has_more"].is_boolean()
        {
            return Err("Notion returned an invalid list".into());
        }
        if response["has_more"] == true
            && response["next_cursor"]
                .as_str()
                .is_none_or(|s| s.is_empty() || Some(s) == cursor)
        {
            return Err("Notion returned an invalid pagination cursor".into());
        }
        Ok(response)
    }

    fn all(&self, path: &str) -> Result<Vec<Value>, String> {
        let mut cursor: Option<String> = None;
        let mut seen = BTreeSet::new();
        let mut values = Vec::new();
        loop {
            let response = self.list(path, None, cursor.as_deref())?;
            values.extend(response["results"].as_array().unwrap().iter().cloned());
            if values.len() > MAX_ITEMS {
                return Err("Notion page exceeds 20000 items; select smaller pages".into());
            }
            if response["has_more"] != true {
                return Ok(values);
            }
            let next = response["next_cursor"].as_str().unwrap().to_string();
            if !seen.insert(next.clone()) {
                return Err("Notion repeated a pagination cursor".into());
            }
            cursor = Some(next);
        }
    }

    pub fn complete_properties(&self, page: &mut Value) -> Result<(), String> {
        let page_id = id(page["id"].as_str().ok_or("Notion page has no ID")?)?;
        let Some(properties) = page["properties"].as_object_mut() else {
            return Err("Notion page has no properties".into());
        };
        for property in properties.values_mut() {
            let kind = property["type"].as_str().unwrap_or("").to_string();
            // Rich text/title may be truncated without a has_more indicator.
            if property["has_more"] == true
                || matches!(kind.as_str(), "title" | "rich_text" | "people" | "relation")
                    && property[&kind].as_array().is_some_and(|v| v.len() >= 25)
            {
                let key =
                    property_path(property["id"].as_str().ok_or("Notion property has no ID")?);
                let items = self.all(&format!("/pages/{page_id}/properties/{key}"))?;
                let values: Vec<Value> =
                    items.into_iter().map(|item| item[&kind].clone()).collect();
                property[&kind] = json!(values);
                property["has_more"] = json!(false);
            }
        }
        Ok(())
    }

    pub fn tree(&self, page: &str, config: &Config) -> Result<Vec<Value>, String> {
        let mut visited = BTreeSet::new();
        self.children(
            page,
            config,
            config.include_blocks.is_empty(),
            0,
            &mut visited,
            &mut 0,
        )
    }

    /// Page parent metadata may name the containing page rather than its physical
    /// toggle/list block. Resolve that edge before applying a block exclusion.
    pub fn child_page_excluded(
        &self,
        parent: &str,
        child: &str,
        excluded: &BTreeSet<String>,
    ) -> Result<bool, String> {
        let mut queue = std::collections::VecDeque::from([(parent.to_string(), false, 0)]);
        let mut seen = BTreeSet::new();
        let mut count = 0;
        while let Some((parent, inherited, depth)) = queue.pop_front() {
            if depth > 50 || !seen.insert(parent.clone()) {
                return Err("cannot resolve Notion child-page block ancestry".into());
            }
            for block in self.all(&format!("/blocks/{parent}/children"))? {
                count += 1;
                if count > MAX_ITEMS {
                    return Err("Notion parent page exceeds 20000 blocks".into());
                }
                let block_id = id(block["id"].as_str().ok_or("Notion block has no ID")?)?;
                let blocked = inherited || excluded.contains(&block_id);
                if block["type"] == "child_page" && block_id == child {
                    return Ok(blocked);
                }
                if block["has_children"] == true
                    && block["type"] != "child_page"
                    && block["type"] != "child_database"
                {
                    queue.push_back((block_id, blocked, depth + 1));
                }
            }
        }
        Err("cannot find Notion child page in its parent's block tree; retry reconciliation".into())
    }

    fn children(
        &self,
        parent: &str,
        config: &Config,
        included: bool,
        depth: usize,
        visited: &mut BTreeSet<String>,
        count: &mut usize,
    ) -> Result<Vec<Value>, String> {
        if depth > 50 {
            return Err("Notion block tree exceeds 50 levels".into());
        }
        let mut result = Vec::new();
        for mut block in self.all(&format!("/blocks/{parent}/children"))? {
            let block_id = id(block["id"].as_str().ok_or("Notion block has no ID")?)?;
            if !visited.insert(block_id.clone()) {
                return Err("Notion block tree contains a cycle".into());
            }
            *count += 1;
            if *count > MAX_ITEMS {
                return Err("Notion page exceeds 20000 blocks".into());
            }
            if config.exclude_blocks.contains(&block_id) || block["in_trash"] == true {
                visited.remove(&block_id);
                continue;
            }
            let selected = included || config.include_blocks.contains(&block_id);
            let kind = block["type"].as_str().unwrap_or("").to_string();
            let mut child_id = block_id.clone();
            if kind == "synced_block"
                && let Some(original) = block
                    .pointer("/synced_block/synced_from/block_id")
                    .and_then(Value::as_str)
            {
                child_id = id(original)?;
            }
            let children = if block["has_children"] == true
                && !matches!(kind.as_str(), "child_page" | "child_database")
            {
                self.children(&child_id, config, selected, depth + 1, visited, count)?
            } else {
                Vec::new()
            };
            if selected {
                block["children"] = json!(children);
                result.push(block);
            } else {
                result.extend(children);
            }
            visited.remove(&block_id);
        }
        Ok(result)
    }
}

fn escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}
pub(crate) fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

// Notion returns short property IDs already URL-encoded. Preserve valid escapes
// instead of turning `%3A` into `%253A`, while still encoding raw unsafe bytes.
fn property_path(value: &str) -> String {
    let mut result = String::new();
    let bytes = value.as_bytes();
    let mut offset = 0;
    while offset < bytes.len() {
        if bytes[offset] == b'%'
            && offset + 2 < bytes.len()
            && bytes[offset + 1..offset + 3]
                .iter()
                .all(u8::is_ascii_hexdigit)
        {
            result.push('%');
            result.push((bytes[offset + 1] as char).to_ascii_uppercase());
            result.push((bytes[offset + 2] as char).to_ascii_uppercase());
            offset += 3;
        } else {
            let byte = bytes[offset];
            if byte.is_ascii_alphanumeric() || b"-_.~".contains(&byte) {
                result.push(byte as char);
            } else {
                result.push_str(&format!("%{byte:02X}"));
            }
            offset += 1;
        }
    }
    result
}

fn process(args: &[String], stdin: &[u8]) -> Result<Vec<u8>, String> {
    #[cfg(target_arch = "wasm32")]
    {
        let output = crate::memcastle::source::host::run_process("curl", args, Some(stdin))
            .map_err(|_| "cannot run curl; install curl on the daemon PATH")?;
        if output.status != 0 {
            return Err("Notion HTTPS request failed; check connectivity and retry".into());
        }
        Ok(output.stdout)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = (args, stdin);
        Err("HTTP acquisition requires the MemCastle host".into())
    }
}

fn response(bytes: &[u8]) -> Result<(u16, Option<u64>, &str), String> {
    if bytes.len() > 8 * 1024 * 1024 {
        return Err("Notion response exceeds 8 MiB".into());
    }
    let mut text = std::str::from_utf8(bytes).map_err(|_| "Notion response is not UTF-8")?;
    loop {
        let (headers, body) = text
            .split_once("\r\n\r\n")
            .or_else(|| text.split_once("\n\n"))
            .ok_or("Notion response has no HTTP headers")?;
        let status = headers
            .lines()
            .next()
            .and_then(|s| s.split_whitespace().nth(1))
            .and_then(|s| s.parse::<u16>().ok())
            .ok_or("Notion response has no HTTP status")?;
        // curl may include a proxy CONNECT or informational header before the real response.
        if body.starts_with("HTTP/") {
            text = body;
            continue;
        }
        let retry = headers
            .lines()
            .filter_map(|l| l.split_once(':'))
            .find(|(k, _)| k.eq_ignore_ascii_case("retry-after"))
            .map(|(_, v)| {
                v.trim()
                    .parse::<u64>()
                    .map_err(|_| "Notion returned invalid Retry-After")
            })
            .transpose()?;
        return Ok((status, retry, body));
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    pub(crate) fn reply(status: u16, value: Value) -> Result<Vec<u8>, String> {
        Ok(format!("HTTP/2 {status}\r\nRetry-After: 0\r\n\r\n{value}").into_bytes())
    }
    #[test]
    fn request_statuses_retry_and_secrets() {
        for status in [401, 403, 301, 302, 307, 308, 400, 404, 429, 529] {
            let client = Client::fixture(move |_, _| {
                reply(status, json!({"message":"private fixture-selected-token"}))
            });
            let result = client.get_optional("/pages/test");
            if status == 404 {
                assert!(result.unwrap().is_none());
            } else {
                let error = result.unwrap_err();
                assert!(!error.contains("fixture-selected-token"));
                assert!(!error.contains("private"));
            }
        }
        let count = std::cell::Cell::new(0);
        let client = Client::fixture(move |_, body| {
            assert_eq!(body.unwrap()["test"], true);
            let n = count.get();
            count.set(n + 1);
            reply(if n == 0 { 503 } else { 200 }, json!({"ok":true}))
        });
        assert_eq!(
            client
                .request("/search", Some(&json!({"test":true})))
                .unwrap()
                .unwrap()["ok"],
            true
        );
        for path in ["//evil", "https://evil", "/x\nAuthorization", "/x\0"] {
            assert!(client.request(path, None).is_err());
        }
        let client =
            Client::fixture(|_, _| Ok(b"HTTP/2 200\r\n\r\nprivate malformed body".to_vec()));
        assert_eq!(
            client.get_optional("/pages/x").unwrap_err(),
            "Notion returned malformed JSON"
        );
        assert!(Client::new().is_err());
    }
    #[test]
    fn pagination_properties_and_block_selection() {
        let a = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";
        let b = "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb";
        let client = Client::fixture(move |path, _| {
            let result = if path.contains("/properties/") {
                json!({"results":[{"title":{"plain_text":"complete"}}],"has_more":false})
            } else if path.starts_with(&format!("/blocks/{a}/")) && !path.contains("start_cursor") {
                json!({"results":[{"id":b,"type":"toggle","has_children":true,"toggle":{"rich_text":[{"plain_text":"parent"}]}}],"has_more":true,"next_cursor":"next"})
            } else if path.contains("start_cursor") {
                json!({"results":[],"has_more":false})
            } else {
                json!({"results":[{"id":"cccccccc-cccc-cccc-cccc-cccccccccccc","type":"paragraph","paragraph":{"rich_text":[{"plain_text":"nested"}]}}],"has_more":false})
            };
            reply(200, result)
        });
        let mut page = json!({"id":a,"properties":{"Name":{"id":"title","type":"title","title":[],"has_more":true}}});
        client.complete_properties(&mut page).unwrap();
        assert_eq!(
            page["properties"]["Name"]["title"][0]["plain_text"],
            "complete"
        );
        let opts = |key: &str| {
            vec![
                ("account".into(), "work".into()),
                ("pages".into(), a.into()),
                (key.into(), b.into()),
            ]
        };
        let tree = client
            .tree(a, &Config::parse(&opts("include_blocks")).unwrap())
            .unwrap();
        assert_eq!(tree.len(), 1);
        assert_eq!(tree[0]["children"].as_array().unwrap().len(), 1);
        assert!(
            client
                .tree(a, &Config::parse(&opts("exclude_blocks")).unwrap())
                .unwrap()
                .is_empty()
        );
    }
    #[test]
    fn list_shape_and_cursor_validation() {
        for value in [
            json!({}),
            json!({"results":[],"has_more":true}),
            json!({"results":[],"has_more":true,"next_cursor":"same"}),
        ] {
            let client = Client::fixture(move |_, _| reply(200, value.clone()));
            assert!(
                client
                    .list("/search", Some(json!({})), Some("same"))
                    .is_err()
            );
        }
        let client = Client::fixture(|_, body| {
            assert_eq!(body.unwrap()["start_cursor"], "next");
            reply(200, json!({"results":[],"has_more":false}))
        });
        assert!(
            client
                .list("/search", Some(json!({})), Some("next"))
                .is_ok()
        );
    }
    #[test]
    fn child_page_exclusions_follow_physical_block_ancestry() {
        let parent = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";
        let toggle = "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb";
        let child = "cccccccc-cccc-cccc-cccc-cccccccccccc";
        let client = Client::fixture(move |path, _| {
            reply(
                200,
                if path.contains(parent) {
                    json!({"results":[{"id":toggle,"type":"toggle","has_children":true}],"has_more":false})
                } else {
                    json!({"results":[{"id":child,"type":"child_page"}],"has_more":false})
                },
            )
        });
        assert!(
            client
                .child_page_excluded(parent, child, &BTreeSet::from([toggle.into()]))
                .unwrap()
        );
        assert!(
            !client
                .child_page_excluded(parent, child, &BTreeSet::new())
                .unwrap()
        );
        assert!(
            client
                .child_page_excluded(parent, "missing", &BTreeSet::new())
                .is_err()
        );
    }
    #[test]
    fn parses_headers_without_reflecting_body() {
        assert_eq!(response(b"HTTP/1.1 200 Connection established\r\n\r\nHTTP/2 429\r\nRetry-After: 75\r\n\r\nsecret").unwrap(), (429, Some(75), "secret"));
        assert!(
            response(b"private invalid body")
                .unwrap_err()
                .contains("headers")
        );
        assert!(
            response(b"HTTP/2 429\r\nretry-after: secret\r\n\r\n{}")
                .unwrap_err()
                .contains("Retry-After")
        );
    }
    #[test]
    fn escapes_stdin_and_query_values() {
        assert_eq!(escape("a\"\\b"), "a\\\"\\\\b");
        assert_eq!(encode("x/y? +"), "x%2Fy%3F%20%2B");
        assert_eq!(property_path("%3aabc/def%"), "%3Aabc%2Fdef%25");
    }
}
