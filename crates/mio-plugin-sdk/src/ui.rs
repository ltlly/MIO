use mio_plugin_api::{
    panel::{
        BadgeNode, Button, CheckboxNode, Container, Layout, Node, PanelSnapshot, ProgressNode,
        SelectNode, Text, Tone, node,
    },
    protocol::{FindingConfidence, FindingSeverity, PluginFindingDto},
};

pub fn snapshot(revision: u64, title: impl Into<String>, root: Node) -> PanelSnapshot {
    PanelSnapshot {
        revision: revision.max(1),
        title: title.into(),
        root: Some(root),
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

pub fn finding(
    title: impl Into<String>,
    severity: FindingSeverity,
    confidence: FindingConfidence,
    detail: impl Into<String>,
    remediation: impl Into<String>,
    parameter: impl Into<String>,
    evidence: impl Into<String>,
) -> PluginFindingDto {
    PluginFindingDto {
        title: title.into(),
        detail: detail.into(),
        remediation: remediation.into(),
        severity: severity as i32,
        confidence: confidence as i32,
        parameter: parameter.into(),
        evidence: evidence.into(),
    }
}
