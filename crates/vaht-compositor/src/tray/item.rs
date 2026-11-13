//! One tray item as the tray knows it: where it is on the bus, and what it
//! shows, read from its `org.kde.StatusNotifierItem` properties.

use std::collections::HashMap;
use std::path::PathBuf;

use vaht_ui::Icon;
use zbus::OwnedVariant;

/// Where an item is on the bus: a connection and an object on it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct ItemAddress {
    /// The connection's bus name, unique or well-known.
    pub(crate) bus: String,
    /// The item's object path.
    pub(crate) path: String,
}

impl ItemAddress {
    /// Where an item is when it names only its bus name.
    pub(crate) const DEFAULT_PATH: &str = "/StatusNotifierItem";

    /// Returns where the item registering as `service`, from the connection
    /// `sender`, is: `service` names its bus name (the object at the
    /// default path), or its object path on the sender's connection.
    pub(crate) fn registered(service: &str, sender: &str) -> Self {
        if service.starts_with('/') {
            Self {
                bus: sender.to_owned(),
                path: service.to_owned(),
            }
        } else {
            Self::listed(service)
        }
    }

    /// Returns the item a watcher lists as `entry`: a bus name followed by
    /// the object path, or a bus name alone.
    pub(crate) fn listed(entry: &str) -> Self {
        match entry.find('/') {
            Some(slash) => Self {
                bus: entry[..slash].to_owned(),
                path: entry[slash..].to_owned(),
            },
            None => Self {
                bus: entry.to_owned(),
                path: Self::DEFAULT_PATH.to_owned(),
            },
        }
    }

    /// Returns the item as a watcher lists it: its bus name, then its path.
    pub(crate) fn to_listed(&self) -> String {
        format!("{}{}", self.bus, self.path)
    }
}

/// Identifies a tray item for as long as it is in the tray.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct TrayItemKey(pub(crate) u64);

/// What an item shows as its picture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ItemIcon {
    /// An icon by name, looked up in the item's own directory first.
    Named {
        /// The name.
        name: String,
        /// The directory of icons the application brings along.
        own_directory: Option<PathBuf>,
    },
    /// Pixels the item sent.
    Pixels(Icon),
    /// Nothing usable.
    Missing,
}

/// A tray item and what it shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TrayItem {
    /// Identifies it.
    pub(crate) key: TrayItemKey,
    /// Where it is.
    pub(crate) address: ItemAddress,
    /// The unique name of its connection, which its announcements come
    /// from.
    pub(crate) owner: String,
    /// Its program's process, if the bus said: clicking the item lets that
    /// program bring its windows forward for a moment.
    pub(crate) pid: Option<u32>,
    /// The application's name for it, which `pin_left` and `pin_right`
    /// name it by.
    pub(crate) id: String,
    /// Its title.
    pub(crate) title: String,
    /// Its tooltip's text, if it has one.
    pub(crate) tooltip: Option<String>,
    /// Its picture: the attention one while it asks for attention.
    pub(crate) icon: ItemIcon,
    /// Its menu's object path (com.canonical.dbusmenu), if it has one.
    pub(crate) menu: Option<String>,
    /// Whether it is only a menu: a click opens the menu, not the
    /// application.
    pub(crate) is_menu: bool,
}

impl TrayItem {
    /// Returns the item at `address`, on the connection `owner`, known as
    /// `key`, from its `properties`.
    pub(crate) fn from_properties(
        key: TrayItemKey,
        address: ItemAddress,
        owner: String,
        properties: &HashMap<String, OwnedVariant>,
    ) -> Self {
        let text = |name: &str| {
            properties
                .get(name)
                .and_then(|value| String::try_from(value.clone()).ok())
                .filter(|text| !text.is_empty())
        };
        let needs_attention = text("Status").as_deref() == Some("NeedsAttention");
        let (icon_name, icon_pixmap) = if needs_attention {
            ("AttentionIconName", "AttentionIconPixmap")
        } else {
            ("IconName", "IconPixmap")
        };
        let icon = text(icon_name)
            .map(|name| ItemIcon::Named {
                name,
                own_directory: text("IconThemePath").map(PathBuf::from),
            })
            .or_else(|| {
                properties
                    .get(icon_pixmap)
                    .and_then(|value| Pictures::try_from(value.clone()).ok())
                    .and_then(largest_picture)
                    .map(ItemIcon::Pixels)
            })
            .unwrap_or(ItemIcon::Missing);
        let tooltip = properties
            .get("ToolTip")
            .and_then(|value| <(String, Pictures, String, String)>::try_from(value.clone()).ok())
            .and_then(tooltip_text);
        Self {
            key,
            id: text("Id").unwrap_or_else(|| address.bus.clone()),
            title: text("Title").unwrap_or_default(),
            tooltip,
            icon,
            menu: properties
                .get("Menu")
                .and_then(|value| zbus::OwnedObjectPath::try_from(value.clone()).ok())
                .map(|path| path.as_str().to_owned())
                .filter(|path| path != "/"),
            is_menu: properties
                .get("ItemIsMenu")
                .and_then(|value| bool::try_from(value).ok())
                .unwrap_or(false),
            address,
            owner,
            pid: None,
        }
    }
}

