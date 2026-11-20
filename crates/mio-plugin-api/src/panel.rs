//! Bounded native panel descriptions. No GPUI objects or executable drawing code cross IPC.

use prost::{Enumeration, Message, Oneof};

pub const MAX_NODES: usize = 512;
pub const MAX_DEPTH: usize = 24;
pub const MAX_EDITOR_BYTES: usize = 256 * 1024;
pub const MAX_TABLE_ROWS: usize = 128;
pub const MAX_DRAW_COMMANDS: usize = 2048;

#[derive(Clone, PartialEq, Message)]
pub struct PanelSnapshot {
    #[prost(uint64, tag = "1")]
    pub revision: u64,
    #[prost(string, tag = "2")]
    pub title: String,
    #[prost(message, optional, tag = "3")]
    pub root: Option<Node>,
}

#[derive(Clone, PartialEq, Message)]
pub struct Node {
    #[prost(uint64, tag = "1")]
    pub id: u64,
    #[prost(string, tag = "2")]
    pub accessibility_label: String,
    #[prost(message, optional, tag = "3")]
    pub layout: Option<Layout>,
    #[prost(
        oneof = "node::Kind",
        tags = "10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23"
    )]
    pub kind: Option<node::Kind>,
}

pub mod node {
    use super::*;

    #[derive(Clone, PartialEq, Oneof)]
    pub enum Kind {
        #[prost(message, tag = "10")]
        Row(Container),
        #[prost(message, tag = "11")]
        Column(Container),
        #[prost(message, tag = "12")]
        Split(Split),
        #[prost(message, tag = "13")]
        Tabs(Tabs),
        #[prost(message, tag = "14")]
        Text(Text),
        #[prost(message, tag = "15")]
        Button(Button),
        #[prost(message, tag = "16")]
        Editor(Editor),
        #[prost(message, tag = "17")]
        VirtualTable(VirtualTable),
        #[prost(message, tag = "18")]
        Canvas(Canvas),
        #[prost(message, tag = "19")]
        ScrollColumn(Container),
        #[prost(message, tag = "20")]
        Checkbox(CheckboxNode),
        #[prost(message, tag = "21")]
        Select(SelectNode),
        #[prost(message, tag = "22")]
        Badge(BadgeNode),
        #[prost(message, tag = "23")]
        Progress(ProgressNode),
    }
}

#[derive(Clone, PartialEq, Message)]
pub struct Layout {
    #[prost(float, tag = "1")]
    pub min_width: f32,
    #[prost(float, tag = "2")]
    pub min_height: f32,
    #[prost(float, tag = "3")]
    pub flex: f32,
    #[prost(float, tag = "4")]
    pub padding: f32,
    #[prost(float, tag = "5")]
    pub gap: f32,
}

#[derive(Clone, PartialEq, Message)]
pub struct Container {
    #[prost(message, repeated, tag = "1")]
    pub children: Vec<Node>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Enumeration)]
#[repr(i32)]
pub enum Axis {
    Horizontal = 0,
    Vertical = 1,
}

#[derive(Clone, PartialEq, Message)]
pub struct Split {
    #[prost(enumeration = "Axis", tag = "1")]
    pub axis: i32,
    #[prost(float, tag = "2")]
    pub fraction: f32,
    #[prost(message, optional, boxed, tag = "3")]
    pub first: Option<Box<Node>>,
    #[prost(message, optional, boxed, tag = "4")]
    pub second: Option<Box<Node>>,
}

#[derive(Clone, PartialEq, Message)]
pub struct Tabs {
    #[prost(enumeration = "Axis", tag = "1")]
    pub axis: i32,
    #[prost(uint64, tag = "2")]
    pub active: u64,
    #[prost(message, repeated, tag = "3")]
    pub tabs: Vec<Tab>,
}

#[derive(Clone, PartialEq, Message)]
pub struct Tab {
    #[prost(string, tag = "1")]
    pub title: String,
    #[prost(message, optional, boxed, tag = "2")]
    pub content: Option<Box<Node>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Enumeration)]
#[repr(i32)]
pub enum Tone {
    Foreground = 0,
    Muted = 1,
    Accent = 2,
    Error = 3,
    Background = 4,
    Border = 5,
    Selection = 6,
}

