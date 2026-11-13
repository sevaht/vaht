//! The `StatusNotifierWatcher` vaht serves, and the one it hosts for when
//! another program serves it.
//!
//! Tray items register with a watcher (`org.kde.StatusNotifierWatcher`),
//! which lists them and announces each one coming and going; hosts (what
//! shows them: here, the taskbar's tray) read the list and follow the
//! announcements. When the name is free, vaht serves [`Watcher`] itself;
//! [`StatusNotifierWatcherProxy`] reaches another program's.

use calloop::channel::Sender;
use zbus::message::Header;
use zbus::object_server::SignalEmitter;

use super::TrayEvent;
use super::item::ItemAddress;

/// The watcher's well-known name, and its object's interface.
pub(crate) const WATCHER_NAME: &str = "org.kde.StatusNotifierWatcher";

/// The watcher's object.
pub(crate) const WATCHER_PATH: &str = "/StatusNotifierWatcher";

/// The watcher vaht serves: the registered items, and where to tell the
/// tray of new ones.
#[derive(Debug)]
pub(crate) struct Watcher {
    /// Each registered item, as the watcher lists it.
    items: Vec<String>,
    /// Tells the tray, on vaht's event loop.
    events: Sender<TrayEvent>,
}

impl Watcher {
    /// Creates a watcher with no items, telling `events` of new ones.
    pub(crate) const fn new(events: Sender<TrayEvent>) -> Self {
        Self {
            items: Vec::new(),
            events,
        }
    }

    /// Forgets every item on the connection `bus`; returns them as listed.
    pub(crate) fn forget(&mut self, bus: &str) -> Vec<String> {
        let (gone, kept) = self
            .items
            .drain(..)
            .partition(|listed| ItemAddress::listed(listed).bus == bus);
        self.items = kept;
        gone
    }
}

#[zbus::interface(name = "org.kde.StatusNotifierWatcher")]
impl Watcher {
    /// Registers the item `service` of the calling connection.
    async fn register_status_notifier_item(
        &mut self,
        service: &str,
        #[zbus(header)] header: Header<'_>,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> zbus::fdo::Result<()> {
        let sender = header
            .sender()
            .ok_or_else(|| zbus::fdo::Error::InvalidArgs("no sender".to_owned()))?;
        let address = ItemAddress::registered(service, sender.as_str());
        let listed = address.to_listed();
        if self.items.contains(&listed) {
            return Ok(());
        }
        self.items.push(listed.clone());
        Self::status_notifier_item_registered(&emitter, &listed).await?;
        self.registered_status_notifier_items_changed(&emitter)
            .await?;
        // The tray is gone only as vaht stops, when no one is told.
        let _ = self.events.send(TrayEvent::Registered(address));
        Ok(())
    }

    /// Accepts a host: vaht itself shows the items, and others may too.
    #[expect(
        clippy::unused_self,
        reason = "a D-Bus method, which zbus calls on the object"
    )]
    fn register_status_notifier_host(&self, service: &str) {
        tracing::debug!(service, "another tray host registered");
    }

    /// Announces an item registering.
    #[zbus(signal)]
    pub(crate) async fn status_notifier_item_registered(
        emitter: &SignalEmitter<'_>,
        service: &str,
    ) -> zbus::Result<()>;

    /// Announces an item going.
    #[zbus(signal)]
    pub(crate) async fn status_notifier_item_unregistered(
        emitter: &SignalEmitter<'_>,
        service: &str,
    ) -> zbus::Result<()>;

    /// Announces a host, so items waiting for one register.
    #[zbus(signal)]
    pub(crate) async fn status_notifier_host_registered(
        emitter: &SignalEmitter<'_>,
    ) -> zbus::Result<()>;

    /// The registered items.
    #[zbus(property)]
    fn registered_status_notifier_items(&self) -> Vec<String> {
        self.items.clone()
    }

    /// Whether a host shows the items: vaht does.
    #[zbus(property)]
    #[expect(
        clippy::unused_self,
        reason = "a D-Bus property, which zbus reads from the object"
    )]
    const fn is_status_notifier_host_registered(&self) -> bool {
        true
    }

    /// The protocol's version.
    #[zbus(property)]
    #[expect(
        clippy::unused_self,
        reason = "a D-Bus property, which zbus reads from the object"
    )]
    const fn protocol_version(&self) -> i32 {
        0
    }
}

/// Another program's watcher, which vaht hosts for.
#[zbus::proxy(
    interface = "org.kde.StatusNotifierWatcher",
    default_service = "org.kde.StatusNotifierWatcher",
    default_path = "/StatusNotifierWatcher"
)]
pub(crate) trait StatusNotifierWatcher {
    /// Registers a host: one showing the items.
    fn register_status_notifier_host(&self, service: &str) -> zbus::Result<()>;

    /// The registered items.
    #[zbus(property)]
    fn registered_status_notifier_items(&self) -> zbus::Result<Vec<String>>;

    /// Announces an item registering.
    #[zbus(signal)]
    fn status_notifier_item_registered(&self, service: &str) -> zbus::Result<()>;

    /// Announces an item going.
    #[zbus(signal)]
    fn status_notifier_item_unregistered(&self, service: &str) -> zbus::Result<()>;
}

/// A tray item's methods: what clicking and scrolling it ask of it.
#[zbus::proxy(interface = "org.kde.StatusNotifierItem")]
pub(crate) trait StatusNotifierItem {
    /// The primary action, at global `x`, `y`: a left click.
    fn activate(&self, x: i32, y: i32) -> zbus::Result<()>;

    /// The secondary action: a middle click.
    fn secondary_activate(&self, x: i32, y: i32) -> zbus::Result<()>;

    /// The item's own menu, for an item without a dbusmenu: a right click.
    fn context_menu(&self, x: i32, y: i32) -> zbus::Result<()>;

    /// A wheel turn of `delta` along `orientation` (`vertical`).
    fn scroll(&self, delta: i32, orientation: &str) -> zbus::Result<()>;
}