/// An item's pictures as it sends them: each its width, height, and
/// ARGB32 pixels in network byte order.
type Pictures = Vec<(i32, i32, Vec<u8>)>;

/// Returns the largest whole picture of `pictures` as an icon.
fn largest_picture(pictures: Pictures) -> Option<Icon> {
    pictures
        .into_iter()
        .filter_map(|(width, height, bytes)| {
            let (width, height) = (u32::try_from(width).ok()?, u32::try_from(height).ok()?);
            Some((width, height, bytes))
        })
        .filter(|(width, height, bytes)| {
            u64::from(*width) * u64::from(*height) * 4 == bytes.len() as u64
        })
        .max_by_key(|(width, height, _)| u64::from(*width) * u64::from(*height))
        .and_then(|(width, height, bytes)| Icon::from_argb_bytes(width, height, &bytes))
}

/// Returns a tooltip's text, as an item sends the tooltip (an icon name,
/// pictures, a title and a description): its title, and its description
/// below, if it has either.
fn tooltip_text(tooltip: (String, Pictures, String, String)) -> Option<String> {
    let (_, _, title, description) = tooltip;
    let text = [title, description]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    (!text.is_empty()).then_some(text)
}

#[cfg(test)]
mod tests {
    use zbus::Variant as Value;

    use super::*;

    #[test]
    fn items_are_found_by_bus_name_or_by_path_on_their_connection() {
        assert_eq!(
            ItemAddress::registered("/org/ayatana/NotificationItem/x", ":1.42"),
            ItemAddress {
                bus: ":1.42".to_owned(),
                path: "/org/ayatana/NotificationItem/x".to_owned()
            }
        );
        let named = ItemAddress::registered("org.kde.StatusNotifierItem-7-1", ":1.42");
        assert_eq!(named.path, ItemAddress::DEFAULT_PATH);
        assert_eq!(ItemAddress::listed(&named.to_listed()), named);
    }

    /// Returns `value` as a property value.
    fn owned(value: &Value<'_>) -> OwnedVariant {
        value.try_to_owned().expect("a plain value")
    }

    #[test]
    fn an_item_shows_its_named_icon_or_its_largest_picture() {
        let address = ItemAddress::listed(":1.5");
        let named = HashMap::from([
            ("Id".to_owned(), owned(&Value::from("music"))),
            ("IconName".to_owned(), owned(&Value::from("audio-player"))),
            (
                "IconThemePath".to_owned(),
                owned(&Value::from("/opt/music/icons")),
            ),
            ("ItemIsMenu".to_owned(), owned(&Value::from(true))),
        ]);
        let item =
            TrayItem::from_properties(TrayItemKey(1), address.clone(), ":1.5".to_owned(), &named);
        assert_eq!(item.id, "music");
        assert!(item.is_menu);
        assert_eq!(
            item.icon,
            ItemIcon::Named {
                name: "audio-player".to_owned(),
                own_directory: Some(PathBuf::from("/opt/music/icons"))
            }
        );
        let red_pixel = [255_u8, 255, 0, 0];
        let pictures = vec![
            (1_i32, 1_i32, red_pixel.to_vec()),
            (2, 2, red_pixel.repeat(4)),
            (3, 3, vec![0; 2]),
        ];
        let pixels = HashMap::from([("IconPixmap".to_owned(), owned(&Value::from(pictures)))]);
        let item = TrayItem::from_properties(TrayItemKey(2), address, ":1.5".to_owned(), &pixels);
        let ItemIcon::Pixels(icon) = item.icon else {
            panic!("pictures become an icon: {:?}", item.icon);
        };
        assert_eq!(icon.size(), 2, "the largest whole one");
        let pixel = icon.pixels().pixel(0, 0).expect("inside");
        assert_eq!((pixel.red(), pixel.alpha()), (255, 255));
        assert_eq!(item.id, ":1.5", "without an id, its bus name");
    }

    #[test]
    fn an_item_asking_for_attention_shows_its_attention_icon() {
        let properties = HashMap::from([
            ("Status".to_owned(), owned(&Value::from("NeedsAttention"))),
            ("IconName".to_owned(), owned(&Value::from("calm"))),
            ("AttentionIconName".to_owned(), owned(&Value::from("alarm"))),
        ]);
        let item = TrayItem::from_properties(
            TrayItemKey(1),
            ItemAddress::listed(":1.5"),
            ":1.5".to_owned(),
            &properties,
        );
        assert!(matches!(item.icon, ItemIcon::Named { name, .. } if name == "alarm"));
    }
}
