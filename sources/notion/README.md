# notion

Text files from a selected directory.

This is a separately activatable MemCastle source module using the `memcastle:source@0.4.0` WIT world.
Build with `cargo build --release --target wasm32-wasip2` after installing the target.
Run `cargo fmt --all -- --check` and `cargo clippy --locked --target wasm32-wasip2 -- -D warnings` locally.
Commit `Cargo.lock` after the initial `cargo generate-lockfile` so CI and releases build the same dependency graph.
Run `memcastle source test .` to check the incremental text-file fixture against the real host.
Package this module with its containing provider plugin; installing that plugin does not enable this source.
