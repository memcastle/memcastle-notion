# Contributing to notion

Thanks for contributing!

Start with the [README](README.md) for installation, packaging and release basics.

## Work on a module

`plugin.toml` is the root descriptor of the plugin and its module IDs and paths.
Keep those IDs stable when changing an implementation.

Rust/Wasm sources live under `sources/<id>/`.
Follow each module's README and tests, commit its `Cargo.lock`,
and build its component to `sources/<id>/source.wasm` before packaging.

## Check a change

Install a plugin-capable MemCastle Core binary with reconciliation extension `0.1.0`
(currently development Core at `c311d09` or newer).
For source and embedding modules, install Rust with the `wasm32-wasip2` target.

Run the affected module's tests and build, then from the project root:

```sh
memcastle plugin inspect --json
memcastle plugin package
memcastle plugin test dist/notion-VERSION.tar.gz
PATH="$PWD/sources/notion/fixtures/bin:$PATH" memcastle source test sources/notion
ARCHIVE=dist/notion-VERSION.tar.gz node scripts/smoke_scoped_miners.mjs
```

Use the actual archive name in `dist/` instead of `VERSION`.
The package command generates the archive's versioned `manifest.toml` and an external checksum;
do not commit generated archives or rewrite `plugin.toml` to add release versions or checksums.

CI builds and tests the configured module types and validates the package against the selected Core matrix.

If you add native tests that produce nonempty LCOV,
update the `codecov_covered_sources` or `codecov_covered_integrations` template answers as applicable
and choose achievable coverage targets.

## Update or release

Template answers and resolved child commits are recorded in `.config/git.tpl.toml`.

To change the module lists or adopt a template update, adjust the answers,
run `git tpl update --defaults --json`, review `git tpl diff --json`,
then apply with `git tpl merge --json`.

Resolve any conflicts as ordinary Git conflicts and rerun the project checks.

Use releasable Conventional Commits for the gh-ship release PR flow.
A reviewed `release/next` PR publishes the validated archive when merged.

The README describes repository credentials and environment setup.
