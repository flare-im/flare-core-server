//! The development mode switch (see [`crate::domain::plugin::development`]).
use crate::domain::plugin::development::PLUGIN_DEV_MODE_ENV;

/// Only the exact value turns it on: a stray `0` or `false` must not open the path.
pub fn plugin_development_enabled() -> bool {
    let enabled = std::env::var(PLUGIN_DEV_MODE_ENV).is_ok_and(|value| value == "1");
    if enabled {
        tracing::warn!(
            "{PLUGIN_DEV_MODE_ENV}=1: unsigned plugins can run in applications flagged for \
             development; never set this in production"
        );
    }
    enabled
}
