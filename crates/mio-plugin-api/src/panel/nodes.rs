use std::{collections::HashSet, ops::ControlFlow};

use super::{validators::*, *};

impl Node {
    pub fn new(id: u64, label: impl Into<String>, kind: node::Kind) -> Self {
        Self {
            id,
            accessibility_label: label.into(),
            layout: None,
            kind: Some(kind),
        }
    }

    fn visit_children<'a, B>(
        &'a self,
        f: &mut impl FnMut(&'a Node) -> ControlFlow<B>,
    ) -> ControlFlow<B> {
        match &self.kind {
            Some(node::Kind::Row(c) | node::Kind::Column(c) | node::Kind::ScrollColumn(c)) => {
                for child in &c.children {
                    f(child)?;
                }
            }
            Some(node::Kind::Split(s)) => {
                if let Some(first) = &s.first {
                    f(first)?;
                }
                if let Some(second) = &s.second {
                    f(second)?;
                }
            }
            Some(node::Kind::Tabs(t)) => {
                for tab in &t.tabs {
                    if let Some(content) = &tab.content {
                        f(content)?;
                    }
                }
            }
            _ => {}
        }
        ControlFlow::Continue(())
    }

    fn visit_children_mut<'a, B>(
        &'a mut self,
        f: &mut impl FnMut(&'a mut Node) -> ControlFlow<B>,
    ) -> ControlFlow<B> {
        match &mut self.kind {
            Some(node::Kind::Row(c) | node::Kind::Column(c) | node::Kind::ScrollColumn(c)) => {
                for child in &mut c.children {
                    f(child)?;
                }
            }
            Some(node::Kind::Split(s)) => {
                if let Some(first) = s.first.as_deref_mut() {
                    f(first)?;
                }
                if let Some(second) = s.second.as_deref_mut() {
                    f(second)?;
                }
            }
            Some(node::Kind::Tabs(t)) => {
                for tab in &mut t.tabs {
                    if let Some(content) = tab.content.as_deref_mut() {
                        f(content)?;
                    }
                }
            }
            _ => {}
        }
        ControlFlow::Continue(())
    }

    pub fn try_visit<'a, B>(
        &'a self,
        f: &mut impl FnMut(&'a Node) -> ControlFlow<B>,
    ) -> ControlFlow<B> {
        f(self)?;
        self.visit_children(&mut |child| child.try_visit(f))
    }

    pub fn visit<'a>(&'a self, f: &mut impl FnMut(&'a Node)) {
        let _ = self.try_visit(&mut |node| {
            f(node);
            ControlFlow::<()>::Continue(())
        });
    }

    pub fn visit_mut(&mut self, f: &mut impl FnMut(&mut Node)) {
        f(self);
        let _ = self.visit_children_mut(&mut |child| {
            child.visit_mut(f);
            ControlFlow::<()>::Continue(())
        });
    }

    pub fn find(&self, id: u64) -> Option<&Node> {
        match self.try_visit(&mut |node| {
            if node.id == id {
                ControlFlow::Break(node)
            } else {
                ControlFlow::Continue(())
            }
        }) {
            ControlFlow::Break(node) => Some(node),
            ControlFlow::Continue(()) => None,
        }
    }

    pub(super) fn collect_ids(&self, out: &mut HashSet<u64>) {
        self.visit(&mut |node| {
            out.insert(node.id);
        });
    }

    pub fn editors<'a>(&'a self, output: &mut Vec<(u64, &'a Editor)>) {
        self.visit(&mut |node| {
            if let Some(node::Kind::Editor(editor)) = &node.kind {
                output.push((node.id, editor));
            }
        });
    }

    pub(super) fn preserve_table_revisions_with(
        &self,
        next_root: &Node,
        allow_vp: &mut impl FnMut(u64, &VirtualTable, &VirtualTable) -> bool,
    ) -> Result<(), String> {
        match self.try_visit(&mut |node| {
            if let Some(node::Kind::VirtualTable(previous)) = &node.kind
                && let Some(node::Kind::VirtualTable(next)) =
                    next_root.find(node.id).and_then(|n| n.kind.as_ref())
                && let Err(err) = check(
                    allow_vp(node.id, previous, next),
                    "表格 viewport revision 已过期；需要重新同步",
                )
            {
                return ControlFlow::Break(err);
            }
            ControlFlow::Continue(())
        }) {
            ControlFlow::Break(err) => Err(err),
            ControlFlow::Continue(()) => Ok(()),
        }
    }

