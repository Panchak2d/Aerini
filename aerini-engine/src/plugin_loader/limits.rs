/// Maximum linear memory per plugin execution, summed over all of the
/// component's memories: 64 MiB.
///
/// Plugin instances that attempt to grow beyond this limit receive a WASM
/// `memory.grow` failure surfacing as a trap in the guest.
const PLUGIN_MEMORY_LIMIT: usize = 64 * 1024 * 1024;

/// Maximum elements in any one table of a plugin instance.
const PLUGIN_TABLE_ELEMENT_LIMIT: usize = 100_000;

/// Store-wide resource limits. `wasmtime::StoreLimits` applies its memory cap
/// to each linear memory separately, so a component declaring many memories
/// could exceed the budget; this tracks one running total instead.
#[derive(Default)]
pub(super) struct PluginLimits {
    memory_in_use: usize,
}

impl wasmtime::ResourceLimiter for PluginLimits {
    fn memory_growing(&mut self, current: usize, desired: usize, maximum: Option<usize>) -> wasmtime::Result<bool> {
        if maximum.is_some_and(|max| desired > max) {
            return Ok(false);
        }
        let total = self.memory_in_use.saturating_add(desired.saturating_sub(current));
        if total > PLUGIN_MEMORY_LIMIT {
            return Ok(false);
        }
        self.memory_in_use = total;
        Ok(true)
    }

    fn table_growing(&mut self, _current: usize, desired: usize, maximum: Option<usize>) -> wasmtime::Result<bool> {
        Ok(desired <= PLUGIN_TABLE_ELEMENT_LIMIT && maximum.is_none_or(|max| desired <= max))
    }
}

/// CPU time limit for plugin execution, expressed in epoch ticks.
///
/// The epoch ticker thread (started in [`PluginLoader::new`]) increments the
/// engine epoch every 10 ms. A deadline of 3 000 ticks therefore limits each
/// plugin call to approximately 30 seconds of wall-clock time before it is
/// interrupted with a trap. Applies to `describe()` at load time and
/// `execute()` per workflow run on the synchronous engine; trigger-engine
/// stores yield instead (see [`PLUGIN_TRIGGER_YIELD_TICKS`]).
pub(super) const PLUGIN_EPOCH_DEADLINE: u64 = 3_000;

/// Per-call wall-clock budget for any single guest call made while resolving
/// or starting a trigger-plugin instance (`describe`, and the initial
/// `events(config)` call) — the async trigger engine's counterpart to
/// [`PLUGIN_EPOCH_DEADLINE`]'s role above, using the same ~30s budget for
/// consistency. Deliberately not applied to *draining* an already-started
/// event stream afterward: waiting indefinitely between events is the
/// normal, expected state for a trigger (unlike a bounded `describe`/`events`
/// call, which should always return promptly) — timing that out would
/// misfire on every healthy, simply-quiet trigger.
pub(super) const PLUGIN_TRIGGER_CALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Wall-clock limit on one `execute()` call on the synchronous engine: the
/// guest's ~30 s epoch budget plus one outbound request, which
/// `plugin_http` caps at 60 s.
pub(super) const PLUGIN_EXECUTE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// Epoch ticks (10 ms each) a trigger-engine guest may run before it yields
/// to the async runtime: about one second.
pub(super) const PLUGIN_TRIGGER_YIELD_TICKS: u64 = 100;

/// Once a trigger-plugin directory scan has run this long, no further file
/// is started.
pub(super) const TRIGGER_SCAN_LIMIT: std::time::Duration = std::time::Duration::from_secs(90);

#[cfg(test)]
mod tests {
    use super::*;

    /// Edge case: the memory budget is shared by every memory in a store, and
    /// a growth that adds nothing is always allowed.
    #[test]
    fn memory_budget_is_shared_across_memories() {
        use wasmtime::ResourceLimiter;

        let mut limits = PluginLimits::default();
        let half = PLUGIN_MEMORY_LIMIT / 2;
        assert!(limits.memory_growing(0, half, None).unwrap());
        assert!(limits.memory_growing(0, half, None).unwrap());
        assert!(!limits.memory_growing(0, 65_536, None).unwrap());
        assert!(limits.memory_growing(half, half, None).unwrap());
    }

    /// Edge case: table growth stops at the element cap.
    #[test]
    fn table_growth_stops_at_the_element_cap() {
        use wasmtime::ResourceLimiter;

        let mut limits = PluginLimits::default();
        assert!(limits.table_growing(0, PLUGIN_TABLE_ELEMENT_LIMIT, None).unwrap());
        assert!(!limits.table_growing(0, PLUGIN_TABLE_ELEMENT_LIMIT + 1, None).unwrap());
    }
}
