use prost::{Enumeration, Message, Oneof};

use super::*;

pub(super) fn check(condition: bool, message: &str) -> Result<(), String> {
    if condition {
        Ok(())
    } else {
        Err(message.into())
    }
}

pub(super) fn coordinate(value: f32) -> bool {
    value.is_finite() && value.abs() <= 16_384.0
}

pub(super) fn dimension(value: f32) -> bool {
    coordinate(value) && value >= 0.0
}

pub(super) fn label(value: &str, maximum: usize) -> Result<(), String> {
    check(
        !value.is_empty() && value.len() <= maximum,
        "标签为空或超过限额",
    )
}

impl Rect {
    pub(super) fn validate(&self) -> Result<(), String> {
        check(
            coordinate(self.x)
                && coordinate(self.y)
                && dimension(self.width)
                && dimension(self.height),
            "Canvas 矩形无效",
        )
    }

    pub(super) fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x <= self.x + self.width && y >= self.y && y <= self.y + self.height
    }
}

impl EditorSnapshot {
    pub fn validate(&self) -> Result<(), String> {
        check(
            self.document_id != 0 && self.revision != 0,
            "编辑器身份或 revision 无效",
        )?;
        check(
            self.contents.len() <= MAX_EDITOR_BYTES,
            "编辑器内容超过 256 KiB 限额",
        )?;
        check(
            self.selection_start <= self.selection_end
                && self.selection_end <= self.contents.len() as u64,
            "编辑器选区无效",
        )
    }
}

#[derive(Clone, PartialEq, Message)]
pub struct PanelPatch {
    #[prost(uint64, tag = "1")]
    pub base_revision: u64,
    #[prost(uint64, tag = "2")]
    pub revision: u64,
    #[prost(message, repeated, tag = "3")]
    pub operations: Vec<PatchOperation>,
}

#[derive(Clone, PartialEq, Message)]
pub struct PatchOperation {
    #[prost(uint64, tag = "1")]
    pub node_id: u64,
    #[prost(oneof = "patch_operation::Change", tags = "2, 3, 4, 5")]
    pub change: Option<patch_operation::Change>,
}

pub mod patch_operation {
    use super::*;

    #[derive(Clone, PartialEq, Oneof)]
    pub enum Change {
        #[prost(message, tag = "2")]
        Replace(Node),
        #[prost(message, tag = "3")]
        Editor(EditorUpdate),
        #[prost(message, tag = "4")]
        Table(VirtualTable),
        #[prost(message, tag = "5")]
        Canvas(Canvas),
    }
}

#[derive(Clone, PartialEq, Message)]
pub struct EditorUpdate {
    #[prost(uint64, tag = "1")]
    pub base_revision: u64,
    #[prost(message, optional, tag = "2")]
    pub snapshot: Option<EditorSnapshot>,
}

#[derive(Clone, PartialEq, Message)]
pub struct UiEvent {
    #[prost(uint64, tag = "1")]
    pub node_id: u64,
    #[prost(
        oneof = "ui_event::Event",
        tags = "2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 14"
    )]
    pub event: Option<ui_event::Event>,
    #[prost(message, repeated, tag = "12")]
    pub editors: Vec<EditorSnapshot>,
}

pub mod ui_event {
    use super::*;

    #[derive(Clone, PartialEq, Oneof)]
    pub enum Event {
        #[prost(message, tag = "2")]
        Click(Empty),
        #[prost(message, tag = "3")]
        EditorChanged(EditorSnapshot),
        #[prost(message, tag = "4")]
        TableSelection(TableSelection),
        #[prost(message, tag = "5")]
        Viewport(ViewportRequest),
        #[prost(message, tag = "6")]
        Pointer(PointerEvent),
        #[prost(message, tag = "7")]
        Key(KeyEvent),
        #[prost(bool, tag = "8")]
        Focus(bool),
        #[prost(uint64, tag = "9")]
        TabSelected(u64),
        #[prost(float, tag = "10")]
        SplitResized(f32),
        #[prost(string, tag = "11")]
        Filter(String),
        #[prost(bool, tag = "13")]
        Toggled(bool),
        #[prost(uint32, tag = "14")]
        SelectedOption(u32),
    }
}

#[derive(Clone, PartialEq, Message)]
pub struct TableSelection {
    #[prost(string, repeated, tag = "1")]
    pub row_ids: Vec<String>,
}

#[derive(Clone, PartialEq, Message)]
pub struct ViewportRequest {
    #[prost(uint64, tag = "1")]
    pub revision: u64,
    #[prost(uint64, tag = "2")]
    pub offset: u64,
    #[prost(uint32, tag = "3")]
    pub limit: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Enumeration)]
#[repr(i32)]
pub enum PointerPhase {
    Down = 0,
    Move = 1,
    Up = 2,
    Cancel = 3,
}

#[derive(Clone, PartialEq, Message)]
pub struct PointerEvent {
    #[prost(uint64, tag = "1")]
    pub hit_id: u64,
    #[prost(enumeration = "PointerPhase", tag = "2")]
    pub phase: i32,
    #[prost(float, tag = "3")]
    pub x: f32,
    #[prost(float, tag = "4")]
    pub y: f32,
}

#[derive(Clone, PartialEq, Message)]
pub struct KeyEvent {
    #[prost(string, tag = "1")]
    pub key: String,
    /// Bit 0 Command, bit 1 Shift, bit 2 Option, bit 3 Control.
    #[prost(uint32, tag = "2")]
    pub modifiers: u32,
}
