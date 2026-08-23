# Aerini Plugin Template

A minimal starting point for writing a Aerini plugin node in Rust.

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
target/wasm32-wasip2/release/aerini_plugin_echo.wasm
```

## Install

Copy the `.wasm` file to your Aerini plugin directory:

- **Desktop app:** open the **Plugins** tab in the left activity bar. Click **Browse** to set (or change) your plugin directory, then **Install .wasm** to copy a compiled plugin into it. Installed plugins are listed with their type id and a **Remove** button.
- **aerini-server:** the directory passed via `--plugin-dir`.

No restart needed — Aerini reloads the plugin registry live after install or remove. The new (or updated) node type appears in the palette immediately.

## Customising

1. Rename the crate in `Cargo.toml`.
2. Update `describe()` with your node's `type_id`, `display_name`, `category`, `description`, `input_schema`, and `output_schema`.
3. Implement `execute()` with your node's logic.
4. Rebuild and copy the `.wasm` file.

See [docs/plugin-authoring.md](../../docs/plugin-authoring.md) for a complete guide.
