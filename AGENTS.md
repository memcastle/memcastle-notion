# Agent guidance: notion

- Read `README.md` for installation and build steps and `CONTRIBUTING.md` for the project workflow.
  Follow module-local instructions for module-specific work.
- Treat `plugin.toml` as the descriptor of module paths and identities;
  it is not the versioned archive manifest.
  Do not add packaged versions or checksums to it.
- Source implementations and tests live under `sources/<id>/`.
  Commit `Cargo.lock` and build `source.wasm` before packaging.
- This render has no agent integrations.
- This render has no embedding providers.
- Check affected modules, then run `memcastle plugin package` and `memcastle plugin test`
  on the resulting archive. Use a plugin-capable Core binary.
  Keep the coverage-reporting module lists in template answers
  in sync with modules that produce nonempty LCOV.
- Template answers and child revisions live in `.config/git.tpl.toml`;
  inspect `git tpl diff --json` before merging an update.
  Preserve provider-specific changes while resolving Git conflicts.