    pub(super) fn find_mut(&mut self, id: u64) -> Option<&mut Node> {
        if self.id == id {
            return Some(self);
        }
        match self.visit_children_mut(&mut |child| match child.find_mut(id) {
            Some(found) => ControlFlow::Break(found),
            None => ControlFlow::Continue(()),
        }) {
            ControlFlow::Break(found) => Some(found),
            ControlFlow::Continue(()) => None,
        }
    }

    fn validate(
        &self,
        depth: usize,
        ids: &mut HashSet<u64>,
        documents: &mut HashSet<u64>,
    ) -> Result<(), String> {
        check(
            depth <= MAX_DEPTH && ids.len() < MAX_NODES,
            "Panel 节点数或深度超过限额",
        )?;
        check(
            self.id != 0 && ids.insert(self.id),
            "Panel 节点 ID 为零或重复",
        )?;
        label(&self.accessibility_label, 1024)?;
        if let Some(l) = &self.layout {
            check(
                [l.min_width, l.min_height, l.flex, l.padding, l.gap]
                    .into_iter()
                    .all(dimension),
                "Panel 布局值无效",
            )?;
        }
        let kind = self.kind.as_ref().ok_or("未知或缺少 Panel 节点类型")?;
        match kind {
            node::Kind::Row(c) | node::Kind::Column(c) | node::Kind::ScrollColumn(c) => {
                for child in &c.children {
                    child.validate(depth + 1, ids, documents)?;
                }
            }
            node::Kind::Split(s) => {
                check(
                    Axis::try_from(s.axis).is_ok()
                        && s.fraction.is_finite()
                        && (0.05..=0.95).contains(&s.fraction),
                    "Split 布局无效",
                )?;
                s.first.as_deref().ok_or("Split 缺少首节点")?.validate(
                    depth + 1,
                    ids,
                    documents,
                )?;
                s.second.as_deref().ok_or("Split 缺少次节点")?.validate(
                    depth + 1,
                    ids,
                    documents,
                )?;
            }
            node::Kind::Tabs(t) => {
                check(
                    Axis::try_from(t.axis).is_ok() && !t.tabs.is_empty(),
                    "Tabs 布局无效",
                )?;
                check(
                    t.tabs
                        .iter()
                        .any(|tab| tab.content.as_ref().is_some_and(|n| n.id == t.active)),
                    "Tabs 活动节点不存在",
                )?;
                for tab in &t.tabs {
                    label(&tab.title, 256)?;
                    tab.content.as_deref().ok_or("Tab 缺少内容")?.validate(
                        depth + 1,
                        ids,
                        documents,
                    )?;
                }
            }
            node::Kind::Text(t) => {
                check(
                    t.text.len() <= 16 * 1024 && Tone::try_from(t.tone).is_ok(),
                    "Text 内容或颜色无效",
                )?;
            }
            node::Kind::Button(b) => label(&b.label, 256)?,
            node::Kind::Editor(e) => {
                check(
                    e.document_id != 0 && documents.insert(e.document_id) && e.revision != 0,
                    "Editor 文档 ID 重复或 revision 无效",
                )?;
                check(
                    e.contents.len() <= MAX_EDITOR_BYTES,
                    "编辑器内容超过 256 KiB 限额",
                )?;
                match EditorFormat::try_from(e.format) {
                    Ok(EditorFormat::Utf8) => {
                        std::str::from_utf8(&e.contents)
                            .map_err(|_| "UTF-8 编辑器包含非文本字节")?;
                    }
                    Ok(EditorFormat::Binary) => {}
                    Err(_) => return Err("未知编辑器格式".into()),
                }
            }
            node::Kind::VirtualTable(t) => t.validate()?,
            node::Kind::Canvas(c) => c.validate()?,
            node::Kind::Checkbox(c) => label(&c.label, 256)?,
            node::Kind::Select(s) => {
                label(&s.label, 256)?;
                check(
                    !s.options.is_empty()
                        && s.options.len() <= 128
                        && (s.selected_index as usize) < s.options.len(),
                    "Select 选项为空、超限或索引越界",
                )?;
                for opt in &s.options {
                    label(opt, 256)?;
                }
            }
            node::Kind::Badge(b) => {
                label(&b.text, 256)?;
                check(Tone::try_from(b.tone).is_ok(), "Badge 颜色无效")?;
            }
            node::Kind::Progress(p) => {
                check(
                    p.label.len() <= 256 && p.percent <= 100,
                    "Progress 标签超限或百分比超过 100",
                )?;
            }
        }
        Ok(())
    }
}

