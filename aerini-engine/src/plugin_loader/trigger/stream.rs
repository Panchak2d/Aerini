use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::task::{Context, Poll};

use wasmtime::component::{Source, StreamConsumer, StreamResult};

use crate::plugin_loader::bindings::wit_trigger;
use crate::plugin_loader::trigger::state::TriggerPluginState;

/// Bridges a guest's `trigger-event` stream to an mpsc channel of plain
/// JSON strings. `poll_consume` waits for channel capacity before pulling
/// from `source`, so a slow receiver applies backpressure to the guest
/// instead of buffering unboundedly on the host side; if the receiver is
/// dropped mid-stream, the stream is reported as `Dropped` so the guest's
/// writer stops.
pub(in crate::plugin_loader) struct TriggerEventConsumer {
    pub(in crate::plugin_loader) tx: tokio_util::sync::PollSender<Result<String, String>>,
    pub(in crate::plugin_loader) activity: Arc<AtomicU64>,
}

impl StreamConsumer<TriggerPluginState> for TriggerEventConsumer {
    type Item = wit_trigger::exports::aerini::plugin::trigger::TriggerEvent;

    fn poll_consume(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        mut store: wasmtime::StoreContextMut<TriggerPluginState>,
        mut source: Source<'_, Self::Item>,
        finish: bool,
    ) -> Poll<wasmtime::Result<StreamResult>> {
        let this = self.get_mut();
        match this.tx.poll_reserve(cx) {
            Poll::Pending => {
                if finish {
                    Poll::Ready(Ok(StreamResult::Cancelled))
                } else {
                    Poll::Pending
                }
            }
            Poll::Ready(Err(_)) => Poll::Ready(Ok(StreamResult::Dropped)),
            Poll::Ready(Ok(())) => {
                if source.remaining(&mut store) == 0 {
                    // poll_reserve already committed a slot; nothing to send yet, so give it back.
                    this.tx.abort_send();
                    return Poll::Ready(Ok(StreamResult::Completed));
                }

                let mut buf: Vec<Self::Item> = Vec::with_capacity(1);
                if let Err(e) = source.read(&mut store, &mut buf) {
                    return Poll::Ready(Err(e));
                }

                let Some(event) = buf.into_iter().next() else {
                    return Poll::Ready(Ok(StreamResult::Completed));
                };

                if this.tx.send_item(Ok(event.data)).is_err() {
                    return Poll::Ready(Ok(StreamResult::Dropped));
                }
                this.activity.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

                Poll::Ready(Ok(StreamResult::Completed))
            }
        }
    }
}
