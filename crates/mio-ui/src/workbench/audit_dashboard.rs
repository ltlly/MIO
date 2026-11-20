use std::collections::{HashMap, HashSet};

use gpui_kit::accesskit::Role;
use gpui_kit::component::{
    button::ButtonVariants,
    checkbox::Checkbox,
    input::{Editor, Input, InputEvent, InputState},
    menu::{ContextMenuExt, DropdownMenu, PopupMenu, PopupMenuItem},
    scroll::{Scrollbar, ScrollbarMode},
    switch::Switch,
};
use gpui_kit::{
    AnyElement, BoxShadow, ClipboardItem, Context, DragMoveEvent, Entity, FontWeight, Hsla,
    MouseButton, MouseDownEvent, Subscription, UniformListScrollHandle, WeakEntity, Window, hsla,
    point,
};
use mio_core::{
    AuditCommand, AuditEvent, AuditStateSnapshot, AuditTaskMode, AuditTaskState, AuditTaskSummary,
    Command, Draft, FlowId, FlowRecord, HttpVersion, ProjectKey, Uuid, WorkTab,
    audit_engine::{
        AuditFinding, AuditPhase, EvidenceSpan, IssueConfidence, render_audit_report_html,
        render_audit_report_json,
    },
    parse_repeater_request, request_to_raw, response_to_raw,
    target_issues::IssueSeverity,
};
use tokio_util::sync::CancellationToken;

use super::*;

mod issue_menu;
mod part_01;
mod part_02;
mod part_03;
mod part_04;
mod part_05;
mod part_06;
mod part_07;
mod workspace;

pub(in crate::workbench) use part_01::*;
pub(in crate::workbench) use part_04::*;
pub(in crate::workbench) use workspace::*;
