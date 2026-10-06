// Trigger-plugin example: emits one heartbeat event on a configurable
// interval and reports when the next beat is due. Implements both interfaces
// the `aerini-node-with-trigger-and-next-fire` world exports -- `node`
// (describe/execute, same as any plugin) and `trigger` (events, the async
// stream) -- and calls the `trigger-schedule` import it adds. See docs/development/plugin-authoring.md's "Trigger
// plugins" section for the concepts this demonstrates, including that
// section's status note: this compiles and traces correctly but cannot yet
// be run end-to-end inside Aerini (host-side event pump has open bugs).
//
// Uses wasip3's own re-exported `wit_bindgen::generate!` (not a direct
// `wit-bindgen` dependency), same reasoning as
// spike/wasi-p3-trigger-poc/guest/src/lib.rs: one shared copy of the async
// runtime-support types wasip3's stream/clock bindings use.
// `runtime_path` points the macro at wasip3's copy of the runtime (this crate
// has no direct `wit-bindgen` dependency), so the generated `wit_stream` is the
// one that accepts `TriggerEvent`.
wasip3::wit_bindgen::generate!({
    world: "aerini-node-with-trigger-and-next-fire",
    runtime_path: "wasip3::wit_bindgen::rt",
});

use exports::aerini::plugin::node::{Guest as NodeGuest, NodeDescriptor, NodeInput, NodeOutput};
use exports::aerini::plugin::trigger::{Guest as TriggerGuest, TriggerEvent};
use std::time::{SystemTime, UNIX_EPOCH};
use aerini::plugin::trigger_schedule;
use wasip3::clocks::monotonic_clock;
use wasip3::wit_bindgen::StreamReader;

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
    // docs/development/plugin-authoring.md's "Trigger plugins" section.
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
            report_next_beat(interval_secs);
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
                report_next_beat(interval_secs);
            }
        });

        rx
    }
}

/// Tells the host when the next beat is due so the Background Runs panel can
/// show a countdown. The report is display-only, so a rejected one (for
/// example a clock that is far off) is ignored and never delays a beat.
fn report_next_beat(interval_secs: u64) {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64);
    let next_ms = now_ms.saturating_add(interval_secs.saturating_mul(1000));
    let _ = trigger_schedule::report_next_fire(next_ms);
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
