// Trigger-plugin example: emits one heartbeat event on a configurable
// interval. Implements both interfaces the `aerini-node-with-trigger` world
// requires -- `node` (describe/execute, same as any plugin) and `trigger`
// (events, the async stream). See docs/plugin-authoring.md's "Trigger
// plugins" section for the concepts this demonstrates, including that
// section's status note: this compiles and traces correctly but cannot yet
// be run end-to-end inside Aerini (host-side event pump has open bugs).
//
// Uses wasip3's own re-exported `wit_bindgen::generate!` (not a direct
// `wit-bindgen` dependency), same reasoning as
// spike/wasi-p3-trigger-poc/guest/src/lib.rs: one shared copy of the async
// runtime-support types wasip3's stream/clock bindings use.
wasip3::wit_bindgen::generate!({ world: "aerini-node-with-trigger" });

use exports::aerini::plugin::node::{Guest as NodeGuest, NodeDescriptor, NodeInput, NodeOutput};
use exports::aerini::plugin::trigger::{Guest as TriggerGuest, TriggerEvent};
use wasip3::clocks::monotonic_clock;
use wasip3::wit_bindgen::rt::async_support::stream_support::StreamReader;

const DEFAULT_INTERVAL_SECS: u64 = 60;
const NS_PER_SEC: u64 = 1_000_000_000;

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
        let (mut tx, rx) = wasip3::wit_stream::new::<TriggerEvent>();

        // Detached from this call's own task, same shape as
        // spike/wasi-p3-trigger-poc/guest/src/lib.rs's own stream-writing
        // pattern:
        // `events()` hands `rx` to the host and returns immediately, while
        // this task keeps writing on its own schedule.
        //
        // UNCONFIRMED, inherited from the spike's own open item (see
        // spike/wasi-p3-trigger-poc/README.md, "Host, edge case"): what
        // happens on this side when the host drops `rx` (job stopped,
        // instance torn down) is not verified against any source. This
        // loop has no explicit exit condition and no code here checks
        // whether `write_all` is still making progress -- unlike the
        // spike's own finite 3-event stream, which closes cleanly by
        // dropping `tx` when its fixed sequence ends. A real build is
        // needed to confirm whether an unresponsive `write_all` on a
        // dropped receiver stalls this task forever or is unblocked by the
        // component-model-async runtime on its own. Flagged, not resolved —
        // do not treat this loop's shutdown behavior as verified.
        wasip3::spawn(async move {
            let mut tick: u64 = 0;
            loop {
                monotonic_clock::wait_for(interval_secs.saturating_mul(NS_PER_SEC)).await;
                tick += 1;
                let event = TriggerEvent {
                    data: format!(
                        r#"{{"tick":{},"emitted_at_ns":{}}}"#,
                        tick,
                        monotonic_clock::now()
                    ),
                };
                tx.write_all(vec![event]).await;
            }
        });

        rx
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
