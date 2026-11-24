# notion

Text files from a selected directory.

This is a separately activatable MemCastle source module using the `memcastle:source@0.5.0` WIT world.
Requires Rust 1.100 or later and `rustup target add wasm32-wasip3` for WASI 0.3.0 components.
Build with `cargo build --locked --release --target wasm32-wasip3`.
Run `cargo fmt --all -- --check` and `cargo clippy --locked --target wasm32-wasip3 -- -D warnings` locally.
Commit `Cargo.lock` after the initial `cargo generate-lockfile` so CI and releases build the same dependency graph.
Run `memcastle source test .` to check the incremental text-file fixture against the real host.
Validate the built component with wasm-tools 1.263.0:

```sh
wasm-tools validate --features cm-async,cm-async-stackful,cm-more-async-builtins \
  target/wasm32-wasip3/release/notion.wasm
```

Use migrated development Core from commit `828f0f6` or a descendant, with its Wasmtime 50.0.3 runtime.
The stable 0.5.2 release predates this migration even though development currently reports the same version.
The `>=0.5, <0.6` default is a transitional range, not evidence that older stable builds support this component.
Tighten the range when the first compatible stable Core release is available.
The async WIT exports use stackful `sync: true` bindings so the implementation can keep ordinary synchronous Rust I/O.
WASI 0.3 support is experimental; it does not enable `std::thread::spawn`.
An external WASI SDK is only needed for builds that explicitly depend on it.

Package this module with its containing provider plugin; installing that plugin does not enable this source.
