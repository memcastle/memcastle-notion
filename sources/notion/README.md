# Notion source

A read-only `memcastle:source@0.4.0` adapter for pages, database/data-source rows,
recursive blocks, page properties, links and attachment metadata. Uses the official
Notion API with `Notion-Version: 2026-03-11`. No binary downloads, OCR, writeback or OAuth.

The module exports `reconciled-source` and declares `compatibility.reconciliation = "0.1.0"`.
It requires Core with [memcastle/memcastle#378](https://github.com/memcastle/memcastle/pull/378),
currently the development release at `c311d09` or newer. Older stable 0.5 Core does
not support the extension, including older binaries also reporting version `0.5.2`.

## Set up an integration

1. Create an **internal integration** for your workspace in
   [Notion's integration settings](https://www.notion.so/profile/integrations).
2. Grant **Read content**. Insert/update content, comments and user/email access are
   unnecessary. An integration sees only content explicitly shared with it.
3. Add the integration as a connection on each desired root page and database.
   Confirm child pages and all relevant data sources are accessible; database and
   data-source IDs are different. Copy IDs from their Notion URLs/settings.
4. Put the integration secret in an environment variable available to the daemon,
   or in a private token file, and reference it through Core's source configuration.
5. Install the plugin, explicitly enable `notion`, and ensure `curl` is on the
   daemon's `PATH`.

```sh
memcastle plugin install notion
memcastle source enable notion
```

Credentials are read only through Core's selected-credential host function. There
is no ambient `NOTION_TOKEN` fallback, token option, or fixture authentication bypass.
The only permissions requested are the selected credential and execution of curl.
The transport uses a fixed API origin, rejects redirects and passes the secret in
curl's stdin configuration, never argv. Attachment links are not fetched.

## Configure independent miners

In your MemCastle configuration (replace example IDs with real IDs):

```toml
[[source_configs]]
id = "notion-work"
source = "notion"
credential = { type = "env", name = "NOTION_TOKEN" }
options = { account = "work" }

[[miners]]
name = "engineering"
source = "notion"
source_config = "notion-work"
wing = "engineering"
schedule = { every = "1h", enabled = true }
options = { roots = ["11111111-1111-1111-1111-111111111111"], title_prefix = "RFC", refresh_seconds = "3600" }

[[miners]]
name = "reference"
source = "notion"
source_config = "notion-work"
wing = "reference"
schedule = { every = "1h", enabled = true }
options = { data_sources = ["22222222-2222-2222-2222-222222222222"], property_filter = '{"property":"Status","status":{"equals":"Published"}}' }
```

A file credential uses Core's `credential = { type = "file", path = "/absolute/path/to/token" }`
instead. Keep secrets out of version control and ordinary source/miner options.

`account` is a required stable, non-secret workspace namespace. Omit the locator
or use `workspace`; named miners can share it, the credential and even the destination.
Core partitions their document inventory and checkpoints automatically. Keep the
miner name, account, locator, source-config ID and destination/mapping stable through
filter edits. Renaming a miner or changing those identity selectors creates a separate
history; filter edits preserve identity and reconcile the previous selection.
Legacy `notion-scope:<name>` locators remain accepted, but uniqueness is no longer required.

## Scope and filters

ID lists accept Core string lists, JSON-array strings, or comma-separated UUIDs;
hyphens are optional. Options are validated locally and unknown options are errors.

| Option | Meaning |
| --- | --- |
| `pages` | These pages only; does not automatically select descendants. |
| `roots` | These root pages and descendant pages/contained databases. |
| `databases` | Every data source within these database containers, including multi-source databases. |
| `data_sources` | Row pages in these data sources. |
| `workspace` | `true` explicitly opts into everything visible to the integration; default `false`. |
| `exclude_pages` | Subtract these pages and their descendant branches, including known pages that moved there. |
| `include_blocks` | Only these block IDs and descendants within selected pages; unset/empty means all blocks. |
| `exclude_blocks` | Subtract these block subtrees; overrides inclusion. Also excludes descendant page branches. |
| `title_prefix` | Case-sensitive title prefix. |
| `title_glob` | Case-sensitive glob (`*`, `?`, character classes). |
| `title_regex` | Rust regular expression; no backreferences/lookaround. |
| `property_filter` | JSON property predicate, described below. |
| `refresh_seconds` | New-content discovery interval, 60–2592000 seconds; default 3600. Known pages reconcile every Core cycle. |

At least one selection anchor or `workspace=true` is required. Anchors form a union;
title and property filters intersect that union; excluded branches always win.
Nonmatching container titles/properties do not stop traversal to matching descendants.
Block selection applies to page body blocks; title/properties remain page context.
Selecting a heading block does **not** select its following siblings: only its API
children belong to the subtree. Include/exclude IDs are stable across text edits.

Property predicates use the supported subset of Notion's filter syntax:

```json
{
  "and": [
    {"property": "Status", "status": {"equals": "Published"}},
    {"property": "Tags", "multi_select": {"contains": "knowledge"}},
    {"or": [
      {"property": "Priority", "number": {"greater_than_or_equal_to": 3}},
      {"property": "Pinned", "checkbox": {"equals": true}}
    ]}
  ]
}
```

Supported property types: `title`, `rich_text`, `status`, `select`, `multi_select`,
`checkbox`, `number`. Properties can be named by name or property ID.

- Text: `equals`, `does_not_equal`, `contains`, `does_not_contain`, `starts_with`, `ends_with`.
- Status/select: `equals`, `does_not_equal`.
- Multi-select: `contains`, `does_not_contain`.
- Checkbox: `equals`, `does_not_equal` with booleans.
- Number: equality/inequality and `greater_than`, `less_than`,
  `greater_than_or_equal_to`, `less_than_or_equal_to`.
- All except checkbox: `is_empty` and `is_not_empty` with booleans.
- `and`/`or` compose nonempty predicate arrays, up to 12 nesting levels.

Missing properties/type mismatches do not match, including negative predicates.
Unsupported conditions fail validation. Data-source queries apply the property
predicate server-side; known pages are also rechecked independently and evaluated
locally, so rows that stop matching are retired. Metadata checks precede body fetches
where possible. Stored selection reasons are fixed rule codes (`metadata_filter`,
`excluded_branch`, `outside_scope`, `trashed`, `unavailable`) associated with page IDs;
errors and progress logs do not include page content or credentials. The cursor also
records the last 128 discovery decisions as page-ID/rule-code pairs for checkpoint
inspection, including newly excluded pages; this bounded diagnostic sample resets
with each reconciliation generation.

## Content and provenance

One stable page UUID is one source document. Child pages have separate document
identities. Text normalization preserves headings, lists/todos, code/language,
quotes, callouts, toggles, equations, tables, captions, bookmarks, mentions and links.
Unknown block types preserve available rich text and children. Synced blocks resolve
their original children; inaccessible originals fail safely.

Titles, tags and useful properties are searchable. Structured source metadata keeps
page URL/ID, timestamps, parent hierarchy, database/data-source context, properties,
block IDs/order, references, and attachment metadata/source URLs. Expiring signed
file URL query strings are omitted from persisted provenance and revision identity;
they are not durable download links. Binary attachment contents are not indexed.
An explicitly shared child can be acquired even if its ancestors are not shared;
unavailable ancestors retain their public IDs in provenance. If missing ancestry
prevents proving root membership or applying ancestor exclusions, the run fails
with sharing guidance rather than guessing scope.

## Synchronization and removal

Core walks its durable document inventory before discovery. The source rechecks
known page metadata against current access and filters, returning an explicit
`refresh` or `retire` decision. Included pages refresh their recursive block bodies
even if page timestamps are unchanged, so independently edited nested blocks are
detected. Content-derived revisions make unchanged ingestion idempotent. The miner's
schedule controls reconciliation frequency; `refresh_seconds` controls how often
completed discovery restarts to find new pages in roots, data sources and search.
Interrupted discovery and Core inventory walks resume across document-limited jobs.

Notion search is eventually consistent and not an exhaustive inventory. Newly shared
old pages are discovered on subsequent scans even when their timestamps are old.
Absence from search/query results never proves deletion. Direct object-specific 404,
trash state, or confirmed scope/filter changes return explicit retirement decisions
and Core closes the current chunks. Global auth errors, rate limits, and unresolved scope-critical ancestry
fail the run instead of causing mass retirement. Retired chunks remain historical
Core records; retirement is not historical erasure. Restored pages can be mined again.

Core registers each discovered document ID before acquisition, retaining pending
IDs after failed or interrupted first ingestion. The plugin's version-2 cursor holds
only discovery traversal/deduplication and bounded diagnostics, never the persistent
document inventory, credentials or page bodies. Empty filtered pages and scan
completion use page-level checkpoints instead of synthetic document candidates.
Actual acquisition and inventory checks count toward Core's document limit; empty
progress pages do not. No zero-segment tombstones or cursor-registration documents
are generated.

Use an ordinary full run to force early rediscovery:

```sh
memcastle miner run engineering --full
```

Core retains its inventory across `--full` and filter edits, so inaccessible or
out-of-scope previously mined pages can still be retired. Continue a truncated full
scan with normal runs rather than starting another full run on every continuation.
Scope changes still use Core's standard `allow_broaden` acknowledgement where required.
Core serializes concurrent jobs for one effective source and transactionally fences
canonical writes; independent miners can run concurrently and share credentials.

### Upgrading from the cursor-inventory implementation

Opting into Core reconciliation changes named-miner storage identities. Previously
mined legacy states remain in the palace; they are not automatically claimed by the
new named-miner identity, and a full run of the new identity does not clean up the
old one. Plan an explicit transition of those histories before relying on retirement
of content mined by an older archive. Retain the previous archive/checkpoints until
that transition is complete. No migration of a real palace is performed by this plugin.
Remove the old `rescan` miner option; `--full` replaces it. Existing scope locators
remain valid, but new miners can simply omit them. Version-1 provider cursors are
not reused as version-2 discovery checkpoints.

### Operating bounds

- Calls have a 40-second acquisition budget; requests use bounded retry/backoff.
  HTTP 429/529 respects `Retry-After`. If the wait cannot fit, the job fails with the
  minimum retry delay; retry after that delay through your schedule/operator.
- Maximum response size: 8 MiB; page acquisition: 20000 blocks/items and 50 levels;
  checkpoint: 4 MiB. Oversized content fails without truncation/checkpoint advancement.
  Divide large discovery scopes/pages rather than silently truncating traversal.
- Incomplete data-source queries are recursively partitioned by creation timestamp.
  An unsplittable millisecond containing too many rows fails with a request to narrow
  property filters, rather than silently declaring the data source fully scanned.

## Build and verify

From the repository root:

```sh
cargo generate-lockfile --manifest-path sources/notion/Cargo.toml
cargo fmt --manifest-path sources/notion/Cargo.toml --all -- --check
cargo clippy --manifest-path sources/notion/Cargo.toml --locked --target wasm32-wasip2 -- -D warnings
cargo test --manifest-path sources/notion/Cargo.toml --locked --lib
cargo build --manifest-path sources/notion/Cargo.toml --locked --release --target wasm32-wasip2
cp sources/notion/target/wasm32-wasip2/release/notion.wasm sources/notion/source.wasm
memcastle plugin inspect --json
memcastle plugin package
memcastle plugin test dist/notion-VERSION.tar.gz
PATH="$PWD/sources/notion/fixtures/bin:$PATH" memcastle source test sources/notion
ARCHIVE=dist/notion-VERSION.tar.gz node scripts/smoke_scoped_miners.mjs
```

Install the `wasm32-wasip2` target and use reconciliation-capable Core for all
publisher/test commands, as well as the daemon. Commit `Cargo.lock`.
The smoke suite requires Node.js 24+ and Python 3, uses real Wasm and an isolated Core
daemon, and replaces curl with offline scenarios. Generic `memcastle source test`
uses the opaque `workspace` locator and `[test] fixture_credential = true`, which
supplies only Core's fixed synthetic token `memcastle-source-test-credential`.
The fake transport also accepts the smoke suite's dummy selected token; production
acquisition has no credential bypass. CI runs conformance and installed-archive
lifecycle checks on development Core until a stable release includes the extension.

If access fails, check that the credential is available **to the daemon**, the
integration has Read content, pages/databases are shared, and IDs select the correct
object kind. HTTP 401/403 fails the run without retiring content. Object-specific
404 can mean missing sharing as well as deletion. Rate-limit errors report a retry
delay, not the API response body. Retry without resetting checkpoints.
