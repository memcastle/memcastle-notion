//! Read-only Notion acquisition. Durable state belongs to Core's acknowledged cursor.

mod api;
mod config;
mod discovery;
mod render;

use exports::memcastle::source::adapter::Guest;
use memcastle::source::types::{
    Candidate, CanonicalDocument, Discovery, RawDocument, SourceError, SourceRef,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

wit_bindgen::generate!({ path: "wit", world: "source" });

struct Source;

fn hash(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}

fn failed(error: impl Into<String>) -> SourceError {
    SourceError::Failed(error.into())
}

impl Guest for Source {
    fn identify(
        locator: Option<String>,
        options: Vec<(String, String)>,
    ) -> Result<SourceRef, SourceError> {
        let locator = locator.ok_or_else(|| {
            SourceError::InvalidInput("set a unique stable notion-scope:<name> locator".into())
        })?;
        let name = locator.strip_prefix("notion-scope:").unwrap_or("");
        if name.is_empty()
            || name.len() > 100
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
        {
            return Err(SourceError::InvalidInput(
                "locator must be notion-scope:<name> (letters, digits, -, _, .)".into(),
            ));
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

    fn discover(source: SourceRef, cursor: String, limit: u32) -> Result<Discovery, SourceError> {
        let config = config::Config::parse(&source.options).map_err(SourceError::InvalidInput)?;
        discovery::discover(&source, &config, &cursor, limit)
    }

    fn read(source: SourceRef, candidate: Candidate) -> Result<Option<RawDocument>, SourceError> {
        if candidate.handle == "progress" {
            return Ok(None);
        }
        let config = config::Config::parse(&source.options).map_err(SourceError::InvalidInput)?;
        let id = config::id(&candidate.external_id).map_err(SourceError::InvalidInput)?;
        let client = api::Client::new().map_err(failed)?;
        let page = match client
            .get_optional(&format!("/pages/{id}"))
            .map_err(failed)?
        {
            Some(page) => page,
            None => return Ok(Some(tombstone(&id, "unavailable"))),
        };
        let mut page = page;
        if config::id(page["id"].as_str().unwrap_or("")).map_err(failed)? != id {
            return Err(failed("Notion returned a different page ID"));
        }
        client.complete_properties(&mut page).map_err(failed)?;
        let reason = discovery::selection(&client, &config, &page).map_err(failed)?;
        if reason != "included" {
            return Ok(Some(tombstone(&id, reason)));
        }
        let blocks = client.tree(&id, &config).map_err(failed)?;
        let hierarchy = discovery::hierarchy(&client, &page).map_err(failed)?;
        let metadata = json!({"page": page, "blocks": blocks, "hierarchy":hierarchy, "selection": "included", "format": 1});
        // Signed file URLs rotate without content changes. Canonicalize only the revision input.
        let revision = hash(&render::revision_value(&metadata).to_string());
        Ok(Some(RawDocument {
            external_id: id,
            revision,
            body: metadata.to_string(),
            metadata: render::provenance(&metadata).to_string(),
            occurred_at: page
                .get("created_time")
                .and_then(Value::as_str)
                .map(str::to_string),
        }))
    }

    fn normalize(raw: RawDocument) -> Result<CanonicalDocument, SourceError> {
        render::normalize(raw).map_err(failed)
    }
}

fn tombstone(id: &str, reason: &str) -> RawDocument {
    let metadata =
        json!({"id": id, "lifecycle": "tombstone", "reason": reason, "format": 1}).to_string();
    RawDocument {
        external_id: id.into(),
        revision: hash(&metadata),
        body: String::new(),
        metadata,
        occurred_at: None,
    }
}

export!(Source);

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_stable_identity_and_progress_has_no_side_effects() {
        let opts = vec![
            ("account".into(), "work".into()),
            ("workspace".into(), "true".into()),
        ];
        for locator in [
            None,
            Some("/tmp/files".into()),
            Some("notion-scope:".into()),
            Some("notion-scope:bad/name".into()),
        ] {
            assert!(Source::identify(locator, opts.clone()).is_err());
        }
        let source = Source::identify(Some("notion-scope:project".into()), opts).unwrap();
        assert_eq!(source.account.as_deref(), Some("work"));
        assert_eq!(Source::default_wing(source.clone()), "project");
        assert_eq!(Source::default_room(), "pages");
        assert!(
            Source::read(
                source,
                Candidate {
                    external_id: "notion-progress".into(),
                    handle: "progress".into(),
                    cursor_after: "null".into()
                }
            )
            .unwrap()
            .is_none()
        );
        assert!(
            Source::normalize(tombstone("page", "trashed"))
                .unwrap()
                .segments
                .is_empty()
        );
    }
}
