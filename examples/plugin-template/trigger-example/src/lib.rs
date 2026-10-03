// Trigger-plugin example: emits one heartbeat event on a configurable
// interval. Implements both interfaces the `aerini-node-with-trigger-and-next-fire`
// world exports -- `node` (describe/execute, same as any plugin) and `trigger`
// (events, the async stream) -- and calls its `trigger-schedule` import to
// tell Aerini when the next heartbeat is due, which drives the "next in Ns"
// countdown in the Background Runs panel. See docs/plugin-authoring.md's
// "Trigger plugins" section for the concepts this demonstrates.
//
// Uses wasip3's own re-exported `wit_bindgen` (not a direct `wit-bindgen`
// dependency) so this crate's `generate!` output and wasip3's clock bindings
// share one copy of the async runtime-support types. `runtime_path` is
// required because there is no direct `wit_bindgen` dependency for the
// macro's default path to resolve, and streams of the generated
// `TriggerEvent` must be created with the `wit_stream` module `generate!`
// emits, not `wasip3::wit_stream`, whose `StreamPayload` trait is a
// separate type.
wasip3::wit_bindgen::generate!({
    world: "aerini-node-with-trigger-and-next-fire",
    runtime_path: "wasip3::wit_bindgen::rt",
});

use exports::aerini::plugin::node::{Guest as NodeGuest, NodeDescriptor, NodeInput, NodeOutput};
use exports::aerini::plugin::trigger::{Guest as TriggerGuest, TriggerEvent};
use wasip3::clocks::monotonic_clock;
use wasip3::wit_bindgen::StreamReader;

use aerini::plugin::trigger_schedule::report_next_fire;

const DEFAULT_INTERVAL_SECS: u64 = 60;
const NS_PER_SEC: u64 = 1_000_000_000;
const MS_PER_SEC: u128 = 1_000;

struct HeartbeatTrigger;

impl NodeGuest for HeartbeatTrigger {
    fn describe() -> NodeDescriptor {
        NodeDescriptor {
            type_id: "com.example.heartbeat-trigger".to_string(),
            display_name: "Heartbeat Trigger".to_string(),
            // "action" / "ai" / "logic" / "utility" only -- there is no
            // "trigger" category. Aerini recognizes trigger capability by
            // graph position (no incoming connections), not this field.
            category: "utility".to_string(),
            description: "Starts a workflow run on a fixed interval, independent of Aerini's built-in Schedule node.".to_string(),
            input_schema: r#"{
                "type": "object",
                "properties": {
                    "interval_secs": {
                        "type": "number",
                        "description": "Seconds between heartbeats. Defaults to 60 if omitted, zero, or negative."
                    }
                }
            }"#.to_string(),
            output_schema: r#"{"type":"object","properties":{}}"#.to_string(),
        }
    }

    // Required by the WIT world -- a component can't export `node`
    // partially -- but not meaningfully called for this plugin's own logic:
    // event data reaches downstream nodes via `$vars`, populated from each
    // `trigger-event.data` before this node's own `execute()` would run for
    // that same graph position. Returns trivial success immediately,
    // matching Aerini's built-in Schedule node's own convention for the same
    // "the real work already happened elsewhere" situation. See
    // docs/plugin-authoring.md's "Trigger plugins" section.
    fn execute(_input: NodeInput) -> NodeOutput {
        NodeOutput {
            success: true,
            data: "{}".to_string(),
            error_code: String::new(),
            error_message: String::new(),
            recoverable: false,
        }
    }
}

impl TriggerGuest for HeartbeatTrigger {
    async fn events(config: String) -> StreamReader<TriggerEvent> {
        let interval_secs = parse_interval_secs(&config);
        let (mut tx, rx) = wit_stream::new::<TriggerEvent>();

        // Detached from this call's own task: `events()` hands `rx` to the
        // host and returns immediately, while this task keeps writing on its
        // own schedule. `write_all` returns the values it could not send once
        // the host drops the stream, which ends the loop.
        wasip3::spawn(async move {
            let mut tick: u64 = 0;
            loop {
                report_next_fire_in(interval_secs);
                monotonic_clock::wait_for(interval_secs.saturating_mul(NS_PER_SEC)).await;
                tick += 1;
                let event = TriggerEvent {
                    data: format!(
                        r#"{{"tick":{},"emitted_at_ns":{}}}"#,
                        tick,
                        monotonic_clock::now()
                    ),
                };
                if !tx.write_all(vec![event]).await.is_empty() {
                    break;
                }
            }
        });

        rx
    }
}

/// Tells Aerini the next heartbeat is due `secs` from now (Unix epoch
/// milliseconds, as `trigger-schedule.report-next-fire` expects). Advisory:
/// Aerini ignores a value it cannot use, and a clock before the epoch simply
/// reports nothing useful.
fn report_next_fire_in(secs: u64) {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let next_ms = now_ms.saturating_add(u128::from(secs).saturating_mul(MS_PER_SEC));
    if let Ok(ms) = u64::try_from(next_ms) {
        report_next_fire(ms);
    }
}

/// Parses `interval_secs` out of the trigger's JSON-object config string
/// (see wit/node.wit's `trigger.events` doc comment). Missing, non-numeric,
/// zero, or negative falls back to `DEFAULT_INTERVAL_SECS` rather than
/// erroring `events()` outright -- a misconfigured interval degrading to a
/// safe default is better for a long-lived trigger instance than taking the
/// whole trigger offline.
fn parse_interval_secs(config: &str) -> u64 {
    serde_json::from_str::<serde_json::Value>(config)
        .ok()
        .and_then(|v| v.get("interval_secs").and_then(|n| n.as_f64()))
        .filter(|secs| *secs > 0.0)
        .map(|secs| secs as u64)
        .unwrap_or(DEFAULT_INTERVAL_SECS)
}

export!(HeartbeatTrigger);
