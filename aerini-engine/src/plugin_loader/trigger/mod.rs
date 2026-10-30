use chrono::{DateTime, Utc};

use crate::plugin_loader::bindings::wit_trigger;
use crate::plugin_loader::trigger::state::TriggerPluginState;

pub(super) mod describe;
pub(super) mod execute;
pub(super) mod stall;
pub(super) mod state;
pub(super) mod stream;

/// Latest next-fire time a running trigger plugin reported, if any. See
/// [`PluginLoader::start_trigger`].
pub type NextFireWatch = tokio::sync::watch::Receiver<Option<DateTime<Utc>>>;

/// Pre-instantiated bindings for a component hosted on `trigger_engine`.
pub(in crate::plugin_loader) type TriggerPre = wit_trigger::AeriniNodeWithTriggerPre<TriggerPluginState>;
