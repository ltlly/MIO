use crate::{InspectorItem, InspectorMutation};
use serde_json::{Map, Value};

const MAX_JSON_INSPECTOR_BYTES: usize = 2 * 1024 * 1024;
const MAX_JSON_DEPTH: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq)]
enum PathSeg {
    Key(String),
    Index(usize),
}

fn is_simple_key(key: &str) -> bool {
    !key.is_empty()
        && !key.contains(['.', '[', ']', '"', '\\'])
        && !key.chars().any(char::is_control)
}

fn push_key_path(prefix: &str, key: &str) -> String {
    if is_simple_key(key) {
        if prefix.is_empty() {
            key.to_owned()
        } else {
            format!("{prefix}.{key}")
        }
    } else {
        let quoted = serde_json::to_string(key).unwrap_or_else(|_| format!("\"{key}\""));
        format!("{prefix}[{quoted}]")
    }
}

fn push_index_path(prefix: &str, index: usize) -> String {
    format!("{prefix}[{index}]")
}

fn looks_like_json_literal(input: &str) -> bool {
    let trimmed = input.trim();
    matches!(trimmed, "true" | "false" | "null")
        || trimmed.starts_with(['"', '{', '['])
        || serde_json::from_str::<serde_json::Number>(trimmed).is_ok()
}

pub(super) fn parse_json_leaf_input(input: &str) -> Value {
    let trimmed = input.trim();
    if looks_like_json_literal(trimmed)
        && let Ok(val) = serde_json::from_str::<Value>(trimmed)
    {
        return val;
    }
    Value::String(input.to_owned())
}

