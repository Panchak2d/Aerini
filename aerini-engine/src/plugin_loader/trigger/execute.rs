use wasmtime::Engine;

use crate::plugin_loader::bindings::{wit, wit_fs_watch, wit_storage, wit_trigger, wit_trigger_next_fire};
use crate::plugin_loader::trigger::state::TriggerPluginState;
use super::TriggerPre;

/// The linker for `trigger_engine`: WASI 0.2 (async) and 0.3, `wasi:http` 0.3,
/// and this package's `storage`, `fs-watch` and `trigger-schedule` imports. The one place that
/// import surface is defined, shared by the capability probe, load-time
/// `describe`, and trigger resolution.
pub(in crate::plugin_loader) fn trigger_linker(
    engine: &Engine,
) -> Result<wasmtime::component::Linker<TriggerPluginState>, String> {
    let mut linker = wasmtime::component::Linker::<TriggerPluginState>::new(engine);
    // A `wasm32-wasip2` guest's std runtime imports WASI 0.2 alongside the
    // 0.3 interfaces `wasip3` provides; both sets must be linked.
    wasmtime_wasi::p2::add_to_linker_async(&mut linker).map_err(|e| e.to_string())?;
    wasmtime_wasi::p3::add_to_linker(&mut linker).map_err(|e| e.to_string())?;
    wasmtime_wasi_http::p3::add_to_linker(&mut linker).map_err(|e| e.to_string())?;
    wit_storage::add_to_linker::<TriggerPluginState, wasmtime::component::HasSelf<TriggerPluginState>>(
        &mut linker,
        |state| state,
    )
    .map_err(|e| e.to_string())?;
    wit_fs_watch::add_to_linker::<TriggerPluginState, wasmtime::component::HasSelf<TriggerPluginState>>(
        &mut linker,
        |state| state,
    )
    .map_err(|e| e.to_string())?;
    wit_trigger_next_fire::add_to_linker::<TriggerPluginState, wasmtime::component::HasSelf<TriggerPluginState>>(
        &mut linker,
        |state| state,
    )
    .map_err(|e| e.to_string())?;
    Ok(linker)
}

/// The trigger world's `node-input` has the same shape as the sync world's but
/// is a distinct generated type.
pub(in crate::plugin_loader) fn wit_input_to_trigger(input: &wit::NodeInput) -> wit_trigger::aerini::plugin::types::NodeInput {
    use wit_trigger::aerini::plugin::types as t;
    let conv = |ps: &[wit::Param]| -> Vec<t::Param> {
        ps.iter()
            .map(|p| t::Param { key: p.key.clone(), value: p.value.clone() })
            .collect()
    };
    t::NodeInput { params: conv(&input.params), credentials: conv(&input.credentials) }
}

fn trigger_output_to_wit(out: wit_trigger::aerini::plugin::types::NodeOutput) -> wit::NodeOutput {
    wit::NodeOutput {
        success: out.success,
        data: out.data,
        error_code: out.error_code,
        error_message: out.error_message,
        recoverable: out.recoverable,
    }
}

/// Runs one `execute()` on `trigger_engine` in a fresh store.
pub(in crate::plugin_loader) async fn run_trigger_execute(
    pre: &TriggerPre,
    store: &mut wasmtime::Store<TriggerPluginState>,
    input: &wit_trigger::aerini::plugin::types::NodeInput,
) -> wasmtime::Result<wit::NodeOutput> {
    let bindings = pre.instantiate_async(&mut *store).await?;
    let (out,) = bindings
        .aerini_plugin_node()
        .func_execute()
        .call_async(&mut *store, (input,))
        .await?;
    Ok(trigger_output_to_wit(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wit_input_to_trigger_preserves_params_and_credentials() {
        let input = wit::NodeInput {
            params: vec![wit::Param { key: "a".into(), value: "1".into() }],
            credentials: vec![wit::Param { key: "tok".into(), value: "s".into() }],
        };
        let t = wit_input_to_trigger(&input);
        assert_eq!((t.params[0].key.as_str(), t.params[0].value.as_str()), ("a", "1"));
        assert_eq!((t.credentials[0].key.as_str(), t.credentials[0].value.as_str()), ("tok", "s"));
        assert_eq!((t.params.len(), t.credentials.len()), (1, 1));
    }

    #[test]
    fn trigger_output_to_wit_preserves_every_field() {
        let o = trigger_output_to_wit(wit_trigger::aerini::plugin::types::NodeOutput {
            success: false,
            data: "{}".into(),
            error_code: "c".into(),
            error_message: "m".into(),
            recoverable: true,
        });
        assert!(!o.success && o.recoverable);
        assert_eq!((o.data.as_str(), o.error_code.as_str(), o.error_message.as_str()), ("{}", "c", "m"));
    }
}
