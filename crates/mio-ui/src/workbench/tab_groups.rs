use gpui_kit::component::{button::ButtonVariants, menu::PopupMenu};
use gpui_kit::{AnyWindowHandle, Bounds, WeakEntity, size};
use mio_core::{TabGroup, Uuid};

use super::*;

mod actions;
mod editor;

#[cfg(test)]
use actions::group_editor;

const PALETTE: &[(u8, u32, &str)] = &[
    (7, 0xe2e2fb, "浅紫色"),
    (1, 0xfce5e4, "浅粉色"),
    (2, 0xf5e7d5, "浅橙色"),
    (3, 0xdeecd8, "浅绿色"),
    (5, 0xeff4f8, "浅蓝色"),
    (4, 0xe0fafe, "浅青色"),
    (6, 0xfdeafa, "浅洋红色"),
    (0, 0xd3d3d3, "灰色"),
    (12, 0xb8b9fa, "紫色"),
    (8, 0xedacb0, "粉色"),
    (9, 0xeacb97, "橙色"),
    (10, 0xafd3a0, "绿色"),
    (11, 0xb1c7db, "蓝色"),
    (13, 0xb8e0e7, "青色"),
    (14, 0xdeadbe, "洋红色"),
];

pub(super) fn group_color(color: Option<u8>) -> gpui_kit::Hsla {
    rgb(PALETTE
        .iter()
        .find(|(value, _, _)| Some(*value) == color)
        .map_or(0xe7e7e7, |(_, color, _)| *color))
    .into()
}

struct GroupEditor {
    pub(super) owner: WeakEntity<Workbench>,
    pub(super) owner_window: AnyWindowHandle,
    pub(super) project: ProjectKey,
    pub(super) group: TabGroup,
    pub(super) base_groups: Vec<TabGroup>,
    pub(super) base_members: Vec<(TabId, Option<Uuid>)>,
    pub(super) tabs: Vec<(TabId, String)>,
    pub(super) selected: HashSet<TabId>,
    pub(super) name: Entity<InputState>,
    pub(super) new: bool,
    pub(super) submitting: bool,
    pub(super) pending: Option<(u64, u64)>,
    pub(super) available: bool,
    pub(super) error: Option<String>,
    pub(super) focus: FocusHandle,
    pub(super) _subscription: Subscription,
}

#[cfg(test)]
mod tests;