impl VirtualTable {
    pub fn validate(&self) -> Result<(), String> {
        check(
            !self.columns.is_empty()
                && self.columns.len() <= 32
                && self.rows.len() <= MAX_TABLE_ROWS,
            "表格列数或页大小超过限额",
        )?;
        check(
            self.offset <= self.total_rows
                && self.rows.len() as u64 <= self.total_rows - self.offset,
            "表格分页范围无效",
        )?;
        let mut columns = HashSet::new();
        for column in &self.columns {
            label(&column.id, 128)?;
            label(&column.title, 256)?;
            check(
                columns.insert(&column.id) && dimension(column.width) && column.width >= 16.0,
                "表格列 ID 或宽度无效",
            )?;
        }
        let mut rows = HashSet::new();
        for row in &self.rows {
            label(&row.id, 128)?;
            check(
                rows.insert(&row.id)
                    && row.cells.len() == self.columns.len()
                    && row.cells.iter().all(|c| c.len() <= 4096),
                "表格行 ID、列数或文本长度无效",
            )?;
        }
        Ok(())
    }
}

impl Canvas {
    pub fn validate(&self) -> Result<(), String> {
        check(
            dimension(self.width)
                && dimension(self.height)
                && self.width > 0.0
                && self.height > 0.0,
            "Canvas 尺寸无效",
        )?;
        check(
            self.commands.len() <= MAX_DRAW_COMMANDS && self.hit_regions.len() <= 256,
            "Canvas 指令或命中区域超过限额",
        )?;
        let (mut clips, mut transforms, mut points) = (0usize, 0usize, 0usize);
        for command in &self.commands {
            match command.op.as_ref().ok_or("未知 Canvas 指令")? {
                draw_command::Op::Rect(r) => {
                    r.bounds.as_ref().ok_or("绘制矩形缺少边界")?.validate()?;
                    check(
                        Tone::try_from(r.tone).is_ok() && dimension(r.radius),
                        "绘制矩形样式无效",
                    )?;
                }
                draw_command::Op::Path(p) => {
                    points += p.points.len();
                    check(
                        p.points.len() >= 2 && p.points.len() <= 1024 && points <= 8192,
                        "Canvas 路径超过限额",
                    )?;
                    check(
                        p.points.iter().all(|p| coordinate(p.x) && coordinate(p.y))
                            && Tone::try_from(p.tone).is_ok()
                            && dimension(p.stroke_width),
                        "Canvas 路径值无效",
                    )?;
                }
                draw_command::Op::Text(t) => {
                    let point = t.origin.as_ref().ok_or("Canvas 文本缺少坐标")?;
                    check(
                        coordinate(point.x)
                            && coordinate(point.y)
                            && t.text.len() <= 4096
                            && Tone::try_from(t.tone).is_ok()
                            && (1.0..=256.0).contains(&t.size),
                        "Canvas 文本无效",
                    )?;
                }
                draw_command::Op::Image(i) => {
                    i.bounds.as_ref().ok_or("Canvas 图片缺少边界")?.validate()?;
                    check(
                        i.pixel_width > 0
                            && i.pixel_height > 0
                            && i.pixel_width <= 2048
                            && i.pixel_height <= 2048
                            && i.png.len() <= 256 * 1024,
                        "Canvas 图片超过限额",
                    )?;
                    check(
                        i.png.len() >= 24
                            && i.png[..8] == *b"\x89PNG\r\n\x1a\n"
                            && i.png[12..16] == *b"IHDR",
                        "Canvas 图片必须是 PNG",
                    )?;
                    check(
                        u32::from_be_bytes(i.png[16..20].try_into().unwrap()) == i.pixel_width
                            && u32::from_be_bytes(i.png[20..24].try_into().unwrap())
                                == i.pixel_height,
                        "PNG 声明尺寸不匹配",
                    )?;
                }
                draw_command::Op::PushClip(r) => {
                    r.validate()?;
                    clips += 1;
                    check(clips <= 16, "Canvas 裁剪栈过深")?;
                }
                draw_command::Op::PopClip(_) => {
                    clips = clips.checked_sub(1).ok_or("Canvas 裁剪栈下溢")?;
                }
                draw_command::Op::PushTransform(t) => {
                    check(
                        t.matrix.len() == 6 && t.matrix.iter().copied().all(coordinate),
                        "Canvas 变换矩阵无效",
                    )?;
                    transforms += 1;
                    check(transforms <= 16, "Canvas 变换栈过深")?;
                }
                draw_command::Op::PopTransform(_) => {
                    transforms = transforms.checked_sub(1).ok_or("Canvas 变换栈下溢")?;
                }
            }
        }
        check(clips == 0 && transforms == 0, "Canvas 绘制栈未闭合")?;
        let mut ids = HashSet::new();
        for hit in &self.hit_regions {
            check(hit.id != 0 && ids.insert(hit.id), "Canvas 命中区域 ID 重复")?;
            label(&hit.accessibility_label, 1024)?;
            let r = hit.bounds.as_ref().ok_or("Canvas 命中区域缺少边界")?;
            r.validate()?;
            check(
                r.x >= 0.0
                    && r.y >= 0.0
                    && r.x + r.width <= self.width
                    && r.y + r.height <= self.height,
                "Canvas 命中区域超出面板",
            )?;
        }
        Ok(())
    }
}