pub(super) fn format_json_leaf_value(value: &Value) -> String {
    match value {
        Value::String(text) => {
            if parse_json_leaf_input(text) == Value::String(text.clone()) {
                text.clone()
            } else {
                serde_json::to_string(text).unwrap_or_else(|_| text.clone())
            }
        }
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

fn collect_leaves(value: &Value, prefix: &str, depth: usize, out: &mut Vec<InspectorItem>) {
    if depth > MAX_JSON_DEPTH {
        return;
    }
    match value {
        Value::Object(map) => {
            if map.is_empty() {
                if !prefix.is_empty() {
                    out.push(InspectorItem {
                        name: prefix.to_owned(),
                        value: "{}".into(),
                    });
                }
                return;
            }
            for (key, child) in map {
                let next = push_key_path(prefix, key);
                collect_leaves(child, &next, depth + 1, out);
            }
        }
        Value::Array(items) => {
            if items.is_empty() {
                if !prefix.is_empty() {
                    out.push(InspectorItem {
                        name: prefix.to_owned(),
                        value: "[]".into(),
                    });
                }
                return;
            }
            for (index, child) in items.iter().enumerate() {
                let next = push_index_path(prefix, index);
                collect_leaves(child, &next, depth + 1, out);
            }
        }
        scalar => {
            if !prefix.is_empty() {
                out.push(InspectorItem {
                    name: prefix.to_owned(),
                    value: format_json_leaf_value(scalar),
                });
            }
        }
    }
}

fn parse_json_container(body: &[u8]) -> Result<(Value, bool, bool), String> {
    if body.len() > MAX_JSON_INSPECTOR_BYTES {
        return Err("JSON 请求正文超过 2 MiB 限额，无法在 Inspector 中编辑。".into());
    }
    let text = std::str::from_utf8(body)
        .map_err(|_| "JSON 请求正文含非 UTF-8 字节，无法在 Inspector 中修改。".to_owned())?;
    let trimmed = text.trim_start_matches('\u{feff}').trim();
    if !(trimmed.starts_with('{') || trimmed.starts_with('[')) {
        return Err("JSON 请求正文根节点必须是对象或数组。".into());
    }
    let value: Value =
        serde_json::from_str(trimmed).map_err(|err| format!("JSON 请求正文解析失败：{err}"))?;
    if !matches!(value, Value::Object(_) | Value::Array(_)) {
        return Err("JSON 请求正文根节点必须是对象或数组。".into());
    }
    Ok((value, trimmed.contains('\n'), text.ends_with('\n')))
}

pub(super) fn inspect_json_body(body: &[u8]) -> Option<Vec<InspectorItem>> {
    let (root, _, _) = parse_json_container(body).ok()?;
    let mut items = Vec::new();
    collect_leaves(&root, "", 0, &mut items);
    Some(items)
}

fn parse_json_path(path: &str) -> Result<Vec<PathSeg>, String> {
    if path.is_empty() {
        return Err("JSON 参数路径不能为空。".into());
    }
    let bytes = path.as_bytes();
    let mut pos = 0;
    let mut segs = Vec::new();
    while pos < bytes.len() {
        if bytes[pos] == b'.' {
            if segs.is_empty() || pos + 1 >= bytes.len() || bytes[pos + 1] == b'.' {
                return Err(format!("JSON 参数路径 `{path}` 语法无效。"));
            }
            pos += 1;
            continue;
        }
        if bytes[pos] == b'[' {
            if bytes.get(pos + 1) == Some(&b'"') {
                let mut end = pos + 2;
                while end < bytes.len() {
                    if bytes[end] == b'\\' {
                        end += 1;
                        if end < bytes.len() {
                            end += path[end..].chars().next().map_or(1, char::len_utf8);
                        }
                        continue;
                    }
                    if bytes[end] == b'"' {
                        break;
                    }
                    end += 1;
                }
                if end >= bytes.len() || bytes.get(end + 1) != Some(&b']') {
                    return Err(format!("JSON 参数路径 `{path}` 引号键未闭合。"));
                }
                let key: String = serde_json::from_str(&path[pos + 1..=end])
                    .map_err(|_| format!("JSON 参数路径 `{path}` 引号键转义无效。"))?;
                segs.push(PathSeg::Key(key));
                pos = end + 2;
            } else {
                let close = path[pos + 1..]
                    .find(']')
                    .map(|rel| pos + 1 + rel)
                    .ok_or_else(|| format!("JSON 参数路径 `{path}` 缺少 `]`。"))?;
                let index_str = &path[pos + 1..close];
                let index: usize = index_str
                    .parse()
                    .map_err(|_| format!("JSON 数组下标 `{index_str}` 无效。"))?;
                segs.push(PathSeg::Index(index));
                pos = close + 1;
            }
        } else {
            let end = path[pos..]
                .find(['.', '['])
                .map(|rel| pos + rel)
                .unwrap_or(bytes.len());
            let key = &path[pos..end];
            if key.is_empty() || key.contains(']') {
                return Err(format!("JSON 参数路径 `{path}` 语法无效。"));
            }
            segs.push(PathSeg::Key(key.to_owned()));
            pos = end;
        }
        if segs.len() > MAX_JSON_DEPTH {
            return Err("JSON 参数路径深度超过 64 层上限。".into());
        }
    }
    Ok(segs)
}

fn set_json_path(current: &mut Value, segs: &[PathSeg], new_val: Value) -> Result<(), String> {
    let Some((first, rest)) = segs.split_first() else {
        *current = new_val;
        return Ok(());
    };
    match (current, first) {
        (Value::Object(map), PathSeg::Key(key)) => {
            if rest.is_empty() {
                map.insert(key.clone(), new_val);
                return Ok(());
            }
            let next_default = || match &rest[0] {
                PathSeg::Key(_) => Value::Object(Map::new()),
                PathSeg::Index(_) => Value::Array(Vec::new()),
            };
            let child = map.entry(key.clone()).or_insert_with(next_default);
            set_json_path(child, rest, new_val)
        }
        (Value::Array(items), PathSeg::Index(index)) => {
            if rest.is_empty() {
                if *index < items.len() {
                    items[*index] = new_val;
                    return Ok(());
                }
                if *index == items.len() {
                    items.push(new_val);
                    return Ok(());
                }
                return Err(format!(
                    "JSON 数组下标 [{index}] 超出当前数组长度 {}。",
                    items.len()
                ));
            }
            if *index == items.len() {
                let next_container = match &rest[0] {
                    PathSeg::Key(_) => Value::Object(Map::new()),
                    PathSeg::Index(_) => Value::Array(Vec::new()),
                };
                items.push(next_container);
            }
            let child = items
                .get_mut(*index)
                .ok_or_else(|| format!("JSON 数组下标 [{index}] 超出范围。"))?;
            set_json_path(child, rest, new_val)
        }
        _ => Err("JSON 参数路径与当前节点容器类型不匹配。".into()),
    }
}

fn remove_json_path(current: &mut Value, segs: &[PathSeg]) -> Result<(), String> {
    let Some((first, rest)) = segs.split_first() else {
        return Err("JSON 参数路径不能为空。".into());
    };
    match (current, first) {
        (Value::Object(map), PathSeg::Key(key)) => {
            if rest.is_empty() {
                if !map.contains_key(key) {
                    return Err("JSON 参数路径不存在。".into());
                }
                *map = std::mem::take(map)
                    .into_iter()
                    .filter(|(existing, _)| existing != key)
                    .collect();
                Ok(())
            } else {
                let child = map
                    .get_mut(key)
                    .ok_or_else(|| "JSON 参数路径不存在。".to_owned())?;
                remove_json_path(child, rest)
            }
        }
        (Value::Array(items), PathSeg::Index(index)) => {
            if *index >= items.len() {
                return Err("JSON 数组下标超出范围。".into());
            }
            if rest.is_empty() {
                items.remove(*index);
                Ok(())
            } else {
                remove_json_path(&mut items[*index], rest)
            }
        }
        _ => Err("JSON 参数路径与当前节点容器类型不匹配。".into()),
    }
}

pub(in crate::http) fn build_json_object_from_pairs(
    pairs: &[(String, String)],
) -> Result<Vec<u8>, String> {
    let mut root = Value::Object(Map::new());
    for (name, value) in pairs {
        let segs = parse_json_path(name)?;
        let val = parse_json_leaf_input(value);
        set_json_path(&mut root, &segs, val)?;
    }
    serde_json::to_vec(&root).map_err(|err| format!("JSON 序列化失败：{err}"))
}

pub(super) fn mutate_json_body(
    original_body: &[u8],
    mutation: InspectorMutation,
) -> Result<Vec<u8>, String> {
    let (mut root, pretty, trailing_nl) = parse_json_container(original_body)?;
    let mut leaves = Vec::new();
    collect_leaves(&root, "", 0, &mut leaves);
    match mutation {
        InspectorMutation::Add { name, value, .. } => {
            let segs = parse_json_path(&name)?;
            let val = parse_json_leaf_input(&value);
            set_json_path(&mut root, &segs, val)?;
        }
        InspectorMutation::Update {
            index, name, value, ..
        } => {
            let old = leaves
                .get(index)
                .ok_or_else(|| "Inspector 项目索引超出范围。".to_owned())?;
            let old_segs = parse_json_path(&old.name)?;
            let new_segs = parse_json_path(&name)?;
            let val = parse_json_leaf_input(&value);
            if old_segs == new_segs {
                set_json_path(&mut root, &new_segs, val)?;
            } else {
                remove_json_path(&mut root, &old_segs)?;
                set_json_path(&mut root, &new_segs, val)?;
            }
        }
        InspectorMutation::Remove { index, .. } => {
            let old = leaves
                .get(index)
                .ok_or_else(|| "Inspector 项目索引超出范围。".to_owned())?;
            let old_segs = parse_json_path(&old.name)?;
            remove_json_path(&mut root, &old_segs)?;
        }
        InspectorMutation::Move { from, to, .. } => {
            if from >= leaves.len() || to >= leaves.len() {
                return Err("Inspector 项目索引超出范围。".into());
            }
            if let Value::Object(map) = &mut root
                && map.len() == leaves.len()
                && from != to
            {
                let mut entries: Vec<(String, Value)> = std::mem::take(map).into_iter().collect();
                let moved = entries.remove(from);
                entries.insert(to, moved);
                *map = entries.into_iter().collect();
            }
        }
    }
    let mut out = if pretty {
        serde_json::to_vec_pretty(&root)
    } else {
        serde_json::to_vec(&root)
    }
    .map_err(|err| format!("JSON 序列化失败：{err}"))?;
    if trailing_nl {
        out.push(b'\n');
    }
    Ok(out)
}
