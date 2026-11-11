# notion

[![CI][ci-badge]][ci-workflow] [![Codecov][codecov-badge]][codecov-project]

[ci-badge]: https://github.com/memcastle/memcastle-notion/actions/workflows/ci.yaml/badge.svg?branch=main
[ci-workflow]: https://github.com/memcastle/memcastle-notion/actions/workflows/ci.yaml
[codecov-badge]: https://codecov.io/gh/memcastle/memcastle-notion/graph/badge.svg
[codecov-project]: https://codecov.io/gh/memcastle/memcastle-notion

Notion source for MemCastle

Mine shared Notion pages, recursive block trees, databases and data sources into
MemCastle. The independently versioned `notion` plugin contains the separately
enabled `notion` source. It uses the official read-only API, stable page identities,
durable incremental checkpoints and periodic reconciliation of previously mined content.
Core owns the document inventory, retirement, miner isolation and source-write fencing
through the optional reconciliation extension `0.1.0`.

See the [source guide](sources/notion/README.md) for integration setup, scoped miners,
filters, supported content and synchronization behavior.

## Install

If this plugin has a published release in the
[MemCastle plugin catalogue](https://memcastle.github.io/plugins.json),
install it by ID with a plugin-capable MemCastle Core binary:

```sh
memcastle plugin install notion
```

Core retrieves and checks the release archive for you.

This implementation requires reconciliation-capable Core, currently the
[development release](https://github.com/memcastle/memcastle/releases/tag/development)
at `c311d09` or newer. Stable Core 0.5 releases predating that change are unsupported,
even when they report the same `0.5.2` version. Use that Core for publisher commands too.

If the catalogue entry has no published release yet, build and package the project first (below).

After installation, explicitly enable the sources you want to use.

```sh
memcastle source enable notion
```

The daemon needs `curl` on its `PATH`. Create an internal Notion integration with
**Read content**, share the desired pages/data sources with it, and select its token
using an env/file credential reference in a source configuration. Credentials are
never miner options. Named miners can share the same workspace locator and credential;
Core keeps their state independent. Mining the whole visible workspace is explicit opt-in.

## Build and test locally

You'll need a plugin-capable MemCastle Core binary and Git.

Install Rust with the `wasm32-wasip2` target for source and embedding modules.

For each source, run its tests, build the Wasm component and copy the compiled component
to `sources/<id>/source.wasm`.
See the module's README and the CI workflow for its crate-specific artifact name:

```sh
cargo generate-lockfile --manifest-path sources/<id>/Cargo.toml
cargo test --manifest-path sources/<id>/Cargo.toml --locked --lib
cargo build --manifest-path sources/<id>/Cargo.toml --locked --release --target wasm32-wasip2
```

For this plugin the module ID and artifact name are both `notion`:

```sh
cp sources/notion/target/wasm32-wasip2/release/notion.wasm sources/notion/source.wasm
```

From the project root, package and smoke-test the result:

```sh
memcastle plugin inspect --json
memcastle plugin package
memcastle plugin test dist/notion-VERSION.tar.gz
PATH="$PWD/sources/notion/fixtures/bin:$PATH" memcastle source test sources/notion
ARCHIVE=dist/notion-VERSION.tar.gz node scripts/smoke_scoped_miners.mjs
```

The offline smoke suite needs Node.js 24+ and Python 3. It installs the real Wasm
archive into an isolated Core daemon with fixture credentials and a fake curl.
No live Notion integration is needed. The generic source runner uses an opaque API
locator and Core's fixed synthetic selected credential. The installed-archive suite
also covers full rescans, retirement, restart recovery and concurrent miners.

Replace `VERSION` with the version printed by `memcastle plugin package`
or the name of the archive in `dist/`.

The package command reads `plugin.toml` and creates an archive with its own versioned `manifest.toml`
plus a matching `.sha256sum` file.
It does not rewrite `plugin.toml` or create a root-level `manifest.toml`.

A `VERSION` SemVer tag supplies a release version;
untagged Git commits get a development version.
Outside a Git checkout, pass `--version` when packaging.

## Project layout

| Path | Purpose |
| --- | --- |
| `plugin.toml` | Plugin identity, supported Core versions and module entries. |
| `sources/<id>/` | Rust/Wasm source modules, each with its own manifest and tests. |
| `dist/` | Generated release archive and checksum after packaging. |

## CI and releases

CI builds the modules that exist, packages the plugin and validates its archive against MemCastle Core.

The `core_versions` answer currently selects only `development`, which provides the
required reconciliation extension. Add stable Core coverage when the feature is released there.
Validation runs on pull requests and pushes to `main`.

When modules have native tests, their jobs upload available LCOV and JUnit results to Codecov using OIDC.
Enable this repository in Codecov to see coverage and test-result statuses.

Project and patch targets are `80%` and `80%`;
reporting sources and integrations use `80%` and `80%`.
Only modules listed in `codecov_covered_sources` or `codecov_covered_integrations`
have blocking component coverage checks.

Releases use gh-ship and git-cliff:
merging a releasable Conventional Commit into `main` opens a `release/next` PR.
Merging the release PR publishes a validated `notion-VERSION.tar.gz` archive under the `VERSION` Git tag.

Configure the `release` environment with variable `APP_CLIENT_ID`
and secret `APP_PRIVATE_KEY` for an App installed on this repository.
The publish workflow uses this credential to check the draft release and upload its validated archive.
Grant contents, actions and pull requests write permission;
issues write is needed to create the release label.

Set squash-merge title to the PR title and body to blank, and enable the release environment.
The `prepare-release` workflow's `dry_run` input avoids pushing or committing.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for local development and release guidance.
Coding agents should also read [AGENTS.md](AGENTS.md).
