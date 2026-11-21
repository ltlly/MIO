use mio_plugin_api::{
    panel::{
        Axis, BadgeNode, Button, Canvas, CheckboxNode, Container, DrawCommand, Editor,
        EditorFormat, HitRegion, Layout, Node, PanelPatch, PanelSnapshot, PatchOperation,
        ProgressNode, SelectNode, Split, Tab, TableColumn, TableRow, Tabs, Text, Tone,
        VirtualTable, node,
    },
    protocol::{FindingConfidence, FindingSeverity, PluginFindingDto},
};

/// Build a validated-revision `PanelSnapshot`.
pub fn snapshot(revision: u64, title: impl Into<String>, root: Node) -> PanelSnapshot {
    PanelSnapshot {
        revision: revision.max(1),
        title: title.into(),
        root: Some(root),
    }
}

/// Build an incremental `PanelPatch` from `base_revision` to `revision`.
pub fn patch(
    base_revision: u64,
    revision: u64,
    operations: Vec<PatchOperation>,
) -> PanelPatch {
    PanelPatch {
        base_revision,
        revision: revision.max(1),
        operations,
    }
}

pub fn text(id: u64, label: impl Into<String>, content: impl Into<String>, tone: Tone) -> Node {
    Node::new(
        id,
        label,
        node::Kind::Text(Text {
            text: content.into(),
            tone: tone as i32,
        }),
    )
}

pub fn button(
    id: u64,
    label: impl Into<String>,
    enabled: bool,
    commit_editors: bool,
) -> Node {
    let text = label.into();
    Node::new(
        id,
        text.clone(),
        node::Kind::Button(Button {
            label: text,
            enabled,
            commit_editors,
        }),
    )
}

pub fn checkbox(id: u64, label: impl Into<String>, checked: bool, enabled: bool) -> Node {
    let text = label.into();
    Node::new(
        id,
        text.clone(),
        node::Kind::Checkbox(CheckboxNode {
            label: text,
            checked,
            enabled,
        }),
    )
}

pub fn select(
    id: u64,
    label: impl Into<String>,
    options: Vec<String>,
    selected_index: u32,
    enabled: bool,
) -> Node {
    let text = label.into();
    Node::new(
        id,
        text.clone(),
        node::Kind::Select(SelectNode {
            label: text,
            options,
            selected_index,
            enabled,
        }),
    )
}

pub fn badge(id: u64, text: impl Into<String>, tone: Tone) -> Node {
    let content = text.into();
    Node::new(
        id,
        content.clone(),
        node::Kind::Badge(BadgeNode {
            text: content,
            tone: tone as i32,
        }),
    )
}

pub fn progress(id: u64, label: impl Into<String>, percent: u32) -> Node {
    let text = label.into();
    Node::new(
        id,
        text.clone(),
        node::Kind::Progress(ProgressNode {
            label: text,
            percent: percent.min(100),
        }),
    )
}

/// Build an `Editor` widget node.
pub fn editor(
    id: u64,
    label: impl Into<String>,
    document_id: u64,
    revision: u64,
    contents: impl Into<Vec<u8>>,
    format: EditorFormat,
    read_only: bool,
) -> Node {
    Node::new(
        id,
        label,
        node::Kind::Editor(Editor {
            document_id,
            revision: revision.max(1),
            contents: contents.into(),
            format: format as i32,
            read_only,
        }),
    )
}

/// Build a `TableColumn` descriptor for `virtual_table`.
pub fn table_column(
    id: impl Into<String>,
    title: impl Into<String>,
    width: f32,
) -> TableColumn {
    TableColumn {
        id: id.into(),
        title: title.into(),
        width,
    }
}

/// Build a `TableRow` entry for `virtual_table`.
pub fn table_row(id: impl Into<String>, cells: Vec<String>) -> TableRow {
    TableRow {
        id: id.into(),
        cells,
    }
}

/// Build a `VirtualTable` widget node.
#[allow(clippy::too_many_arguments)]
pub fn virtual_table(
    id: u64,
    label: impl Into<String>,
    columns: Vec<TableColumn>,
    total_rows: u64,
    offset: u64,
    viewport_revision: u64,
    rows: Vec<TableRow>,
    multi_select: bool,
) -> Node {
    Node::new(
        id,
        label,
        node::Kind::VirtualTable(VirtualTable {
            columns,
            total_rows,
            offset,
            viewport_revision,
            rows,
            multi_select,
        }),
    )
}