#[derive(Clone, PartialEq, Message)]
pub struct Text {
    #[prost(string, tag = "1")]
    pub text: String,
    #[prost(enumeration = "Tone", tag = "2")]
    pub tone: i32,
}

#[derive(Clone, PartialEq, Message)]
pub struct Button {
    #[prost(string, tag = "1")]
    pub label: String,
    #[prost(bool, tag = "2")]
    pub enabled: bool,
    /// Capture every editor after committing native IME composition before activating this button.
    #[prost(bool, tag = "3")]
    pub commit_editors: bool,
}

#[derive(Clone, PartialEq, Message)]
pub struct CheckboxNode {
    #[prost(string, tag = "1")]
    pub label: String,
    #[prost(bool, tag = "2")]
    pub checked: bool,
    #[prost(bool, tag = "3")]
    pub enabled: bool,
}

#[derive(Clone, PartialEq, Message)]
pub struct SelectNode {
    #[prost(string, tag = "1")]
    pub label: String,
    #[prost(string, repeated, tag = "2")]
    pub options: Vec<String>,
    #[prost(uint32, tag = "3")]
    pub selected_index: u32,
    #[prost(bool, tag = "4")]
    pub enabled: bool,
}

#[derive(Clone, PartialEq, Message)]
pub struct BadgeNode {
    #[prost(string, tag = "1")]
    pub text: String,
    #[prost(enumeration = "Tone", tag = "2")]
    pub tone: i32,
}

#[derive(Clone, PartialEq, Message)]
pub struct ProgressNode {
    #[prost(string, tag = "1")]
    pub label: String,
    #[prost(uint32, tag = "2")]
    pub percent: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Enumeration)]
#[repr(i32)]
pub enum EditorFormat {
    Utf8 = 0,
    Binary = 1,
}

#[derive(Clone, PartialEq, Message)]
pub struct Editor {
    #[prost(uint64, tag = "1")]
    pub document_id: u64,
    #[prost(uint64, tag = "2")]
    pub revision: u64,
    #[prost(bytes = "vec", tag = "3")]
    pub contents: Vec<u8>,
    #[prost(enumeration = "EditorFormat", tag = "4")]
    pub format: i32,
    #[prost(bool, tag = "5")]
    pub read_only: bool,
}

#[derive(Clone, PartialEq, Message)]
pub struct EditorSnapshot {
    #[prost(uint64, tag = "1")]
    pub document_id: u64,
    #[prost(uint64, tag = "2")]
    pub revision: u64,
    #[prost(bytes = "vec", tag = "3")]
    pub contents: Vec<u8>,
    /// UTF-8 byte offsets for text, byte offsets for binary editors.
    #[prost(uint64, tag = "4")]
    pub selection_start: u64,
    #[prost(uint64, tag = "5")]
    pub selection_end: u64,
}

#[derive(Clone, PartialEq, Message)]
pub struct VirtualTable {
    #[prost(message, repeated, tag = "1")]
    pub columns: Vec<TableColumn>,
    #[prost(uint64, tag = "2")]
    pub total_rows: u64,
    #[prost(uint64, tag = "3")]
    pub offset: u64,
    #[prost(uint64, tag = "4")]
    pub viewport_revision: u64,
    #[prost(message, repeated, tag = "5")]
    pub rows: Vec<TableRow>,
    #[prost(bool, tag = "6")]
    pub multi_select: bool,
}

#[derive(Clone, PartialEq, Message)]
pub struct TableColumn {
    #[prost(string, tag = "1")]
    pub id: String,
    #[prost(string, tag = "2")]
    pub title: String,
    #[prost(float, tag = "3")]
    pub width: f32,
}

#[derive(Clone, PartialEq, Message)]
pub struct TableRow {
    #[prost(string, tag = "1")]
    pub id: String,
    #[prost(string, repeated, tag = "2")]
    pub cells: Vec<String>,
}

#[derive(Clone, PartialEq, Message)]
pub struct Point {
    #[prost(float, tag = "1")]
    pub x: f32,
    #[prost(float, tag = "2")]
    pub y: f32,
}

