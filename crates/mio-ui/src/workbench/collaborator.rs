use std::sync::Arc;

use gpui_kit::accesskit::Role;
use gpui_kit::component::{
    input::{Editor, Input, InputEvent, InputState},
    menu::{ContextMenuExt, PopupMenu, PopupMenuItem},
    resizable::{resizable_panel, v_resizable},
};
use gpui_kit::{
    AnyElement, ClipboardItem, Context, Entity, FontWeight, MouseButton, RenderImage, Subscription,
    WeakEntity, Window, img,
};
use mio_core::{
    CollaboratorCommand, CollaboratorEvent, CollaboratorStatus, Command,
    MAX_COLLABORATOR_BATCH_PAYLOADS, MAX_COLLABORATOR_INTERACTIONS, OastInteraction,
    OastPayloadToken, OastProtocol, SelfHostedOastConfig, Uuid,
};

use super::*;

mod part_01;
mod part_02;
mod part_03;
mod part_04;

pub(in crate::workbench) use part_01::*;