impl PanelSnapshot {
    pub fn validate(&self) -> Result<(), String> {
        check(self.revision != 0, "Panel revision 不能为零")?;
        label(&self.title, 256)?;
        self.root.as_ref().ok_or("Panel 缺少根节点")?.validate(
            0,
            &mut HashSet::new(),
            &mut HashSet::new(),
        )
    }
}

#[cfg(test)]
mod visit_tests {
    use super::*;

    #[test]
    fn visit_and_visit_mut_traverse_all_container_variants() {
        let mut root = Node::new(
            1,
            "root",
            node::Kind::Split(Split {
                axis: Axis::Horizontal as i32,
                fraction: 0.5,
                first: Some(Box::new(Node::new(
                    2,
                    "col",
                    node::Kind::Column(Container {
                        children: vec![Node::new(
                            3,
                            "txt",
                            node::Kind::Text(Text {
                                text: "a".into(),
                                tone: Tone::Foreground as i32,
                            }),
                        )],
                    }),
                ))),
                second: Some(Box::new(Node::new(
                    4,
                    "tabs",
                    node::Kind::Tabs(Tabs {
                        axis: Axis::Horizontal as i32,
                        active: 5,
                        tabs: vec![Tab {
                            title: "t".into(),
                            content: Some(Box::new(Node::new(
                                5,
                                "leaf",
                                node::Kind::Text(Text {
                                    text: "b".into(),
                                    tone: Tone::Foreground as i32,
                                }),
                            ))),
                        }],
                    }),
                ))),
            }),
        );
        let mut seen = Vec::new();
        root.visit(&mut |n| seen.push(n.id));
        assert_eq!(seen, vec![1, 2, 3, 4, 5]);
        assert_eq!(root.find(5).unwrap().id, 5);

        root.visit_mut(&mut |n| {
            if let Some(node::Kind::Text(t)) = &mut n.kind {
                t.text.push('!');
            }
        });
        assert!(matches!(
            root.find(3).and_then(|n| n.kind.as_ref()),
            Some(node::Kind::Text(t)) if t.text == "a!"
        ));
        assert!(matches!(
            root.find(5).and_then(|n| n.kind.as_ref()),
            Some(node::Kind::Text(t)) if t.text == "b!"
        ));
    }
}
