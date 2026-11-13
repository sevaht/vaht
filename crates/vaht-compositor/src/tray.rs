//! The tray: programs' status icons (`StatusNotifierItem`), on the taskbar.
//!
//! A [`Tray`] is a host of tray items on the session bus. It serves the
//! watcher items register with ([`watcher`]), or, when another program
//! already serves one, registers with that one as a host. Everything it
//! learns arrives as an event on vaht's loop ([`TrayEvent`]): an item
//! registering (from the watcher), an item's properties read, an item
//! announcing a new icon, title or tooltip, a connection leaving the bus.
//! Nothing is polled. Each item is a [`TrayItem`] value, read anew whenever
//! it announces a change. Clicking one calls it ([`TrayAction`]); its
//! dbusmenu menu waits for vaht's menus.

mod item;
mod watcher;

use std::collections::HashMap;
use std::fmt;

use calloop::channel;
use calloop::futures::{Scheduler, executor};
use futures_util::future::Either;
use futures_util::{Stream, StreamExt, stream};
use smithay::reexports::calloop::LoopHandle;
use smithay::utils::{Logical, Point};
use zbus::OwnedVariant;
use zbus::fdo::{DBusProxy, PropertiesProxy, RequestNameFlags, RequestNameReply};
use zbus::message::Type as MessageType;
use zbus::object_server::SignalEmitter;
use zbus::{Connection, MatchRule, MessageStream};

use self::item::ItemAddress;
pub(crate) use self::item::{ItemIcon, TrayItem, TrayItemKey};
use self::watcher::{
    StatusNotifierItemProxy, StatusNotifierWatcherProxy, WATCHER_NAME, WATCHER_PATH, Watcher,
};
use crate::bus::Bus;
use crate::event_source::{EventSource, StreamSource};
use crate::resources::ResourceLedger;

/// The interface every tray item serves.
const ITEM_INTERFACE: &str = "org.kde.StatusNotifierItem";

/// How the tray reaches its owner: what to call when what it shows
/// changed, and how to find it again in the owner's state.
pub(crate) struct TrayHooks<State: 'static> {
    /// Called when the items shown changed.
    pub(crate) on_change: fn(&mut State),
    /// Finds the tray in the state, while it exists.
    pub(crate) find: fn(&mut State) -> Option<&mut Tray<State>>,
}

impl<State> Clone for TrayHooks<State> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<State> Copy for TrayHooks<State> {}

impl<State> fmt::Debug for TrayHooks<State> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("TrayHooks").finish_non_exhaustive()
    }
}

/// What a click or a wheel turn on an item asks of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TrayAction {
    /// Its main action: a left click.
    Activate,
    /// Its other action: a middle click.
    SecondaryActivate,
    /// Its menu: a right click.
    ContextMenu,
    /// A wheel turn, in notches; positive is down.
    Scroll(i32),
}

impl TrayAction {
    /// The left button's code, as the kernel numbers it (`BTN_LEFT`).
    const LEFT_BUTTON: u32 = 0x110;
    /// The right button's code (`BTN_RIGHT`).
    const RIGHT_BUTTON: u32 = 0x111;
    /// The middle button's code (`BTN_MIDDLE`).
    const MIDDLE_BUTTON: u32 = 0x112;

    /// Returns what pressing the button `code` asks, if anything.
    pub(crate) const fn for_button(code: u32) -> Option<Self> {
        match code {
            Self::LEFT_BUTTON => Some(Self::Activate),
            Self::MIDDLE_BUTTON => Some(Self::SecondaryActivate),
            Self::RIGHT_BUTTON => Some(Self::ContextMenu),
            _ => None,
        }
    }
}

/// What reading an item gave.
#[derive(Debug)]
pub(crate) struct ItemRead {
    /// Its connection's unique name.
    owner: String,
    /// Its program's process, if the bus says.
    pid: Option<u32>,
    /// Its properties.
    properties: HashMap<String, OwnedVariant>,
}

/// What the tray learns, on vaht's loop.
#[derive(Debug)]
pub(crate) enum TrayEvent {
    /// The tray is on the bus, or could not get there.
    Connected(Result<TrayConnection, String>),
    /// An item registered.
    Registered(ItemAddress),
    /// An item left.
    Unregistered(ItemAddress),
    /// The item at this object of this connection (its unique name)
    /// announced a change.
    Changed {
        /// The connection's unique name.
        owner: String,
        /// The item's object.
        path: String,
    },
    /// The connection with this name left the bus.
    OwnerGone(String),
    /// An item's properties were read.
    Fetched {
        /// The item.
        address: ItemAddress,
        /// What was read of it, or why it could not be.
        properties: Result<ItemRead, String>,
    },
    /// A call to an item, or to the watcher, failed.
    CallFailed(String),
    /// Something done, with nothing to tell.
    Done,
}