#[derive(Clone, PartialEq, Message)]
pub struct Rect {
    #[prost(float, tag = "1")]
    pub x: f32,
    #[prost(float, tag = "2")]
    pub y: f32,
    #[prost(float, tag = "3")]
    pub width: f32,
    #[prost(float, tag = "4")]
    pub height: f32,
}

#[derive(Clone, PartialEq, Message)]
pub struct Canvas {
    #[prost(float, tag = "1")]
    pub width: f32,
    #[prost(float, tag = "2")]
    pub height: f32,
    #[prost(message, repeated, tag = "3")]
    pub commands: Vec<DrawCommand>,
    #[prost(message, repeated, tag = "4")]
    pub hit_regions: Vec<HitRegion>,
}

#[derive(Clone, PartialEq, Message)]
pub struct HitRegion {
    #[prost(uint64, tag = "1")]
    pub id: u64,
    #[prost(message, optional, tag = "2")]
    pub bounds: Option<Rect>,
    #[prost(string, tag = "3")]
    pub accessibility_label: String,
    #[prost(bool, tag = "4")]
    pub focusable: bool,
    #[prost(bool, tag = "5")]
    pub capture_pointer: bool,
}

#[derive(Clone, PartialEq, Message)]
pub struct DrawCommand {
    #[prost(oneof = "draw_command::Op", tags = "1, 2, 3, 4, 5, 6, 7, 8")]
    pub op: Option<draw_command::Op>,
}

pub mod draw_command {
    use super::*;

    #[derive(Clone, PartialEq, Oneof)]
    pub enum Op {
        #[prost(message, tag = "1")]
        Rect(DrawRect),
        #[prost(message, tag = "2")]
        Path(DrawPath),
        #[prost(message, tag = "3")]
        Text(DrawText),
        #[prost(message, tag = "4")]
        Image(DrawImage),
        #[prost(message, tag = "5")]
        PushClip(Rect),
        #[prost(message, tag = "6")]
        PopClip(Empty),
        #[prost(message, tag = "7")]
        PushTransform(Transform),
        #[prost(message, tag = "8")]
        PopTransform(Empty),
    }
}

#[derive(Clone, PartialEq, Message)]
pub struct Empty {}

#[derive(Clone, PartialEq, Message)]
pub struct DrawRect {
    #[prost(message, optional, tag = "1")]
    pub bounds: Option<Rect>,
    #[prost(enumeration = "Tone", tag = "2")]
    pub tone: i32,
    #[prost(float, tag = "3")]
    pub radius: f32,
}

#[derive(Clone, PartialEq, Message)]
pub struct DrawPath {
    #[prost(message, repeated, tag = "1")]
    pub points: Vec<Point>,
    #[prost(bool, tag = "2")]
    pub closed: bool,
    #[prost(bool, tag = "3")]
    pub filled: bool,
    #[prost(enumeration = "Tone", tag = "4")]
    pub tone: i32,
    #[prost(float, tag = "5")]
    pub stroke_width: f32,
}

#[derive(Clone, PartialEq, Message)]
pub struct DrawText {
    #[prost(message, optional, tag = "1")]
    pub origin: Option<Point>,
    #[prost(string, tag = "2")]
    pub text: String,
    #[prost(enumeration = "Tone", tag = "3")]
    pub tone: i32,
    #[prost(float, tag = "4")]
    pub size: f32,
}

#[derive(Clone, PartialEq, Message)]
pub struct DrawImage {
    #[prost(message, optional, tag = "1")]
    pub bounds: Option<Rect>,
    #[prost(uint32, tag = "2")]
    pub pixel_width: u32,
    #[prost(uint32, tag = "3")]
    pub pixel_height: u32,
    #[prost(bytes = "vec", tag = "4")]
    pub png: Vec<u8>,
}

#[derive(Clone, PartialEq, Message)]
pub struct Transform {
    /// Six affine coefficients: a, b, c, d, tx, ty.
    #[prost(float, repeated, tag = "1")]
    pub matrix: Vec<f32>,
}

mod editors;
mod nodes;
#[cfg(test)]
mod tests;
mod validators;

pub use editors::*;
pub use validators::{
    EditorUpdate, KeyEvent, PanelPatch, PatchOperation, PointerEvent, PointerPhase, TableSelection,
    UiEvent, ViewportRequest, patch_operation, ui_event,
};