/// Build a two-pane `Split` container node.
pub fn split(
    id: u64,
    label: impl Into<String>,
    axis: Axis,
    fraction: f32,
    first: Node,
    second: Node,
) -> Node {
    let clamped = if fraction.is_finite() {
        fraction.clamp(0.05, 0.95)
    } else {
        0.5
    };
    Node::new(
        id,
        label,
        node::Kind::Split(Split {
            axis: axis as i32,
            fraction: clamped,
            first: Some(Box::new(first)),
            second: Some(Box::new(second)),
        }),
    )
}

/// Build a single `Tab` entry for `tabs`.
pub fn tab(title: impl Into<String>, content: Node) -> Tab {
    Tab {
        title: title.into(),
        content: Some(Box::new(content)),
    }
}

/// Build a `Tabs` container node.
pub fn tabs(
    id: u64,
    label: impl Into<String>,
    axis: Axis,
    active: u64,
    tabs: Vec<Tab>,
) -> Node {
    Node::new(
        id,
        label,
        node::Kind::Tabs(Tabs {
            axis: axis as i32,
            active,
            tabs,
        }),
    )
}

/// Build a `Canvas` widget node.
pub fn canvas(
    id: u64,
    label: impl Into<String>,
    width: f32,
    height: f32,
    commands: Vec<DrawCommand>,
    hit_regions: Vec<HitRegion>,
) -> Node {
    Node::new(
        id,
        label,
        node::Kind::Canvas(Canvas {
            width,
            height,
            commands,
            hit_regions,
        }),
    )
}

pub fn row(id: u64, label: impl Into<String>, children: Vec<Node>, gap: f32) -> Node {
    let mut node = Node::new(id, label, node::Kind::Row(Container { children }));
    node.layout = Some(Layout {
        min_width: 0.0,
        min_height: 0.0,
        flex: 0.0,
        padding: 0.0,
        gap,
    });
    node
}

pub fn column(id: u64, label: impl Into<String>, children: Vec<Node>, gap: f32) -> Node {
    let mut node = Node::new(id, label, node::Kind::Column(Container { children }));
    node.layout = Some(Layout {
        min_width: 0.0,
        min_height: 0.0,
        flex: 0.0,
        padding: 0.0,
        gap,
    });
    node
}

pub fn scroll_column(id: u64, label: impl Into<String>, children: Vec<Node>, gap: f32) -> Node {
    let mut node = Node::new(id, label, node::Kind::ScrollColumn(Container { children }));
    node.layout = Some(Layout {
        min_width: 0.0,
        min_height: 0.0,
        flex: 1.0,
        padding: 4.0,
        gap,
    });
    node
}

/// Fluent builder for constructing a `PluginFindingDto`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FindingBuilder {
    title: String,
    severity: FindingSeverity,
    confidence: FindingConfidence,
    detail: String,
    remediation: String,
    parameter: String,
    evidence: String,
}

impl FindingBuilder {
    /// Start a finding builder with a required title and default `Information` / `Firm` metadata.
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            severity: FindingSeverity::Information,
            confidence: FindingConfidence::Firm,
            detail: String::new(),
            remediation: String::new(),
            parameter: String::new(),
            evidence: String::new(),
        }
    }

    pub fn severity(mut self, severity: FindingSeverity) -> Self {
        self.severity = severity;
        self
    }

    pub fn confidence(mut self, confidence: FindingConfidence) -> Self {
        self.confidence = confidence;
        self
    }

    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = detail.into();
        self
    }

    pub fn remediation(mut self, remediation: impl Into<String>) -> Self {
        self.remediation = remediation.into();
        self
    }

    pub fn parameter(mut self, parameter: impl Into<String>) -> Self {
        self.parameter = parameter.into();
        self
    }

    pub fn evidence(mut self, evidence: impl Into<String>) -> Self {
        self.evidence = evidence.into();
        self
    }

    pub fn build(self) -> PluginFindingDto {
        PluginFindingDto {
            title: self.title,
            detail: self.detail,
            remediation: self.remediation,
            severity: self.severity as i32,
            confidence: self.confidence as i32,
            parameter: self.parameter,
            evidence: self.evidence,
        }
    }
}

impl From<FindingBuilder> for PluginFindingDto {
    fn from(builder: FindingBuilder) -> Self {
        builder.build()
    }
}

/// Start a fluent `FindingBuilder` for passive/active scan checks and audit submissions.
pub fn finding(title: impl Into<String>) -> FindingBuilder {
    FindingBuilder::new(title)
}