/// The tray's connection, and what it is on the bus.
#[derive(Debug)]
pub(crate) struct TrayConnection {
    /// The connection.
    connection: Connection,
    /// Whether vaht serves the watcher, or hosts for another's.
    role: TrayRole,
}

/// What the tray is on the bus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TrayRole {
    /// It serves the watcher.
    Watcher,
    /// It hosts for another program's watcher.
    Host,
}

/// The tray: its items, and its place on the session bus.
#[derive(Debug)]
pub(crate) struct Tray<State: 'static> {
    /// Runs the tray's calls and reads.
    _executor: EventSource<State>,
    /// Hears from the watcher vaht serves.
    _watcher_events: EventSource<State>,
    /// Hears items announcing changes and connections leaving.
    news: Option<StreamSource<State>>,
    /// Schedules the tray's calls and reads.
    scheduler: Scheduler<TrayEvent>,
    /// Keeps the watcher's channel open while the tray lives: calloop
    /// removes a channel's source once its last sender goes, which the
    /// watcher's would as a connection fails, leaving `_watcher_events`
    /// registered in name only.
    _events: channel::Sender<TrayEvent>,
    /// The connection, once on the bus.
    connection: Option<Connection>,
    /// What the tray is on the bus, once there.
    role: Option<TrayRole>,
    /// The items read, in the order they registered.
    items: Vec<TrayItem>,
    /// The key the next new item gets.
    next_key: u64,
    /// The loop the tray's sources are registered with.
    handle: LoopHandle<'static, State>,
    /// Counts the tray's sources.
    ledger: ResourceLedger,
    /// How the tray reaches its owner.
    hooks: TrayHooks<State>,
}

