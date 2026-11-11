//! Bounded discovery traversal. Core owns the persistent document inventory.
use crate::{
    api::Client,
    config::{Config, id},
    exports::memcastle::source::reconciliation::DiscoveryPage,
    failed,
    memcastle::source::types::{Candidate, SourceError, SourceRef},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeSet, VecDeque},
    time::{SystemTime, UNIX_EPOCH},
};

const MAX_CURSOR: usize = 4 * 1024 * 1024;

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "task", rename_all = "snake_case", deny_unknown_fields)]
enum Task {
    Page {
        id: String,
    },
    Blocks {
        id: String,
        cursor: Option<String>,
        seen: BTreeSet<String>,
    },
    Database {
        id: String,
    },
    Query {
        id: String,
        cursor: Option<String>,
        seen: BTreeSet<String>,
        from: i64,
        to: i64,
    },
    Search {
        cursor: Option<String>,
        seen: BTreeSet<String>,
    },
}

#[derive(Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct State {
    version: u32,
    scope: String,
    fingerprint: String,
    visited: BTreeSet<String>,
    queue: VecDeque<Task>,
    next_scan: u64,
    generation: u64,
    /// Bounded metadata-only diagnostics, not an additional content inventory.
    #[serde(default)]
    discovery_decisions: VecDeque<(String, String)>,
}

impl State {
    fn load(cursor: &str, scope: &str) -> Result<Self, SourceError> {
        if cursor.len() > MAX_CURSOR {
            return Err(SourceError::CursorInvalid(
                "Notion cursor exceeds 4 MiB".into(),
            ));
        }
        if cursor.trim() == "null" {
            return Ok(Self {
                version: 2,
                scope: scope.into(),
                ..Self::default()
            });
        }
        let state: Self = serde_json::from_str(cursor).map_err(|_| SourceError::CursorInvalid("invalid or legacy Notion discovery checkpoint; use a full run with reconciliation-capable Core".into()))?;
        if state.version != 2 || state.scope != scope {
            return Err(SourceError::CursorInvalid(
                "unsupported Notion checkpoint version or scope".into(),
            ));
        }
        Ok(state)
    }

    fn start(&mut self, config: &Config, now: u64) {
        self.fingerprint = config.fingerprint.clone();
        self.visited.clear();
        self.discovery_decisions.clear();
        self.queue.clear();
        self.generation += 1;
        for id in config.pages.iter().chain(config.roots.iter()) {
            self.queue.push_back(Task::Page { id: id.clone() });
        }
        for id in &config.databases {
            self.queue.push_back(Task::Database { id: id.clone() });
        }
        for id in &config.data_sources {
            self.queue.push_back(query(id, now));
        }
        if config.workspace {
            self.queue.push_back(Task::Search {
                cursor: None,
                seen: BTreeSet::new(),
            });
        }
        self.next_scan = 0;
    }

    fn checkpoint(&self) -> Result<String, SourceError> {
        let cursor =
            serde_json::to_string(self).map_err(|_| failed("cannot encode Notion checkpoint"))?;
        if cursor.len() > MAX_CURSOR {
            return Err(failed(
                "Notion traversal checkpoint exceeds 4 MiB; narrow the discovery scope and run a full scan",
            ));
        }
        Ok(cursor)
    }

    fn decision(&mut self, id: &str, reason: &str) {
        self.discovery_decisions
            .push_back((id.into(), reason.into()));
        if self.discovery_decisions.len() > 128 {
            self.discovery_decisions.pop_front();
        }
    }
}

