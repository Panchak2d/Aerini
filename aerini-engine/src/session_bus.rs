//! Questions put to the Linux session bus.
//!
//! Every function here answers "no" or "don't know" when the bus can't be
//! reached or doesn't answer within [`QUERY_TIMEOUT`], so a caller on a
//! machine without a session bus (a server, a container) is never blocked.

use std::future::Future;
use std::time::Duration;

use zbus::proxy::{Builder, CacheProperties};
use zbus::{Connection, Proxy};

const QUERY_TIMEOUT: Duration = Duration::from_secs(2);

async fn bounded<T>(query: impl Future<Output = Option<T>>) -> Option<T> {
    tokio::time::timeout(QUERY_TIMEOUT, query).await.ok().flatten()
}

async fn proxy_for(
    conn: &Connection,
    destination: &'static str,
    path: &'static str,
    interface: &'static str,
) -> Option<Proxy<'static>> {
    Builder::<Proxy<'static>>::new(conn)
        .destination(destination).ok()?
        .path(path).ok()?
        .interface(interface).ok()?
        .cache_properties(CacheProperties::No)
        .build()
        .await
        .ok()
}

/// Whether a StatusNotifier host (the part of a desktop that draws tray
/// icons) is registered with the session's watcher. `false` also covers "no
/// session bus" and "no watcher". The watcher is looked up under the
/// `org.kde` name, which KDE Plasma and the GNOME AppIndicator extension
/// both serve; the `org.freedesktop` name in the specification is not served
/// by either.
pub async fn status_notifier_host_registered() -> bool {
    bounded(async {
        let conn = Connection::session().await.ok()?;
        let watcher = proxy_for(
            &conn,
            "org.kde.StatusNotifierWatcher",
            "/StatusNotifierWatcher",
            "org.kde.StatusNotifierWatcher",
        )
        .await?;
        watcher.get_property::<bool>("IsStatusNotifierHostRegistered").await.ok()
    })
    .await
    .unwrap_or(false)
}

/// Whether the notification daemon reads markup in a notification body.
/// `None` when the daemon could not be asked.
pub async fn notification_body_markup() -> Option<bool> {
    bounded(async {
        let conn = Connection::session().await.ok()?;
        let daemon = proxy_for(
            &conn,
            "org.freedesktop.Notifications",
            "/org/freedesktop/Notifications",
            "org.freedesktop.Notifications",
        )
        .await?;
        let capabilities: Vec<String> = daemon.call("GetCapabilities", &()).await.ok()?;
        Some(supports_body_markup(&capabilities))
    })
    .await
}

fn supports_body_markup(capabilities: &[String]) -> bool {
    capabilities.iter().any(|c| c == "body-markup")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_markup_is_matched_as_a_whole_capability_name() {
        let caps = |names: &[&str]| names.iter().map(|n| n.to_string()).collect::<Vec<_>>();
        assert!(supports_body_markup(&caps(&["actions", "body", "body-markup"])));
        assert!(!supports_body_markup(&caps(&["actions", "body", "body-hyperlinks", "x-body-markup"])));
        assert!(!supports_body_markup(&[]));
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn an_unreachable_bus_gives_the_safe_answers() {
        let saved = std::env::var_os("DBUS_SESSION_BUS_ADDRESS");
        std::env::set_var("DBUS_SESSION_BUS_ADDRESS", "unix:path=/nonexistent/aerini-test-bus");
        let host = status_notifier_host_registered().await;
        let markup = notification_body_markup().await;
        match saved {
            Some(v) => std::env::set_var("DBUS_SESSION_BUS_ADDRESS", v),
            None => std::env::remove_var("DBUS_SESSION_BUS_ADDRESS"),
        }
        assert!(!host);
        assert_eq!(markup, None);
    }
}