impl<State: 'static> Tray<State> {
    /// Starts a tray on `bus` (normally [`Bus::Session`]); it shows nothing
    /// until it is there, and nothing at all if it cannot get there, which
    /// is logged.
    ///
    /// # Errors
    ///
    /// Returns an error if the tray's sources cannot be registered.
    pub(crate) fn start(
        handle: &LoopHandle<'static, State>,
        ledger: &ResourceLedger,
        bus: Bus,
        hooks: TrayHooks<State>,
    ) -> Result<Self, calloop::Error> {
        let (executor, scheduler) = executor::<TrayEvent>()?;
        let executor = EventSource::insert(handle, ledger, executor, move |event, (), state| {
            Self::deliver(state, event, hooks);
        })
        .map_err(|error| error.error)?;
        let (events, received) = channel::channel::<TrayEvent>();
        let watcher_events =
            EventSource::insert(handle, ledger, received, move |event, (), state| {
                if let channel::Event::Msg(event) = event {
                    Self::deliver(state, event, hooks);
                }
            })
            .map_err(|error| error.error)?;
        let watcher = Watcher::new(events.clone());
        let connected = scheduler.schedule(async move {
            TrayEvent::Connected(
                Self::connect(&bus, watcher)
                    .await
                    .map_err(|error| error.to_string()),
            )
        });
        if connected.is_err() {
            tracing::warn!("the tray could not start");
        }
        Ok(Self {
            _executor: executor,
            _watcher_events: watcher_events,
            news: None,
            scheduler,
            _events: events,
            connection: None,
            role: None,
            items: Vec::new(),
            next_key: 0,
            handle: handle.clone(),
            ledger: ledger.clone(),
            hooks,
        })
    }

    /// Returns the items, those `pin_left` names (by id, in any case)
    /// first in that order, those `pin_right` names last, the others
    /// between in the order they came, as Fluxbox orders its tray.
    pub(crate) fn ordered(&self, pin_left: &[String], pin_right: &[String]) -> Vec<&TrayItem> {
        let position =
            |pins: &[String], id: &str| pins.iter().position(|pin| pin.eq_ignore_ascii_case(id));
        let rank =
            |item: &TrayItem| match (position(pin_left, &item.id), position(pin_right, &item.id)) {
                (Some(at), _) => (0, at),
                (None, Some(at)) => (2, at),
                (None, None) => (1, 0),
            };
        let mut items: Vec<&TrayItem> = self.items.iter().collect();
        items.sort_by_key(|item| rank(item));
        items
    }

    /// Returns the process of the item `key`, if the bus said.
    pub(crate) fn pid_of(&self, key: TrayItemKey) -> Option<u32> {
        self.items
            .iter()
            .find(|item| item.key == key)
            .and_then(|item| item.pid)
    }

    /// Asks the item `key` for `action`, the pointer at global `at`.
    pub(crate) fn act(&self, key: TrayItemKey, action: TrayAction, at: Point<i32, Logical>) {
        let (Some(item), Some(connection)) = (
            self.items.iter().find(|item| item.key == key),
            self.connection.clone(),
        ) else {
            return;
        };
        let address = item.address.clone();
        let call = async move {
            let proxy = StatusNotifierItemProxy::builder(&connection)
                .destination(address.bus.as_str())
                .path(address.path.as_str())
                .cache_properties(zbus::proxy::CacheProperties::No)
                .build()
                .await?;
            match action {
                TrayAction::Activate => proxy.activate(at.x, at.y).await,
                TrayAction::SecondaryActivate => proxy.secondary_activate(at.x, at.y).await,
                TrayAction::ContextMenu => proxy.context_menu(at.x, at.y).await,
                TrayAction::Scroll(notches) => proxy.scroll(notches, "vertical").await,
            }
        };
        self.schedule(async move {
            call.await.map_or_else(
                |error: zbus::Error| TrayEvent::CallFailed(error.to_string()),
                |()| TrayEvent::Done,
            )
        });
    }

    /// Hands `event` to the tray in `state`, and tells the owner if what it
    /// shows changed.
    fn deliver(state: &mut State, event: TrayEvent, hooks: TrayHooks<State>) {
        let changed = (hooks.find)(state).is_some_and(|tray| tray.hear(event));
        if changed {
            (hooks.on_change)(state);
        }
    }

    /// Takes in `event`; returns whether the items shown changed.
    fn hear(&mut self, event: TrayEvent) -> bool {
        match event {
            TrayEvent::Connected(Ok(connected)) => {
                self.listen(&connected);
                false
            }
            TrayEvent::Connected(Err(error)) => {
                tracing::info!(%error, "no tray: the session bus cannot be reached");
                false
            }
            TrayEvent::Registered(address) => {
                self.fetch(address);
                false
            }
            TrayEvent::Changed { owner, path } => {
                let changed: Vec<ItemAddress> = self
                    .items
                    .iter()
                    .filter(|item| item.owner == owner && item.address.path == path)
                    .map(|item| item.address.clone())
                    .collect();
                for address in changed {
                    self.fetch(address);
                }
                false
            }
            TrayEvent::Fetched {
                address,
                properties,
            } => self.take_properties(address, properties),
            TrayEvent::Unregistered(address) => {
                let before = self.items.len();
                self.items.retain(|item| item.address != address);
                self.items.len() != before
            }
            TrayEvent::OwnerGone(name) => self.forget_connection(&name),
            TrayEvent::CallFailed(error) => {
                tracing::debug!(%error, "a tray call failed");
                false
            }
            TrayEvent::Done => false,
        }
    }

    /// Keeps the item at `address` as `properties` describe it; returns
    /// whether that changed what is shown.
    fn take_properties(
        &mut self,
        address: ItemAddress,
        properties: Result<ItemRead, String>,
    ) -> bool {
        let ItemRead {
            owner,
            pid,
            properties,
        } = match properties {
            Ok(read) => read,
            Err(error) => {
                tracing::debug!(%error, item = %address.to_listed(), "a tray item could not be read");
                return false;
            }
        };
        let existing = self.items.iter().position(|item| item.address == address);
        let key = existing.map_or_else(
            || {
                self.next_key += 1;
                TrayItemKey(self.next_key)
            },
            |at| self.items[at].key,
        );
        let mut item = TrayItem::from_properties(key, address, owner, &properties);
        item.pid = pid;
        match existing {
            Some(at) if self.items[at] == item => false,
            Some(at) => {
                self.items[at] = item;
                true
            }
            None => {
                self.items.push(item);
                true
            }
        }
    }

    /// Forgets the items of the connection `name` (unique or well-known),
    /// which left the bus; returns whether any were shown.
    fn forget_connection(&mut self, name: &str) -> bool {
        let before = self.items.len();
        self.items
            .retain(|item| item.owner != name && item.address.bus != name);
        if self.role == Some(TrayRole::Watcher)
            && let Some(connection) = self.connection.clone()
        {
            let name = name.to_owned();
            self.schedule(async move {
                Self::unlist(&connection, &name).await.map_or_else(
                    |error| TrayEvent::CallFailed(error.to_string()),
                    |()| TrayEvent::Done,
                )
            });
        }
        self.items.len() != before
    }

    /// Reads the item at `address`, then tells the tray.
    fn fetch(&self, address: ItemAddress) {
        let Some(connection) = self.connection.clone() else {
            return;
        };
        self.schedule(async move {
            let properties = Self::read(&connection, &address)
                .await
                .map_err(|error| error.to_string());
            TrayEvent::Fetched {
                address,
                properties,
            }
        });
    }

    /// Starts hearing items' announcements and connections leaving, and,
    /// hosting for another watcher, its announcements.
    fn listen(&mut self, connected: &TrayConnection) {
        self.connection = Some(connected.connection.clone());
        self.role = Some(connected.role);
        let connection = connected.connection.clone();
        let role = connected.role;
        let news = stream::once(async move { Self::news(&connection, role).await }).flat_map(
            |subscribed| match subscribed {
                Ok(news) => Either::Left(news),
                Err(error) => {
                    tracing::warn!(
                        error = &error as &dyn std::error::Error,
                        "the tray will not hear items change"
                    );
                    Either::Right(stream::empty())
                }
            },
        );
        let hooks = self.hooks;
        match StreamSource::start(&self.handle, &self.ledger, news, move |event, state| {
            Self::deliver(state, event, hooks);
        }) {
            Ok(source) => self.news = Some(source),
            Err(error) => tracing::warn!(%error, "the tray will not hear items change"),
        }
    }

    /// Schedules `work` on the tray's executor.
    fn schedule(&self, work: impl Future<Output = TrayEvent> + 'static) {
        if self.scheduler.schedule(work).is_err() {
            tracing::warn!("the tray's executor is gone");
        }
    }

    /// Connects to `bus`, serving `watcher` if the watcher's name is free,
    /// else registering as a host with the program that has it.
    async fn connect(bus: &Bus, watcher: Watcher) -> zbus::Result<TrayConnection> {
        let connection = bus
            .builder()
            .serve_at(WATCHER_PATH, watcher)
            .build()
            .await?;
        // Another program having the name is an answer, not a failure.
        let is_watcher = match connection
            .request_name_with_flags(WATCHER_NAME, RequestNameFlags::DO_NOT_QUEUE)
            .await
        {
            Ok(reply) => reply == RequestNameReply::PrimaryOwner,
            Err(zbus::Error::NameTaken) => false,
            Err(error) => return Err(error),
        };
        if is_watcher {
            Watcher::status_notifier_host_registered(&SignalEmitter::new(
                &connection,
                WATCHER_PATH,
            )?)
            .await?;
            return Ok(TrayConnection {
                connection,
                role: TrayRole::Watcher,
            });
        }
        connection
            .object_server()
            .remove::<Watcher, _>(WATCHER_PATH)
            .await?;
        let host = format!("org.kde.StatusNotifierHost-{}", std::process::id());
        connection.request_name(host.as_str()).await?;
        StatusNotifierWatcherProxy::new(&connection)
            .await?
            .register_status_notifier_host(&host)
            .await?;
        Ok(TrayConnection {
            connection,
            role: TrayRole::Host,
        })
    }

    /// Returns what the tray hears on `connection`: items announcing
    /// changes, connections leaving and, hosting for another watcher, the
    /// items it lists and then its items coming and going. The list is
    /// read after subscribing, so no item registering in between is missed.
    async fn news(
        connection: &Connection,
        role: TrayRole,
    ) -> zbus::Result<impl Stream<Item = TrayEvent> + 'static + use<State>> {
        let item_signals = MatchRule::builder()
            .msg_type(MessageType::Signal)
            .interface(ITEM_INTERFACE)
            .build()?;
        let changes = MessageStream::for_match_rule(item_signals, connection, None)
            .await?
            .filter_map(|message| async move {
                let message = message.ok()?;
                let header = message.header();
                Some(TrayEvent::Changed {
                    owner: header.sender()?.to_string(),
                    path: header.path()?.to_string(),
                })
            });
        let leaving = DBusProxy::new(connection)
            .await?
            .receive_name_owner_changed()
            .await?
            .filter_map(|signal| async move {
                let arguments = signal.args().ok()?;
                arguments
                    .new_owner()
                    .is_none()
                    .then(|| TrayEvent::OwnerGone(arguments.name().to_string()))
            });
        let mut streams = vec![changes.boxed_local(), leaving.boxed_local()];
        if role == TrayRole::Host {
            let watcher = StatusNotifierWatcherProxy::new(connection).await?;
            streams.push(
                watcher
                    .receive_status_notifier_item_registered()
                    .await?
                    .filter_map(|signal| async move {
                        let service = signal.args().ok()?.service().to_owned();
                        Some(TrayEvent::Registered(ItemAddress::listed(service)))
                    })
                    .boxed_local(),
            );
            streams.push(
                watcher
                    .receive_status_notifier_item_unregistered()
                    .await?
                    .filter_map(|signal| async move {
                        let service = signal.args().ok()?.service().to_owned();
                        Some(TrayEvent::Unregistered(ItemAddress::listed(service)))
                    })
                    .boxed_local(),
            );
            let listed = watcher.registered_status_notifier_items().await?;
            streams.push(
                stream::iter(listed)
                    .map(|listed| TrayEvent::Registered(ItemAddress::listed(&listed)))
                    .boxed_local(),
            );
        }
        Ok(stream::select_all(streams))
    }

    /// Reads the properties of the item at `address`, with the unique name
    /// of its connection and its process, if the bus says.
    async fn read(connection: &Connection, address: &ItemAddress) -> zbus::Result<ItemRead> {
        let bus = DBusProxy::new(connection).await?;
        let owner = if address.bus.starts_with(':') {
            address.bus.clone()
        } else {
            bus.get_name_owner(address.bus.as_str().try_into()?)
                .await?
                .to_string()
        };
        // Not every bus can say; the item works without it.
        let pid = match owner.as_str().try_into() {
            Ok(name) => bus.get_connection_unix_process_id(name).await.ok(),
            Err(_) => None,
        };
        let properties = PropertiesProxy::builder(connection)
            .destination(owner.as_str())
            .path(address.path.as_str())
            .cache_properties(zbus::proxy::CacheProperties::No)
            .build()
            .await?
            .get_all(ITEM_INTERFACE.try_into()?)
            .await?;
        Ok(ItemRead {
            owner,
            pid,
            properties,
        })
    }

    /// Takes the items of the connection `name` off the list of the watcher
    /// vaht serves, announcing each.
    async fn unlist(connection: &Connection, name: &str) -> zbus::Result<()> {
        let watcher = connection
            .object_server()
            .interface::<_, Watcher>(WATCHER_PATH)
            .await?;
        let gone = watcher.get_mut().await.forget(name);
        if gone.is_empty() {
            return Ok(());
        }
        for listed in &gone {
            Watcher::status_notifier_item_unregistered(watcher.signal_emitter(), listed).await?;
        }
        watcher
            .get()
            .await
            .registered_status_notifier_items_changed(watcher.signal_emitter())
            .await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use calloop::EventLoop;

    use super::*;
    use crate::bus::PrivateBus;

    /// What a test's event loop holds.
    #[derive(Debug, Default)]
    struct Probe {
        /// The tray under test.
        tray: Option<Tray<Probe>>,
        /// The connection serving the test's item, once connected.
        item: Option<Connection>,
    }

    /// How long one dispatch waits for events.
    const WAIT: Duration = Duration::from_millis(50);

    /// How long a test waits for something it expects.
    const PATIENCE: Duration = Duration::from_secs(5);

    /// The hooks that find the probe's tray.
    const HOOKS: TrayHooks<Probe> = TrayHooks {
        on_change: |_| {},
        find: |probe| probe.tray.as_mut(),
    };

    /// An item a test serves, recording the calls it gets.
    struct TestItem {
        /// Its title.
        title: String,
        /// The calls it got.
        calls: Arc<Mutex<Vec<String>>>,
    }

    #[zbus::interface(name = "org.kde.StatusNotifierItem")]
    impl TestItem {
        #[zbus(property)]
        #[expect(
            clippy::unused_self,
            reason = "a D-Bus property, which zbus reads from the object"
        )]
        fn id(&self) -> String {
            "test-item".to_owned()
        }

        #[zbus(property)]
        fn title(&self) -> String {
            self.title.clone()
        }

        #[zbus(property)]
        #[expect(
            clippy::unused_self,
            reason = "a D-Bus property, which zbus reads from the object"
        )]
        fn icon_name(&self) -> String {
            "test-icon".to_owned()
        }

        fn activate(&self, x: i32, y: i32) {
            self.calls
                .lock()
                .expect("not poisoned")
                .push(format!("activate {x} {y}"));
        }

        #[zbus(signal)]
        async fn new_title(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;
    }

    /// A program's world: a loop with a tray, on a bus.
    struct World {
        /// The loop.
        event_loop: EventLoop<'static, Probe>,
        /// The bus.
        bus: Bus,
        /// The bus's daemon, when this world started it.
        _daemon: Option<PrivateBus>,
        /// What the loop holds.
        probe: Probe,
        /// Runs the test's futures.
        scheduler: Scheduler<Option<Connection>>,
        /// Keeps the test's executor registered.
        _executor: EventSource<Probe>,
        /// The calls the item got.
        calls: Arc<Mutex<Vec<String>>>,
    }

    impl World {
        /// A world on a bus of its own.
        fn new() -> Self {
            let daemon = PrivateBus::start();
            Self::on(daemon.bus(), Some(daemon))
        }

        /// Another world on `other`'s bus: another program.
        fn beside(other: &Self) -> Self {
            Self::on(other.bus.clone(), None)
        }

        /// A world on `bus`.
        fn on(bus: Bus, daemon: Option<PrivateBus>) -> Self {
            let event_loop: EventLoop<'static, Probe> = EventLoop::try_new().expect("a loop");
            let (executor, scheduler) = executor().expect("an executor");
            let executor = EventSource::insert(
                &event_loop.handle(),
                &ResourceLedger::new(),
                executor,
                |connected: Option<Connection>, (), probe: &mut Probe| {
                    if connected.is_some() {
                        probe.item = connected;
                    }
                },
            )
            .expect("registered");
            Self {
                event_loop,
                bus,
                _daemon: daemon,
                probe: Probe::default(),
                scheduler,
                _executor: executor,
                calls: Arc::default(),
            }
        }

        /// Starts the tray, and waits until it is on the bus.
        fn start_tray(&mut self) {
            let tray = Tray::start(
                &self.event_loop.handle(),
                &ResourceLedger::new(),
                self.bus.clone(),
                HOOKS,
            )
            .expect("started");
            self.probe.tray = Some(tray);
            self.dispatch_until(|probe| {
                probe.tray.as_ref().is_some_and(|tray| tray.role.is_some())
            });
        }

        /// Returns the tray's role on the bus.
        fn role(&self) -> Option<TrayRole> {
            self.probe.tray.as_ref().and_then(|tray| tray.role)
        }

        /// Serves the test's item titled `title`, and registers it.
        fn start_item(&mut self, title: &str) {
            let bus = self.bus.clone();
            let item = TestItem {
                title: title.to_owned(),
                calls: Arc::clone(&self.calls),
            };
            self.scheduler
                .schedule(async move {
                    let connection = bus
                        .builder()
                        .serve_at(ItemAddress::DEFAULT_PATH, item)
                        .build()
                        .await
                        .expect("connected");
                    connection
                        .call_method(
                            Some(WATCHER_NAME),
                            WATCHER_PATH,
                            Some(WATCHER_NAME),
                            "RegisterStatusNotifierItem",
                            &(ItemAddress::DEFAULT_PATH,),
                        )
                        .await
                        .expect("registered");
                    Some(connection)
                })
                .expect("scheduled");
        }

        /// Retitles the item, announcing it.
        fn retitle_item(&self, title: &str) {
            let connection = self.probe.item.clone().expect("the item is on the bus");
            let title = title.to_owned();
            self.scheduler
                .schedule(async move {
                    let item = connection
                        .object_server()
                        .interface::<_, TestItem>(ItemAddress::DEFAULT_PATH)
                        .await
                        .expect("served");
                    item.get_mut().await.title = title;
                    TestItem::new_title(item.signal_emitter())
                        .await
                        .expect("announced");
                    None
                })
                .expect("scheduled");
        }

        /// Returns the titles the tray shows.
        fn titles(probe: &Probe) -> Vec<String> {
            probe.tray.as_ref().map_or_else(Vec::new, |tray| {
                tray.ordered(&[], &[])
                    .iter()
                    .map(|item| item.title.clone())
                    .collect()
            })
        }

        /// Dispatches until `done` holds of the probe, or a while has
        /// passed.
        fn dispatch_until(&mut self, done: impl Fn(&Probe) -> bool) {
            let deadline = Instant::now() + PATIENCE;
            while !done(&self.probe) && Instant::now() < deadline {
                self.event_loop
                    .dispatch(WAIT, &mut self.probe)
                    .expect("dispatch");
            }
        }
    }

    #[test]
    fn an_item_registers_changes_is_clicked_and_leaves() {
        let mut world = World::new();
        world.start_tray();
        assert_eq!(world.role(), Some(TrayRole::Watcher));
        world.start_item("Music");
        world.dispatch_until(|probe| World::titles(probe) == ["Music"]);
        assert_eq!(World::titles(&world.probe), ["Music"]);
        let tray = world.probe.tray.as_ref().expect("started");
        let item = tray.ordered(&[], &[])[0];
        assert_eq!(item.id, "test-item");
        assert!(matches!(&item.icon, ItemIcon::Named { name, .. } if name == "test-icon"));
        assert_eq!(
            tray.pid_of(item.key),
            Some(std::process::id()),
            "the bus says which program the item is, for activation grace"
        );

        world.retitle_item("Paused");
        world.dispatch_until(|probe| World::titles(probe) == ["Paused"]);
        assert_eq!(
            World::titles(&world.probe),
            ["Paused"],
            "the announcement was heard"
        );

        let tray = world.probe.tray.as_ref().expect("started");
        let key = tray.ordered(&[], &[])[0].key;
        tray.act(key, TrayAction::Activate, (5, 7).into());
        let calls = Arc::clone(&world.calls);
        world.dispatch_until(|_| !calls.lock().expect("not poisoned").is_empty());
        assert_eq!(*world.calls.lock().expect("not poisoned"), ["activate 5 7"]);

        world.probe.item = None;
        world.dispatch_until(|probe| World::titles(probe).is_empty());
        assert!(
            World::titles(&world.probe).is_empty(),
            "gone with its connection"
        );
    }

    #[test]
    fn with_another_watcher_the_tray_hosts_for_it() {
        let mut serving = World::new();
        serving.start_tray();
        let mut hosting = World::beside(&serving);
        hosting.start_tray();
        assert_eq!(hosting.role(), Some(TrayRole::Host));
        serving.start_item("Music");
        serving.dispatch_until(|probe| World::titles(probe) == ["Music"]);
        hosting.dispatch_until(|probe| World::titles(probe) == ["Music"]);
        assert_eq!(
            World::titles(&hosting.probe),
            ["Music"],
            "heard from the watcher"
        );
        serving.probe.item = None;
        hosting.dispatch_until(|probe| World::titles(probe).is_empty());
        assert_eq!(
            World::titles(&hosting.probe),
            Vec::<String>::new(),
            "gone with it"
        );
    }

    #[test]
    fn pinned_items_come_first_and_last() {
        let mut world = World::new();
        world.start_tray();
        let tray = world.probe.tray.as_mut().expect("started");
        for id in ["a", "b", "c"] {
            let text = |text: &str| zbus::Variant::from(text).try_to_owned().expect("plain");
            let properties = HashMap::from([
                ("Id".to_owned(), text(id)),
                ("Title".to_owned(), text(&id.to_uppercase())),
            ]);
            tray.take_properties(
                ItemAddress::listed(&format!(":1.{id}")),
                Ok(ItemRead {
                    owner: format!(":1.{id}"),
                    pid: None,
                    properties,
                }),
            );
        }
        let order: Vec<String> = tray
            .ordered(&["C".to_owned()], &["a".to_owned()])
            .iter()
            .map(|item| item.title.clone())
            .collect();
        assert_eq!(order, ["C", "B", "A"]);
    }

    #[test]
    fn buttons_ask_for_what_they_mean() {
        assert_eq!(TrayAction::for_button(0x110), Some(TrayAction::Activate));
        assert_eq!(TrayAction::for_button(0x111), Some(TrayAction::ContextMenu));
        assert_eq!(
            TrayAction::for_button(0x112),
            Some(TrayAction::SecondaryActivate)
        );
        assert_eq!(TrayAction::for_button(0x113), None);
    }
}
