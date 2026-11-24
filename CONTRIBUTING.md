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

Use `mise tasks ls` to discover root module tasks for `build`, `test`, `lint`, and `format:check`.
See the [README](README.md#root-module-tasks) for typed task names and module setup prerequisites.


Install a plugin-capable MemCastle Core binary.
For source and embedding modules, install Rust 1.100+ with `wasm32-wasip3`,
wasm-tools 1.263.0, Python 3.11+, and migrated development Core from `828f0f6` or a descendant.
Stable Core 0.5.2 predates the WASI 0.3 migration despite matching the transitional compatibility range.

Run the affected module's tests and build, then from the project root:

```sh
memcastle plugin inspect --json
memcastle plugin package
memcastle plugin test dist/notion-VERSION.tar.gz
```

Use the actual archive name in `dist/` instead of `VERSION`.
The package command generates the archive's versioned `manifest.toml` and an external checksum;
do not commit generated archives or rewrite `plugin.toml` to add release versions or checksums.

CI builds and tests the configured module types and validates the package against the selected Core matrix.
Validate built components with `scripts/validate_component.py` as described in the README.
Run each source's `memcastle source test` fixtures against the actual host.


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