fn query(id: &str, now: u64) -> Task {
    Task::Query {
        id: id.into(),
        cursor: None,
        seen: BTreeSet::new(),
        from: 0,
        to: (now as i64 + 1) * 1000,
    }
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub(crate) fn discover(
    source: &SourceRef,
    config: &Config,
    cursor: &str,
    limit: u32,
) -> Result<DiscoveryPage, SourceError> {
    advance(
        source,
        config,
        cursor,
        limit,
        &Client::new().map_err(failed)?,
        now(),
    )
}

fn advance(
    source: &SourceRef,
    config: &Config,
    cursor: &str,
    limit: u32,
    client: &Client,
    time: u64,
) -> Result<DiscoveryPage, SourceError> {
    if limit == 0 {
        return Err(SourceError::InvalidInput(
            "discovery limit must be positive".into(),
        ));
    }
    let mut state = State::load(cursor, &source.locator)?;
    if state.fingerprint != config.fingerprint || state.queue.is_empty() && time >= state.next_scan
    {
        state.start(config, time);
    } else if state.queue.is_empty() {
        return Ok(DiscoveryPage {
            candidates: vec![],
            exhausted: true,
            cursor_after: None,
        });
    }

    let task = state
        .queue
        .pop_front()
        .ok_or_else(|| failed("Notion scope has no traversal tasks"))?;
    let mut read_id = None;
    match task {
        Task::Page { id: page_id } => {
            if state.visited.insert(format!("page:{page_id}")) {
                let page = client
                    .get_optional(&format!("/pages/{page_id}"))
                    .map_err(failed)?;
                match page {
                    None => {
                        state.decision(&page_id, "unavailable");
                    }
                    Some(mut page) => {
                        client.complete_properties(&mut page).map_err(failed)?;
                        let reason = selection(client, config, &page).map_err(failed)?;
                        state.decision(&page_id, reason);
                        if reason == "included" {
                            read_id = Some(page_id.clone());
                        }
                        // Metadata filters must not cut traversal to potentially matching descendants.
                        if page["in_trash"] != true
                            && !matches!(reason, "excluded_branch" | "trashed")
                            && (config.workspace
                                || under_root(client, config, &page).map_err(failed)?)
                        {
                            state.queue.push_back(Task::Blocks {
                                id: page_id,
                                cursor: None,
                                seen: BTreeSet::new(),
                            });
                        }
                    }
                }
            }
        }
        Task::Database { id: database_id } => {
            if state.visited.insert(format!("database:{database_id}"))
                && let Some(database) = client
                    .get_optional(&format!("/databases/{database_id}"))
                    .map_err(failed)?
            {
                let sources = database["data_sources"]
                    .as_array()
                    .ok_or_else(|| failed("Notion database has no data_sources"))?;
                for source in sources {
                    state
                        .queue
                        .push_back(query(&object_id(source).map_err(failed)?, time));
                }
            }
        }
        Task::Blocks {
            id: parent,
            cursor,
            mut seen,
        } => {
            let response = client
                .list(
                    &format!("/blocks/{parent}/children"),
                    None,
                    cursor.as_deref(),
                )
                .map_err(failed)?;
            for block in response["results"].as_array().unwrap() {
                let block_id = object_id(block).map_err(failed)?;
                if config.exclude_blocks.contains(&block_id) || block["in_trash"] == true {
                    continue;
                }
                match block["type"].as_str().unwrap_or("") {
                    "child_page" => state.queue.push_back(Task::Page { id: block_id }),
                    "child_database" => state.queue.push_back(Task::Database { id: block_id }),
                    _ if block["has_children"] == true
                        && state.visited.insert(format!("block:{block_id}")) =>
                    {
                        state.queue.push_back(Task::Blocks {
                            id: block_id,
                            cursor: None,
                            seen: BTreeSet::new(),
                        });
                    }
                    _ => {}
                }
            }
            if let Some(next) = next(&response, &mut seen).map_err(failed)? {
                state.queue.push_back(Task::Blocks {
                    id: parent,
                    cursor: Some(next),
                    seen,
                });
            }
        }
        Task::Search { cursor, mut seen } => {
            let response = client
                .list("/search", Some(json!({})), cursor.as_deref())
                .map_err(failed)?;
            for object in response["results"].as_array().unwrap() {
                let id = object_id(object).map_err(failed)?;
                match object["object"].as_str() {
                    Some("page") => state.queue.push_back(Task::Page { id }),
                    Some("data_source") => state.queue.push_back(query(&id, time)),
                    _ => {}
                }
            }
            if let Some(next) = next(&response, &mut seen).map_err(failed)? {
                state.queue.push_back(Task::Search {
                    cursor: Some(next),
                    seen,
                });
            }
        }
        Task::Query {
            id: source_id,
            cursor,
            mut seen,
            from,
            to,
        } => {
            let date = |ms| {
                chrono::DateTime::from_timestamp_millis(ms)
                    .map(|d| d.to_rfc3339())
                    .ok_or_else(|| failed("invalid Notion query time range"))
            };
            let mut filters = vec![
                json!({"timestamp":"created_time", "created_time":{"on_or_after":date(from)?}}),
                json!({"timestamp":"created_time", "created_time":{"before":date(to)?}}),
            ];
            if let Some(filter) = &config.properties {
                filters.push(filter.clone());
            }
            let response = client
                .list(
                    &format!("/data_sources/{source_id}/query"),
                    Some(json!({"filter":{"and":filters}})),
                    cursor.as_deref(),
                )
                .map_err(failed)?;
            for page in response["results"].as_array().unwrap() {
                state.queue.push_back(Task::Page {
                    id: object_id(page).map_err(failed)?,
                });
            }
            if response["request_status"]["type"] == "incomplete" {
                if to - from <= 1 {
                    return Err(failed(
                        "Notion query has too many rows at one creation timestamp; narrow property filters",
                    ));
                }
                let mid = from + (to - from) / 2;
                state.queue.push_back(Task::Query {
                    id: source_id.clone(),
                    cursor: None,
                    seen: BTreeSet::new(),
                    from,
                    to: mid,
                });
                state.queue.push_back(Task::Query {
                    id: source_id,
                    cursor: None,
                    seen: BTreeSet::new(),
                    from: mid,
                    to,
                });
            } else if let Some(next) = next(&response, &mut seen).map_err(failed)? {
                state.queue.push_back(Task::Query {
                    id: source_id,
                    cursor: Some(next),
                    seen,
                    from,
                    to,
                });
            }
        }
    }
    let exhausted = state.queue.is_empty();
    if exhausted {
        state.next_scan = time.saturating_add(config.refresh);
        // Deduplication belongs only to this traversal, not a cross-run inventory.
        state.visited.clear();
    }
    let checkpoint = state.checkpoint()?;
    Ok(DiscoveryPage {
        candidates: read_id
            .into_iter()
            .map(|id| Candidate {
                external_id: id,
                handle: "page".into(),
                cursor_after: checkpoint.clone(),
            })
            .collect(),
        exhausted,
        cursor_after: Some(checkpoint),
    })
}

fn next(response: &Value, seen: &mut BTreeSet<String>) -> Result<Option<String>, String> {
    if response["has_more"] != true {
        return Ok(None);
    }
    let next = response["next_cursor"]
        .as_str()
        .ok_or("missing pagination cursor")?
        .to_string();
    if !seen.insert(next.clone()) {
        return Err("Notion repeated a pagination cursor".into());
    }
    Ok(Some(next))
}

fn object_id(value: &Value) -> Result<String, String> {
    id(value["id"].as_str().ok_or("Notion object has no ID")?)
}

// Keep hierarchy evaluation separate from metadata filters so descendants of a filtered
// container can still be selected. Unknown ancestry is an error, never deletion evidence.
fn ancestry(client: &Client, page: &Value) -> Result<Vec<(String, String, bool)>, String> {
    Ok(nodes(&hierarchy(client, page)?))
}

fn nodes(objects: &[Value]) -> Vec<(String, String, bool)> {
    objects
        .iter()
        .map(|object| {
            (
                object["id"].as_str().unwrap().to_string(),
                object["object"].as_str().unwrap_or("page").to_string(),
                object["in_trash"] == true,
            )
        })
        .collect()
}

pub(crate) fn hierarchy(client: &Client, page: &Value) -> Result<Vec<Value>, String> {
    let mut object = page.clone();
    let mut result = Vec::new();
    let mut seen = BTreeSet::new();
    for _ in 0..50 {
        let current = object_id(&object)?;
        if !seen.insert(current.clone()) {
            return Err("Notion ancestry contains a cycle".into());
        }
        let title = if object["object"] == "page" {
            crate::config::title(&object)
        } else {
            crate::config::rich(&object["title"])
        };
        result.push(json!({"id":current,"object":object["object"],"title":title,"url":object["url"],"parent":object["parent"],"in_trash":object["in_trash"],"created_time":object["created_time"],"last_edited_time":object["last_edited_time"]}));
        let parent = &object["parent"];
        let kind = parent["type"].as_str().ok_or("Notion parent has no type")?;
        if kind == "workspace" {
            return Ok(result);
        }
        let parent_id = id(parent[kind].as_str().ok_or("Notion parent has no ID")?)?;
        let path = match kind {
            "page_id" => "pages",
            "block_id" => "blocks",
            "database_id" => "databases",
            "data_source_id" => "data_sources",
            _ => return Err("unknown Notion parent type".into()),
        };
        object = match client.get_optional(&format!("/{path}/{parent_id}"))? {
            Some(object) => object,
            None => {
                // Sharing a child does not necessarily authorize its ancestors.
                // Keep the public parent ID; never invent missing scope evidence.
                result.push(json!({"id":parent_id,"object":kind.trim_end_matches("_id"),"unavailable":true}));
                return Ok(result);
            }
        };
    }
    Err("Notion ancestry exceeds 50 levels".into())
}

fn under_root(client: &Client, config: &Config, page: &Value) -> Result<bool, String> {
    Ok(ancestry(client, page)?.iter().any(|(id, _, _)| {
        config.roots.contains(id)
            || config.databases.contains(id)
            || config.data_sources.contains(id)
    }))
}

pub(crate) fn selection(
    client: &Client,
    config: &Config,
    page: &Value,
) -> Result<&'static str, String> {
    if page["in_trash"] == true || page["archived"] == true {
        return Ok("trashed");
    }
    let hierarchy = hierarchy(client, page)?;
    let incomplete = hierarchy.iter().any(|object| object["unavailable"] == true);
    let chain = nodes(&hierarchy);
    if chain
        .iter()
        .any(|(id, _, _)| config.exclude_pages.contains(id) || config.exclude_blocks.contains(id))
    {
        return Ok("excluded_branch");
    }
    if chain.iter().any(|(_, _, trashed)| *trashed) {
        return Ok("trashed");
    }
    if incomplete && (!config.exclude_pages.is_empty() || !config.exclude_blocks.is_empty()) {
        return Err("cannot resolve Notion ancestor exclusions; share the relevant ancestors with the integration".into());
    }
    let page_id = object_id(page)?;
    let selected = config.workspace
        || config.pages.contains(&page_id)
        || chain.iter().any(|(id, _, _)| {
            config.roots.contains(id)
                || config.databases.contains(id)
                || config.data_sources.contains(id)
        });
    if !selected {
        if incomplete {
            return Err("cannot resolve Notion root membership; share the relevant ancestors with the integration".into());
        }
        return Ok("outside_scope");
    }
    if !config.matches(page) {
        return Ok("metadata_filter");
    }
    if !config.exclude_blocks.is_empty() {
        for edge in chain.windows(2) {
            if edge[0].1 == "page"
                && edge[1].1 == "page"
                && client.child_page_excluded(&edge[1].0, &edge[0].0, &config.exclude_blocks)?
            {
                return Ok("excluded_branch");
            }
        }
    }
    Ok("included")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::tests::reply;
    const PAGE: &str = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";
    const CHILD: &str = "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb";
    fn page(id: &str) -> Value {
        json!({"object":"page","id":id,"parent":{"type":"workspace","workspace":true},"properties":{"Name":{"type":"title","title":[{"plain_text":"Test"}]}}})
    }
    fn source() -> SourceRef {
        SourceRef {
            source: "notion".into(),
            account: Some("work".into()),
            locator: "notion-scope:test".into(),
            options: vec![],
        }
    }
    #[test]
    fn traverses_search_databases_children_and_excluded_only_pages() {
        let database = "cccccccc-cccc-cccc-cccc-cccccccccccc";
        let data_source = "dddddddd-dddd-dddd-dddd-dddddddddddd";
        let config = Config::parse(&[
            ("account".into(), "work".into()),
            ("workspace".into(), "true".into()),
            ("databases".into(), database.into()),
        ])
        .unwrap();
        let client = Client::fixture(move |path, body| {
            let value = if path == "/search" {
                if body.unwrap().get("start_cursor").is_some() {
                    json!({"results":[],"has_more":false})
                } else {
                    json!({"results":[page(PAGE),{"id":data_source,"object":"data_source"}],"has_more":true,"next_cursor":"search-next"})
                }
            } else if path.contains("/databases/") {
                json!({"data_sources":[{"id":data_source}]})
            } else if path.ends_with("/query") {
                json!({"results":[page(CHILD)],"has_more":false})
            } else if path.starts_with("/pages/") {
                page(if path.contains(PAGE) { PAGE } else { CHILD })
            } else if path.contains(PAGE) {
                json!({"results":[{"id":CHILD,"type":"child_page"},{"id":database,"type":"child_database"}],"has_more":false})
            } else {
                json!({"results":[],"has_more":false})
            };
            reply(200, value)
        });
        let mut cursor = "null".to_string();
        let mut reads = BTreeSet::new();
        let mut exhausted = false;
        for _ in 0..50 {
            let result = advance(&source(), &config, &cursor, 10, &client, 100).unwrap();
            assert!(result.candidates.len() <= 1);
            if let Some(candidate) = result.candidates.first() {
                assert_eq!(candidate.handle, "page");
                reads.insert(candidate.external_id.clone());
            }
            cursor = result.cursor_after.unwrap();
            if result.exhausted {
                exhausted = true;
                break;
            }
        }
        assert!(exhausted);
        assert_eq!(reads.len(), 2);
        assert!(
            advance(&source(), &config, &cursor, 10, &client, 101)
                .unwrap()
                .candidates
                .is_empty()
        );
        let config = Config::parse(&[
            ("account".into(), "work".into()),
            ("workspace".into(), "true".into()),
            ("title_prefix".into(), "No match".into()),
        ])
        .unwrap();
        // Changing filters restarts only discovery; Core owns prior document IDs.
        let result = advance(&source(), &config, &cursor, 10, &client, 102).unwrap();
        let checkpoint = result.cursor_after.unwrap();
        let state = State::load(&checkpoint, &source().locator).unwrap();
        assert!(matches!(state.queue.front(), Some(Task::Page { .. })));
        assert!(
            serde_json::from_str::<Value>(&checkpoint)
                .unwrap()
                .get("known")
                .is_none()
        );
    }
    #[test]
    fn real_candidates_replay_until_acknowledged_without_registration_documents() {
        let config = Config::parse(&[
            ("account".into(), "work".into()),
            ("pages".into(), PAGE.into()),
        ])
        .unwrap();
        let client = Client::fixture(|_, _| reply(200, page(PAGE)));
        let first = advance(&source(), &config, "null", 1, &client, 100).unwrap();
        assert_eq!(first.candidates[0].handle, "page");
        assert_eq!(first.candidates[0].external_id, PAGE);
        let checkpoint = first.cursor_after.as_ref().unwrap();
        assert_eq!(&first.candidates[0].cursor_after, checkpoint);
        let state = State::load(checkpoint, &source().locator).unwrap();
        assert_eq!(state.version, 2);
        assert!(state.visited.is_empty());
        assert_eq!(state.discovery_decisions.back().unwrap().1, "included");
        let replay = advance(&source(), &config, "null", 1, &client, 100).unwrap();
        assert_eq!(
            first.candidates[0].cursor_after,
            replay.candidates[0].cursor_after
        );
        assert!(first.exhausted);
        let terminal = checkpoint;
        assert!(
            advance(&source(), &config, terminal, 1, &client, 101)
                .unwrap()
                .candidates
                .is_empty()
        );
        assert!(
            !advance(&source(), &config, terminal, 1, &client, 4000)
                .unwrap()
                .candidates
                .is_empty()
        );
        assert!(advance(&source(), &config, "null", 0, &client, 100).is_err());
        let broken = Client::fixture(|_, _| reply(401, json!({"private":"secret"})));
        assert!(advance(&source(), &config, "null", 1, &broken, 100).is_err());
        assert!(advance(&source(), &config, "null", 1, &client, 100).is_ok());
    }
    #[test]
    fn ancestry_exclusion_scope_and_metadata_are_independent() {
        let client = Client::fixture(|_, _| reply(200, page(PAGE)));
        let opts = |extra: (&str, &str)| {
            vec![
                ("account".into(), "work".into()),
                ("roots".into(), PAGE.into()),
                (extra.0.into(), extra.1.into()),
            ]
        };
        let mut child = page(CHILD);
        child["parent"] = json!({"type":"page_id","page_id":PAGE});
        let config = Config::parse(&opts(("title_prefix", "No match"))).unwrap();
        assert!(under_root(&client, &config, &child).unwrap());
        assert_eq!(
            selection(&client, &config, &child).unwrap(),
            "metadata_filter"
        );
        let config = Config::parse(&opts(("exclude_pages", PAGE))).unwrap();
        assert_eq!(
            selection(&client, &config, &child).unwrap(),
            "excluded_branch"
        );
        let config = Config::parse(&opts(("pages", CHILD))).unwrap();
        assert_eq!(selection(&client, &config, &child).unwrap(), "included");
        child["in_trash"] = json!(true);
        assert_eq!(selection(&client, &config, &child).unwrap(), "trashed");
        let missing = Client::fixture(|_, _| reply(404, json!({})));
        child["in_trash"] = json!(false);
        assert_eq!(selection(&missing, &config, &child).unwrap(), "included");
        let exclusions = Config::parse(&opts((
            "exclude_pages",
            "cccccccc-cccc-cccc-cccc-cccccccccccc",
        )))
        .unwrap();
        assert!(selection(&missing, &exclusions, &child).is_err());
        let unrelated = Config::parse(&[
            ("account".into(), "work".into()),
            (
                "roots".into(),
                "cccccccc-cccc-cccc-cccc-cccccccccccc".into(),
            ),
        ])
        .unwrap();
        assert!(selection(&missing, &unrelated, &child).is_err());
    }
    #[test]
    fn incomplete_queries_split_without_discarding_pending_rows() {
        let config = Config::parse(&[
            ("account".into(), "work".into()),
            ("data_sources".into(), PAGE.into()),
        ])
        .unwrap();
        let client = Client::fixture(|_, _| {
            reply(
                200,
                json!({"results":[page(CHILD)],"has_more":false,"request_status":{"type":"incomplete"}}),
            )
        });
        let discovery = advance(&source(), &config, "null", 1, &client, 100).unwrap();
        assert!(discovery.candidates.is_empty());
        let state = State::load(&discovery.cursor_after.unwrap(), &source().locator).unwrap();
        assert_eq!(state.queue.len(), 3);
        assert!(matches!(state.queue.front(), Some(Task::Page { .. })));
        assert!(matches!(state.queue.back(),Some(Task::Query {from,..}) if *from > 0));
    }
    #[test]
    fn page_checkpoints_advance_filtered_traversal_without_fake_candidates() {
        let key = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";
        let config = Config::parse(&[
            ("account".into(), "work".into()),
            ("roots".into(), key.into()),
            ("title_prefix".into(), "No match".into()),
        ])
        .unwrap();
        let client = Client::fixture(move |path, _| {
            reply(
                200,
                if path.starts_with("/pages/") {
                    page(PAGE)
                } else {
                    json!({"results":[],"has_more":false})
                },
            )
        });
        let first = advance(&source(), &config, "null", 1, &client, 100).unwrap();
        assert!(first.candidates.is_empty());
        assert!(!first.exhausted);
        let checkpoint = first.cursor_after.unwrap();
        assert_ne!(checkpoint, "null");
        let next = advance(&source(), &config, &checkpoint, 1, &client, 100).unwrap();
        assert!(next.candidates.is_empty());
        assert!(next.exhausted);
        let terminal = next.cursor_after.unwrap();
        assert_ne!(checkpoint, terminal);
        assert!(
            advance(&source(), &config, &terminal, 1, &client, 101)
                .unwrap()
                .cursor_after
                .is_none()
        );
        assert!(State::load(&terminal, "other-workspace").is_err());
        assert!(State::load("{}", "notion-scope:one").is_err());
        assert!(State::load(r#"{"version":1,"known":[]}"#, "workspace").is_err());
    }
    #[test]
    fn rejects_repeated_pagination() {
        let mut seen = BTreeSet::new();
        let response = json!({"has_more":true,"next_cursor":"one"});
        assert_eq!(next(&response, &mut seen).unwrap(), Some("one".into()));
        assert!(next(&response, &mut seen).is_err());
    }
}
