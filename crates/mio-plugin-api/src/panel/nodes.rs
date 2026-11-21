use std::collections::HashSet;

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

    pub fn find(&self, id: u64) -> Option<&Node> {
        if self.id == id {
            return Some(self);
        }
        match &self.kind {
            Some(node::Kind::Row(c) | node::Kind::Column(c) | node::Kind::ScrollColumn(c)) => {
                c.children.iter().find_map(|n| n.find(id))
            }
            Some(node::Kind::Split(s)) => s
                .first
                .as_deref()
                .and_then(|n| n.find(id))
                .or_else(|| s.second.as_deref().and_then(|n| n.find(id))),
            Some(node::Kind::Tabs(t)) => t
                .tabs
                .iter()
                .find_map(|t| t.content.as_deref().and_then(|n| n.find(id))),
            _ => None,
        }
    }

    pub(super) fn collect_ids(&self, out: &mut HashSet<u64>) {
        out.insert(self.id);
        match &self.kind {
            Some(node::Kind::Row(c) | node::Kind::Column(c) | node::Kind::ScrollColumn(c)) => {
                for child in &c.children {
                    child.collect_ids(out);
                }
            }
            Some(node::Kind::Split(s)) => {
                if let Some(first) = &s.first {
                    first.collect_ids(out);
                }
                if let Some(second) = &s.second {
                    second.collect_ids(out);
                }
            }
            Some(node::Kind::Tabs(t)) => {
                for tab in &t.tabs {
                    if let Some(content) = &tab.content {
                        content.collect_ids(out);
                    }
                }
            }
            _ => {}
        }
    }

    pub fn editors<'a>(&'a self, output: &mut Vec<(u64, &'a Editor)>) {
        match &self.kind {
            Some(node::Kind::Editor(editor)) => output.push((self.id, editor)),
            Some(node::Kind::Row(c) | node::Kind::Column(c) | node::Kind::ScrollColumn(c)) => {
                for n in &c.children {
                    n.editors(output);
                }
            }
            Some(node::Kind::Split(s)) => {
                if let Some(n) = &s.first {
                    n.editors(output);
                }
                if let Some(n) = &s.second {
                    n.editors(output);
                }
            }
            Some(node::Kind::Tabs(t)) => {
                for tab in &t.tabs {
                    if let Some(n) = &tab.content {
                        n.editors(output);
                    }
                }
            }
            _ => {}
        }
    }

    pub(super) fn preserve_table_revisions_with(
        &self,
        next_root: &Node,
        allow_vp: &mut impl FnMut(u64, &VirtualTable, &VirtualTable) -> bool,
    ) -> Result<(), String> {
        if let Some(node::Kind::VirtualTable(previous)) = &self.kind
            && let Some(node::Kind::VirtualTable(next)) =
                next_root.find(self.id).and_then(|node| node.kind.as_ref())
        {
            check(
                allow_vp(self.id, previous, next),
                "表格 viewport revision 已过期；需要重新同步",
            )?;
        }
        match &self.kind {
            Some(node::Kind::Row(c) | node::Kind::Column(c) | node::Kind::ScrollColumn(c)) => {
                for node in &c.children {
                    node.preserve_table_revisions_with(next_root, allow_vp)?;
                }
            }
            Some(node::Kind::Split(split)) => {
                if let Some(node) = &split.first {
                    node.preserve_table_revisions_with(next_root, allow_vp)?;
                }
                if let Some(node) = &split.second {
                    node.preserve_table_revisions_with(next_root, allow_vp)?;
                }
            }
            Some(node::Kind::Tabs(tabs)) => {
                for tab in &tabs.tabs {
                    if let Some(node) = &tab.content {
                        node.preserve_table_revisions_with(next_root, allow_vp)?;
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    pub(super) fn find_mut(&mut self, id: u64) -> Option<&mut Node> {
        if self.id == id {
            return Some(self);
        }
        match &mut self.kind {
            Some(node::Kind::Row(c) | node::Kind::Column(c) | node::Kind::ScrollColumn(c)) => {
                c.children.iter_mut().find_map(|n| n.find_mut(id))
            }
            Some(node::Kind::Split(s)) => s
                .first
                .as_deref_mut()
                .and_then(|n| n.find_mut(id))
                .or_else(|| s.second.as_deref_mut().and_then(|n| n.find_mut(id))),
            Some(node::Kind::Tabs(t)) => t
                .tabs
                .iter_mut()
                .find_map(|t| t.content.as_deref_mut().and_then(|n| n.find_mut(id))),
            _ => None,
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
