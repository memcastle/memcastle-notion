//! Notion acquisition over Core's explicit reconciliation contract.

mod api;
mod config;
mod discovery;
mod render;

use exports::memcastle::source::{
    adapter::Guest as Adapter,
    reconciliation::{
        Acquisition, Decision, DiscoveryPage, Guest as Reconciliation, KnownDocument,
    },
};
use memcastle::source::types::{
    Candidate, CanonicalDocument, Discovery, RawDocument, SourceError, SourceRef,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

wit_bindgen::generate!({ path: "wit", world: "reconciled-source" });

struct Source;

fn hash(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}
fn failed(error: impl Into<String>) -> SourceError {
    SourceError::Failed(error.into())
}
fn config(source: &SourceRef) -> Result<config::Config, SourceError> {
    config::Config::parse(&source.options).map_err(SourceError::InvalidInput)
}
fn extension_required() -> SourceError {
    failed(
        "Notion requires Core's reconciliation extension 0.1.0; use the current development Core and the unmodified source manifest",
    )
}

impl Adapter for Source {
    fn identify(
        locator: Option<String>,
        options: Vec<(String, String)>,
    ) -> Result<SourceRef, SourceError> {
        let locator = locator.unwrap_or_else(|| "workspace".into());
        // Retain existing locator spellings, but miner isolation is now Core's job.
        if locator != "workspace" {
            let name = locator.strip_prefix("notion-scope:").unwrap_or("");
            if name.is_empty()
                || name.len() > 100
                || !name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
            {
                return Err(SourceError::InvalidInput("omit the locator or use workspace (legacy notion-scope:<name> locators are also accepted)".into()));
            }
        }
        let config = config::Config::parse(&options).map_err(SourceError::InvalidInput)?;
        Ok(SourceRef {
            source: "notion".into(),
            account: Some(config.account),
            locator,
            options,
        })
    }
    fn default_wing(source: SourceRef) -> String {
        source
            .locator
            .strip_prefix("notion-scope:")
            .unwrap_or("notion")
            .to_string()
    }
    fn default_room() -> String {
        "pages".into()
    }
    // A stripped/misdeclared manifest must not silently lose retirement semantics.
    fn discover(_: SourceRef, _: String, _: u32) -> Result<Discovery, SourceError> {
        Err(extension_required())
    }
    fn read(_: SourceRef, _: Candidate) -> Result<Option<RawDocument>, SourceError> {
        Err(extension_required())
    }
    fn normalize(raw: RawDocument) -> Result<CanonicalDocument, SourceError> {
        render::normalize(raw).map_err(failed)
    }
}

impl Reconciliation for Source {
    fn discover(
        source: SourceRef,
        cursor: String,
        limit: u32,
    ) -> Result<DiscoveryPage, SourceError> {
        discovery::discover(&source, &config(&source)?, &cursor, limit)
    }
    fn reconcile(source: SourceRef, known: KnownDocument) -> Result<Decision, SourceError> {
        reconcile(
            &api::Client::new().map_err(failed)?,
            &config(&source)?,
            &known,
        )
    }
    fn acquire(source: SourceRef, candidate: Candidate) -> Result<Acquisition, SourceError> {
        acquire(
            &api::Client::new().map_err(failed)?,
            &config(&source)?,
            &candidate,
        )
    }
}

fn selected_page(
    client: &api::Client,
    config: &config::Config,
    external_id: &str,
) -> Result<Result<Value, String>, SourceError> {
    let id = config::id(external_id).map_err(SourceError::InvalidInput)?;
    let Some(mut page) = client
        .get_optional(&format!("/pages/{id}"))
        .map_err(failed)?
    else {
        return Ok(Err("unavailable".into()));
    };
    if config::id(page["id"].as_str().unwrap_or("")).map_err(failed)? != id {
        return Err(failed("Notion returned a different page ID"));
    }
    client.complete_properties(&mut page).map_err(failed)?;
    let reason = discovery::selection(client, config, &page).map_err(failed)?;
    if reason == "included" {
        Ok(Ok(page))
    } else {
        Ok(Err(reason.into()))
    }
}

fn reconcile(
    client: &api::Client,
    config: &config::Config,
    known: &KnownDocument,
) -> Result<Decision, SourceError> {
    match selected_page(client, config, &known.external_id)? {
        // Refresh the tree even if the page edit timestamp is unchanged: nested
        // blocks can change independently. Core skips identical revisions.
        Ok(_) => Ok(Decision::Refresh("page".into())),
        Err(reason) => Ok(Decision::Retire(reason)),
    }
}

fn acquire(
    client: &api::Client,
    config: &config::Config,
    candidate: &Candidate,
) -> Result<Acquisition, SourceError> {
    // Recheck scope/access after discovery/reconciliation, which may race a move,
    // filter edit, trash operation or sharing revocation.
    let page = match selected_page(client, config, &candidate.external_id)? {
        Ok(page) => page,
        Err(reason) => return Ok(Acquisition::Retire(reason)),
    };
    let id = config::id(&candidate.external_id).map_err(SourceError::InvalidInput)?;
    let blocks = client.tree(&id, config).map_err(failed)?;
    let hierarchy = discovery::hierarchy(client, &page).map_err(failed)?;
    let document = json!({"page":page,"blocks":blocks,"hierarchy":hierarchy,"selection":"included","format":1});
    Ok(Acquisition::Document(RawDocument {
        external_id: id,
        revision: hash(&render::revision_value(&document).to_string()),
        body: document.to_string(),
        metadata: render::provenance(&document).to_string(),
        occurred_at: page["created_time"].as_str().map(str::to_string),
    }))
}

export!(Source);

#[cfg(test)]
mod tests {
    use super::*;
    use api::tests::reply;
    const PAGE: &str = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";
    fn options() -> Vec<(String, String)> {
        vec![
            ("account".into(), "work".into()),
            ("pages".into(), PAGE.into()),
        ]
    }
    fn known() -> KnownDocument {
        KnownDocument {
            external_id: PAGE.into(),
            revision: "old".into(),
            metadata: "{}".into(),
            pending: false,
            retired: false,
        }
    }
    fn page() -> Value {
        json!({"object":"page","id":PAGE,"parent":{"type":"workspace","workspace":true},"properties":{"Name":{"type":"title","title":[{"plain_text":"Reference"}]}}})
    }
    #[test]
    fn core_isolates_miners_so_a_unique_locator_is_not_required() {
        let source = Source::identify(None, options()).unwrap();
        assert_eq!(source.locator, "workspace");
        assert_eq!(source.account.as_deref(), Some("work"));
        assert_eq!(Source::default_wing(source.clone()), "notion");
        assert_eq!(Source::default_room(), "pages");
        let legacy = Source::identify(Some("notion-scope:project".into()), options()).unwrap();
        assert_eq!(Source::default_wing(legacy), "project");
        for locator in ["/tmp/files", "notion-scope:", "notion-scope:bad/name"] {
            assert!(Source::identify(Some(locator.into()), options()).is_err());
        }
        assert!(<Source as Adapter>::discover(source.clone(), "null".into(), 1).is_err());
        assert!(
            Source::read(
                source,
                Candidate {
                    external_id: PAGE.into(),
                    handle: "page".into(),
                    cursor_after: "null".into()
                }
            )
            .is_err()
        );
    }
    #[test]
    fn retirement_is_explicit_and_transient_failures_never_become_deletions() {
        let config = config::Config::parse(&options()).unwrap();
        let candidate = Candidate {
            external_id: PAGE.into(),
            handle: "page".into(),
            cursor_after: "null".into(),
        };
        let missing = api::Client::fixture(|_, _| reply(404, json!({})));
        assert!(
            matches!(reconcile(&missing,&config,&known()).unwrap(),Decision::Retire(reason) if reason == "unavailable")
        );
        assert!(
            matches!(acquire(&missing,&config,&candidate).unwrap(),Acquisition::Retire(reason) if reason == "unavailable")
        );
        let denied = api::Client::fixture(|_, _| reply(403, json!({"private":"secret"})));
        assert!(reconcile(&denied, &config, &known()).is_err());
        assert!(acquire(&denied, &config, &candidate).is_err());
        let excluded = api::Client::fixture(|_, _| {
            let mut p = page();
            p["in_trash"] = json!(true);
            reply(200, p)
        });
        assert!(
            matches!(reconcile(&excluded,&config,&known()).unwrap(),Decision::Retire(reason) if reason == "trashed")
        );
    }
    #[test]
    fn pending_and_retired_records_refresh_without_timestamp_or_metadata_churn() {
        let config = config::Config::parse(&options()).unwrap();
        let client = api::Client::fixture(|path, _| {
            reply(
                200,
                if path.starts_with("/pages/") {
                    page()
                } else {
                    json!({"results":[],"has_more":false})
                },
            )
        });
        let mut record = known();
        record.pending = true;
        record.retired = true;
        assert!(matches!(
            reconcile(&client, &config, &record).unwrap(),
            Decision::Refresh(_)
        ));
        let candidate = Candidate {
            external_id: PAGE.into(),
            handle: "page".into(),
            cursor_after: "null".into(),
        };
        let Acquisition::Document(first) = acquire(&client, &config, &candidate).unwrap() else {
            panic!("expected document")
        };
        let Acquisition::Document(second) = acquire(&client, &config, &candidate).unwrap() else {
            panic!("expected document")
        };
        assert_eq!(first.revision, second.revision);
        assert_eq!(first.metadata, second.metadata);
        assert!(!Source::normalize(first).unwrap().segments.is_empty());
    }
}
