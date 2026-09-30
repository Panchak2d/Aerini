use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Instant;

use tokio::sync::mpsc;
use wasmtime::component::{
    Component, Lift, Linker, ResourceTable, Source, StreamConsumer, StreamResult,
};
use wasmtime::{Config, Engine, Store, StoreContextMut};
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

struct Sink<T> {
    tx: mpsc::UnboundedSender<T>,
}

impl<T> StreamConsumer<HostState> for Sink<T>
where
    T: Lift + Send + Sync + 'static,
{
    type Item = T;

    fn poll_consume(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        mut store: StoreContextMut<HostState>,
        mut source: Source<'_, T>,
        _finish: bool,
    ) -> Poll<wasmtime::Result<StreamResult>> {
        if source.remaining(&mut store) == 0 {
            return Poll::Ready(Ok(StreamResult::Completed));
        }

        let mut buf: Vec<T> = Vec::with_capacity(4);
        if let Err(e) = source.read(&mut store, &mut buf) {
            return Poll::Ready(Err(e));
        }

        for item in buf {
            if self.tx.send(item).is_err() {
                return Poll::Ready(Ok(StreamResult::Dropped));
            }
        }

        Poll::Ready(Ok(StreamResult::Completed))
    }
}

#[tokio::main]
async fn main() -> wasmtime::Result<()> {
    let mut config = Config::new();
    config.wasm_component_model(true);
    config.wasm_component_model_async(true);
    let engine = Engine::new(&config)?;

    let mut linker: Linker<HostState> = Linker::new(&engine);
    // The guest is built for wasm32-wasip2, so its std runtime imports WASI 0.2 alongside
    // the p3 interfaces the guest's own code uses. Both sets are required.
    wasmtime_wasi::p2::add_to_linker_async(&mut linker)?;
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

        store
            .run_concurrent(async |accessor| -> wasmtime::Result<()> {
                let reader = trigger.call_events(accessor).await?;
                let (tx, mut rx) = mpsc::unbounded_channel();
                accessor.with(|access| reader.pipe(access, Sink { tx }))?;

                while let Some(event) = rx.recv().await {
                    println!(
                        "poll {poll}: seq={} message={:?} emitted_at_ns={}",
                        event.seq, event.message, event.emitted_at_ns
                    );
                }
                Ok(())
            })
            .await??;

        println!("poll {poll} drained in {:?}", started.elapsed());
    }

    Ok(())
}
