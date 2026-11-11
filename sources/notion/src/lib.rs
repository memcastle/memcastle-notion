//! A minimal incremental MemCastle source that reads `.txt` files from its locator.

use serde_json::{Value, json};
use std::path::Path;

wit_bindgen::generate!({ path: "wit", world: "source" });

use exports::memcastle::source::adapter::Guest;
use memcastle::source::types::{
    Candidate, CanonicalDocument, Discovery, RawDocument, Segment, SourceError, SourceKind,
    SourceRef,
};

struct Source;

impl Guest for Source {
    fn identify(
        locator: Option<String>,
        options: Vec<(String, String)>,
    ) -> Result<SourceRef, SourceError> {
        if !options.is_empty() {
            return Err(SourceError::InvalidInput(
                "this source accepts no options".into(),
            ));
        }
        let locator =
            locator.ok_or_else(|| SourceError::InvalidInput("give a directory to mine".into()))?;
        if !Path::new(&locator).is_dir() {
            return Err(SourceError::InvalidInput(format!(
                "{locator} is not a readable directory"
            )));
        }
        Ok(SourceRef {
            source: "notion".into(),
            account: None,
            locator,
            options,
        })
    }

    fn default_wing(source: SourceRef) -> String {
        Path::new(&source.locator).file_name().map_or_else(
            || "files".into(),
            |name| name.to_string_lossy().into_owned(),
        )
    }

    fn default_room() -> String {
        "notes".into()
    }

    fn discover(source: SourceRef, cursor: String, limit: u32) -> Result<Discovery, SourceError> {
        let last: Value = serde_json::from_str(&cursor)
            .map_err(|_| SourceError::CursorInvalid("invalid JSON cursor".into()))?;
        let after = if last.is_null() {
            None
        } else {
            Some(
                last.get("after")
                    .and_then(Value::as_str)
                    .ok_or_else(|| SourceError::CursorInvalid("missing `after`".into()))?,
            )
        };
        let mut names: Vec<_> = std::fs::read_dir(&source.locator)
            .map_err(|e| SourceError::Failed(e.to_string()))?
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|name| name.ends_with(".txt") && after.is_none_or(|last| name.as_str() > last))
            .collect();
        names.sort();
        let exhausted = names.len() <= limit as usize;
        names.truncate(limit as usize);
        Ok(Discovery {
            candidates: names
                .into_iter()
                .map(|name| Candidate {
                    cursor_after: json!({"after": name}).to_string(),
                    handle: name.clone(),
                    external_id: name,
                })
                .collect(),
            exhausted,
        })
    }

    fn read(source: SourceRef, candidate: Candidate) -> Result<Option<RawDocument>, SourceError> {
        let path = Path::new(&source.locator).join(&candidate.handle);
        let Ok(body) = std::fs::read_to_string(&path) else {
            return Ok(None);
        };
        // A content hash, rather than the file's timestamp, detects edits without needless re-ingestion.
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for byte in body.bytes() {
            hash = (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3);
        }
        Ok(Some(RawDocument {
            external_id: candidate.external_id,
            revision: format!("{hash:016x}"),
            body,
            metadata: json!({"path": path.display().to_string()}).to_string(),
            occurred_at: None,
        }))
    }

    fn normalize(raw: RawDocument) -> Result<CanonicalDocument, SourceError> {
        let metadata: Value = serde_json::from_str(&raw.metadata).unwrap_or(Value::Null);
        Ok(CanonicalDocument {
            title: Some(raw.external_id),
            room: None,
            name: None,
            kind: SourceKind::File,
            uri: metadata
                .get("path")
                .and_then(Value::as_str)
                .map(str::to_string),
            tags: Vec::new(),
            segments: vec![Segment { text: raw.body }],
        })
    }
}

export!(Source);
