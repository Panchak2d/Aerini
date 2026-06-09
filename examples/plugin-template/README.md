# Flowo Plugin Template

A minimal starting point for writing a Flowo plugin node in Rust.

This template implements a single **Echo** node that returns whatever you pass into the `message` parameter. Replace `EchoPlugin` with your own logic.

---

## Prerequisites

- Rust stable (1.82 or later)
- The `wasm32-wasip2` target:
  ```bash
  rustup target add wasm32-wasip2
  ```

## Build

```bash
cargo build --target wasm32-wasip2 --release
```

The compiled plugin is at:
```
target/wasm32-wasip2/release/flowo_plugin_echo.wasm
```

## Install

Copy the `.wasm` file to your Flowo plugin directory:

- **Desktop app:** the path stored in the `plugin_dir` key of the settings database. There is no UI for this yet — set it directly in the database or via a future settings panel.
- **flowo-server:** the directory passed via `--plugin-dir`.

Restart Flowo (or the server). The new node type appears in the palette automatically.

## Customising

1. Rename the crate in `Cargo.toml`.
2. Update `describe()` with your node's `type_id`, `display_name`, `category`, `description`, `input_schema`, and `output_schema`.
3. Implement `execute()` with your node's logic.
4. Rebuild and copy the `.wasm` file.

See [docs/plugin-authoring.md](../../docs/plugin-authoring.md) for a complete guide.
