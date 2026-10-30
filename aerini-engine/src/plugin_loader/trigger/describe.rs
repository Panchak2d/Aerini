use crate::plugin_loader::bindings::{wit_trigger, wit_trigger_metadata};
use crate::plugin_loader::describe::RawDescribe;
use crate::plugin_loader::error::PluginLoadError;
use crate::plugin_loader::limits::PLUGIN_TRIGGER_CALL_TIMEOUT;
use crate::plugin_loader::trigger::state::TriggerPluginState;
use super::TriggerPre;

/// Instantiates `pre` into `store` and calls `describe()` through the `_async`
/// APIs, which are the only ones a store with async-typed imports accepts.
/// Returns the instance alongside the result so the trigger-resolution path can
/// keep using it. A trigger world has no `metadata` export, so `version`,
/// `author` and `icon` stay empty (host defaults).
async fn instantiate_and_describe_trigger_inner(
    pre: &TriggerPre,
    store: &mut wasmtime::Store<TriggerPluginState>,
) -> wasmtime::Result<(wit_trigger::AeriniNodeWithTrigger, RawDescribe)> {
    let bindings = pre.instantiate_async(&mut *store).await?;
    let (d,) = bindings
        .aerini_plugin_node()
        .func_describe()
        .call_async(&mut *store, ())
        .await?;
    let raw = RawDescribe {
        type_id: d.type_id,
        display_name: d.display_name,
        category: d.category,
        description: d.description,
        input_schema: d.input_schema,
        output_schema: d.output_schema,
        version: String::new(),
        author: String::new(),
        icon: String::new(),
    };
    Ok((bindings, raw))
}

/// Bounds `call` by [`PLUGIN_TRIGGER_CALL_TIMEOUT`], which also covers a guest
/// blocked in a host call (the epoch deadline only interrupts running guest
/// code).
async fn within_trigger_call_timeout<T>(
    call: impl std::future::Future<Output = wasmtime::Result<T>>,
) -> Result<T, PluginLoadError> {
    match tokio::time::timeout(PLUGIN_TRIGGER_CALL_TIMEOUT, call).await {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(e)) => Err(PluginLoadError::WasmLink(e.to_string())),
        Err(_elapsed) => Err(PluginLoadError::CallTimedOut(PLUGIN_TRIGGER_CALL_TIMEOUT.as_secs())),
    }
}

/// [`instantiate_and_describe_trigger_inner`] under [`within_trigger_call_timeout`].
pub(in crate::plugin_loader) async fn instantiate_and_describe_trigger(
    pre: &TriggerPre,
    store: &mut wasmtime::Store<TriggerPluginState>,
) -> Result<(wit_trigger::AeriniNodeWithTrigger, RawDescribe), PluginLoadError> {
    within_trigger_call_timeout(instantiate_and_describe_trigger_inner(pre, store)).await
}

/// Like [`instantiate_and_describe_trigger_inner`] for a component that also
/// exports `metadata`: one instantiation, `describe()` plus
/// `describe-metadata()`. The instance is not returned; the event stream is
/// always started from the plain trigger bindings.
async fn describe_trigger_with_metadata_inner(
    pre: &wit_trigger_metadata::AeriniNodeWithTriggerAndMetadataPre<TriggerPluginState>,
    store: &mut wasmtime::Store<TriggerPluginState>,
) -> wasmtime::Result<RawDescribe> {
    let bindings = pre.instantiate_async(&mut *store).await?;
    let (d,) = bindings
        .aerini_plugin_node()
        .func_describe()
        .call_async(&mut *store, ())
        .await?;
    let (m,) = bindings
        .aerini_plugin_metadata()
        .func_describe_metadata()
        .call_async(&mut *store, ())
        .await?;
    Ok(RawDescribe {
        type_id: d.type_id,
        display_name: d.display_name,
        category: d.category,
        description: d.description,
        input_schema: d.input_schema,
        output_schema: d.output_schema,
        version: m.version,
        author: m.author,
        icon: m.icon,
    })
}

/// Describes a trigger plugin for loading: through the `metadata`-aware
/// bindings when the component exports `metadata`, otherwise through the plain
/// trigger bindings with default identity fields. The export check is
/// type-level only.
pub(in crate::plugin_loader) async fn describe_trigger_for_load(
    pre: &TriggerPre,
    store: &mut wasmtime::Store<TriggerPluginState>,
) -> Result<RawDescribe, PluginLoadError> {
    match wit_trigger_metadata::AeriniNodeWithTriggerAndMetadataPre::new(pre.instance_pre().clone()) {
        Ok(meta_pre) => within_trigger_call_timeout(describe_trigger_with_metadata_inner(&meta_pre, store)).await,
        Err(_) => instantiate_and_describe_trigger(pre, store).await.map(|(_, raw)| raw),
    }
}

/// Runs the future built by `make` to completion on a fresh current-thread
/// runtime on its own scoped thread, blocking the caller until it finishes.
/// This is what lets the synchronous `load_plugin`/`describe_plugin` drive
/// `_async` wasmtime calls: it is safe from any context (plain thread, tokio
/// worker, current-thread runtime), whereas `Handle::block_on` panics when called
/// from inside a runtime. The future is built on the new thread, so it need not
/// be `Send`; the store it creates is dropped inside the runtime.
pub(in crate::plugin_loader) fn block_on_scratch_runtime<T, F, Fut>(make: F) -> Result<T, PluginLoadError>
where
    T: Send,
    F: FnOnce() -> Fut + Send,
    Fut: std::future::Future<Output = Result<T, PluginLoadError>>,
{
    std::thread::scope(|scope| {
        scope
            .spawn(move || -> Result<T, PluginLoadError> {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(PluginLoadError::Io)?;
                rt.block_on(make())
            })
            .join()
            .unwrap_or_else(|_| {
                Err(PluginLoadError::WasmLink("plugin describe thread panicked".to_string()))
            })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// The scratch-thread bridge `load_plugin`/`describe_plugin` use must work
    /// from a plain thread.
    #[test]
    fn block_on_scratch_runtime_works_outside_a_runtime() {
        let v = block_on_scratch_runtime(|| async { Ok::<_, PluginLoadError>(7u32) })
            .expect("bridge failed");
        assert_eq!(v, 7);
    }

    /// ...and from inside a multi-thread runtime, where `Handle::block_on`
    /// would panic.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn block_on_scratch_runtime_works_inside_multi_thread_runtime() {
        let v = block_on_scratch_runtime(|| async {
            tokio::time::sleep(Duration::from_millis(5)).await;
            Ok::<_, PluginLoadError>(9u32)
        })
        .expect("bridge failed");
        assert_eq!(v, 9);
    }

    /// ...and from inside a current-thread runtime.
    #[tokio::test]
    async fn block_on_scratch_runtime_works_inside_current_thread_runtime() {
        let v = block_on_scratch_runtime(|| async { Ok::<_, PluginLoadError>(1u32) })
            .expect("bridge failed");
        assert_eq!(v, 1);
    }

    /// A panic in the bridged future surfaces as an error instead of
    /// unwinding into the caller.
    #[test]
    fn block_on_scratch_runtime_maps_panic_to_error() {
        let r: Result<u32, PluginLoadError> = block_on_scratch_runtime(|| async {
            if true {
                panic!("boom");
            }
            Ok::<u32, PluginLoadError>(0)
        });
        assert!(matches!(r, Err(PluginLoadError::WasmLink(_))));
    }
}
