use std::time::Instant;

use wasmtime::component::{Component, Linker, ResourceTable};
use wasmtime::{Config, Engine, Store};
use wasmtime_wasi::{WasiCtx, WasiCtxView, WasiView};

mod bindings {
    wasmtime::component::bindgen!({
        world: "spike-trigger",
        path: "../guest/wit",
    });
}

struct HostState {
    wasi_ctx: WasiCtx,
    table: ResourceTable,
}

impl WasiView for HostState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView { ctx: &mut self.wasi_ctx, table: &mut self.table }
    }
}

#[tokio::main]
async fn main() -> wasmtime::Result<()> {
    let mut config = Config::new();
    config.wasm_component_model(true);
    config.async_support(true);
    config.wasm_component_model_async(true);
    let engine = Engine::new(&config)?;

    let mut linker: Linker<HostState> = Linker::new(&engine);
    wasmtime_wasi::p3::add_to_linker(&mut linker)?;

    let component = Component::from_file(
        &engine,
        "../guest/target/wasm32-wasip2/release/aerini_spike_trigger_guest.wasm",
    )?;

    let mut store = Store::new(
        &engine,
        HostState { wasi_ctx: WasiCtx::builder().build(), table: ResourceTable::new() },
    );

    let bindings = bindings::SpikeTrigger::instantiate_async(&mut store, &component, &linker).await?;
    let trigger = bindings.aerini_spike_trigger_trigger();

    for poll in 1..=2 {
        let started = Instant::now();
        let mut reader = trigger.call_events(&mut store).await?;

        loop {
            let mut buf = Vec::with_capacity(4);
            let (result, filled) = reader.read(&mut store, buf).await?;
            buf = filled;
            for event in &buf {
                println!("poll {poll}: seq={} message={:?} emitted_at_ns={}", event.seq, event.message, event.emitted_at_ns);
            }
            if result.is_closed() {
                break;
            }
        }

        println!("poll {poll} drained in {:?}", started.elapsed());
    }

    Ok(())
}
