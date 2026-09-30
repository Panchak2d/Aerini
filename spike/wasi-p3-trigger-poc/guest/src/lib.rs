// Uses wasip3's own re-exported `wit_bindgen` (not a direct `wit-bindgen` dependency)
// so this crate's `generate!` output and wasip3's pre-generated clock/stream bindings
// share exactly one version of the async runtime-support types. `runtime_path` is
// required because the crate has no direct `wit_bindgen` dependency for the macro's
// default path to resolve, and streams of the generated `Event` must be created with
// the `wit_stream` module `generate!` emits here, not `wasip3::wit_stream`, whose
// `StreamPayload` trait is a separate type.
wasip3::wit_bindgen::generate!({
    world: "spike-trigger",
    path: "wit",
    runtime_path: "wasip3::wit_bindgen::rt",
});

use exports::aerini::spike_trigger::trigger::{Event, Guest};
use wasip3::clocks::monotonic_clock;
use wasip3::wit_bindgen::StreamReader;

const EVENT_COUNT: u32 = 3;
const EVENT_SPACING_NS: u64 = 50_000_000;

struct SpikeTrigger;

impl Guest for SpikeTrigger {
    async fn events() -> StreamReader<Event> {
        let (mut tx, rx) = wit_stream::new::<Event>();

        // Detached from this call's own task: `events()` hands `rx` to the host and
        // returns immediately, while this task keeps writing until the fixed sequence
        // ends (dropping `tx`, which closes the stream) or the host drops `rx` first.
        wasip3::spawn(async move {
            for seq in 0..EVENT_COUNT {
                let event = Event {
                    seq,
                    message: format!("spike-event-{seq}"),
                    emitted_at_ns: monotonic_clock::now(),
                };
                tx.write_all(vec![event]).await;
                if seq + 1 < EVENT_COUNT {
                    monotonic_clock::wait_for(EVENT_SPACING_NS).await;
                }
            }
        });

        rx
    }
}

export!(SpikeTrigger);
