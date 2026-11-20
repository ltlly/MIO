use std::fmt::Write;

use super::*;

pub(super) const MAX_WORD_UNITS: usize = 2048;
pub(super) const MAX_BYTE_UNITS: usize = 4096;

#[derive(Clone, Copy)]
enum DiffOp {
    Equal(usize, usize),
    Delete(usize),
    Insert(usize),
}

fn lcs_ops<T: PartialEq>(left: &[T], right: &[T]) -> Vec<DiffOp> {
    let n = left.len();
    let m = right.len();
    let stride = m + 1;
    let mut dp = vec![0u16; (n + 1) * stride];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            dp[i * stride + j] = if left[i] == right[j] {
                dp[(i + 1) * stride + j + 1] + 1
            } else {
                dp[(i + 1) * stride + j].max(dp[i * stride + j + 1])
            };
        }
    }
    let mut ops = Vec::with_capacity(n + m);
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if left[i] == right[j] {
            ops.push(DiffOp::Equal(i, j));
            i += 1;
            j += 1;
        } else if dp[(i + 1) * stride + j] >= dp[i * stride + j + 1] {
            ops.push(DiffOp::Delete(i));
            i += 1;
        } else {
            ops.push(DiffOp::Insert(j));
            j += 1;
        }
    }
    while i < n {
        ops.push(DiffOp::Delete(i));
        i += 1;
    }
    while j < m {
        ops.push(DiffOp::Insert(j));
        j += 1;
    }
    ops
}

pub(super) fn tokenize_words(bytes: &[u8]) -> Vec<String> {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return bytes
            .chunks(16)
            .take(MAX_WORD_UNITS)
            .map(editing::hex_encode)
            .collect();
    };
    let mut tokens = Vec::new();
    let mut chars = text.char_indices().peekable();
    while let Some(&(start, ch)) = chars.peek() {
        if tokens.len() >= MAX_WORD_UNITS {
            break;
        }
        chars.next();
        let mut end = start + ch.len_utf8();
        if ch.is_alphanumeric() || matches!(ch, '_' | '/' | '.' | '-' | ':') {
            while let Some(&(next_idx, next_ch)) = chars.peek() {
                if next_ch.is_alphanumeric() || matches!(next_ch, '_' | '/' | '.' | '-' | ':') {
                    end = next_idx + next_ch.len_utf8();
                    chars.next();
                } else {
                    break;
                }
            }
        } else if ch == '\r' {
            if let Some(&(next_idx, '\n')) = chars.peek() {
                end = next_idx + 1;
                chars.next();
            }
        } else if ch == '\n' {
            // Single newline token so line structure stays aligned.
        } else if ch.is_whitespace() {
            while let Some(&(next_idx, next_ch)) = chars.peek() {
                if next_ch.is_whitespace() && next_ch != '\r' && next_ch != '\n' {
                    end = next_idx + next_ch.len_utf8();
                    chars.next();
                } else {
                    break;
                }
            }
        }
        tokens.push(text[start..end].to_owned());
    }
    tokens
}

fn is_pure_whitespace(token: &str) -> bool {
    !token.is_empty() && token.chars().all(char::is_whitespace)
}

fn display_word_token(token: &str) -> String {
    if is_pure_whitespace(token) {
        token.escape_default().to_string()
    } else {
        token.to_owned()
    }
}

pub(super) type SideBlock = (Option<std::ops::Range<usize>>, Option<std::ops::Range<usize>>);

fn count_and_format_diff<T: PartialEq>(
    header: &'static str,
    left: &[T],
    right: &[T],
    mut is_ws: impl FnMut(&T) -> bool,
    mut display: impl FnMut(&T) -> String,
) -> (usize, usize, usize, String, Vec<std::ops::Range<usize>>) {
    let ops = lcs_ops(left, right);
    let mut modified = 0;
    let mut added = 0;
    let mut deleted = 0;
    let mut details = String::new();
    let mut detail_ranges = Vec::new();
    let mut idx = 0;
    while idx < ops.len() {
        if matches!(ops[idx], DiffOp::Equal(_, _)) {
            idx += 1;
            continue;
        }
        let mut deletes = Vec::new();
        let mut inserts = Vec::new();
        while idx < ops.len() {
            match ops[idx] {
                DiffOp::Delete(i) => deletes.push(i),
                DiffOp::Insert(j) => inserts.push(j),
                DiffOp::Equal(_, _) => break,
            }
            idx += 1;
        }
        let has_non_ws =
            deletes.iter().any(|&i| !is_ws(&left[i])) || inserts.iter().any(|&j| !is_ws(&right[j]));
        if has_non_ws {
            deletes.retain(|&i| !is_ws(&left[i]));
            inserts.retain(|&j| !is_ws(&right[j]));
        }
        let block_start = details.len();
        let paired = deletes.len().min(inserts.len());
        for k in 0..paired {
            modified += 1;
            let li = deletes[k];
            let rj = inserts[k];
            let _ = writeln!(
                details,
                "Modified {:08X}/{:08X}\n- {}\n+ {}",
                li,
                rj,
                display(&left[li]),
                display(&right[rj])
            );
        }
        for &li in &deletes[paired..] {
            deleted += 1;
            let _ = writeln!(details, "- {:08X}  {}", li, display(&left[li]));
        }
        for &rj in &inserts[paired..] {
            added += 1;
            let _ = writeln!(details, "+ {:08X}  {}", rj, display(&right[rj]));
        }
        if details.len() > block_start {
            let end = details.strip_suffix('\n').map_or(details.len(), str::len);
            detail_ranges.push(block_start..end);
        }
    }
    let prefix = format!("{header}\nModified: {modified}  Added: {added}  Deleted: {deleted}\n");
    let offset = prefix.len();
    let blocks = detail_ranges.into_iter().map(|r| (r.start + offset)..(r.end + offset)).collect();
    (modified, added, deleted, format!("{prefix}{details}"), blocks)
}

pub(super) fn compare_items(left: &[u8], right: &[u8], byte_mode: bool) -> (String, Vec<std::ops::Range<usize>>) {
    if byte_mode {
        let left = &left[..left.len().min(MAX_BYTE_UNITS)];
        let right = &right[..right.len().min(MAX_BYTE_UNITS)];
        let (_, _, _, text, blocks) = count_and_format_diff("Hex bytes", left, right, |_| false, |b| format!("{b:02X}"));
        (text, blocks)
    } else {
        let left_units = tokenize_words(left);
        let right_units = tokenize_words(right);
        let (_, _, _, text, blocks) = count_and_format_diff(
            "Words",
            &left_units,
            &right_units,
            |u| is_pure_whitespace(u.as_str()),
            |u| display_word_token(u.as_str()),
        );
        (text, blocks)
    }
}

pub(super) fn compare_side_by_side(
    left_label: &str,
    left: &[u8],
    right_label: &str,
    right: &[u8],
    byte_mode: bool,
) -> (String, String, Vec<SideBlock>) {
    if byte_mode {
        let l = &left[..left.len().min(MAX_BYTE_UNITS)];
        let r = &right[..right.len().min(MAX_BYTE_UNITS)];
        let (modified, added, deleted, _, _) =
            count_and_format_diff("Hex bytes", l, r, |_| false, |b| format!("{b:02X}"));
        let ops = lcs_ops(l, r);
        let mut del_mask = vec![false; l.len()];
        let mut ins_mask = vec![false; r.len()];
        let mut raw_blocks = Vec::new();
        let mut idx = 0;
        while idx < ops.len() {
            if matches!(ops[idx], DiffOp::Equal(_, _)) {
                idx += 1;
                continue;
            }
            let (mut l_first, mut r_first) = (None, None);
            while idx < ops.len() {
                match ops[idx] {
                    DiffOp::Delete(i) => { del_mask[i] = true; l_first.get_or_insert(i); }
                    DiffOp::Insert(j) => { ins_mask[j] = true; r_first.get_or_insert(j); }
                    DiffOp::Equal(_, _) => break,
                }
                idx += 1;
            }
            raw_blocks.push((l_first, r_first));
        }
        let render_hex = |bytes: &[u8], mask: &[bool]| -> (String, Vec<std::ops::Range<usize>>) {
            let mut out = String::new();
            let mut spans = Vec::with_capacity(bytes.len());
            for (row_idx, chunk) in bytes.chunks(16).enumerate() {
                let base = row_idx * 16;
                let _ = write!(out, "{base:08X}  ");
                for (offset, &byte) in chunk.iter().enumerate() {
                    if offset > 0 {
                        out.push(' ');
                    }
                    let start = out.len();
                    if mask[base + offset] {
                        let _ = write!(out, "[{byte:02X}]");
                    } else {
                        let _ = write!(out, "{byte:02X}");
                    }
                    spans.push(start..out.len());
                }
                out.push('\n');
            }
            (out, spans)
        };
        let left_prefix = format!("#A {left_label}\nModified: {modified}  Deleted: {deleted}\n");
        let right_prefix = format!("#B {right_label}\nModified: {modified}  Added: {added}\n");
        let (left_hex, left_spans) = render_hex(l, &del_mask);
        let (right_hex, right_spans) = render_hex(r, &ins_mask);
        let blocks = raw_blocks.into_iter().map(|(li, rj)| {
            (
                li.and_then(|i| left_spans.get(i)).map(|s| (s.start + left_prefix.len())..(s.end + left_prefix.len())),
                rj.and_then(|j| right_spans.get(j)).map(|s| (s.start + right_prefix.len())..(s.end + right_prefix.len())),
            )
        }).collect();
        (format!("{left_prefix}{left_hex}"), format!("{right_prefix}{right_hex}"), blocks)
    } else {
        let left_units = tokenize_words(left);
        let right_units = tokenize_words(right);
        let (modified, added, deleted, _, _) = count_and_format_diff(
            "Words",
            &left_units,
            &right_units,
            |u| is_pure_whitespace(u.as_str()),
            |u| display_word_token(u.as_str()),
        );
        let ops = lcs_ops(&left_units, &right_units);
        let mut left_body = String::new();
        let mut right_body = String::new();
        let mut body_blocks = Vec::new();
        let mut idx = 0;
        while idx < ops.len() {
            if let DiffOp::Equal(i, j) = ops[idx] {
                left_body.push_str(&left_units[i]);
                right_body.push_str(&right_units[j]);
                idx += 1;
                continue;
            }
            let (mut l_span, mut r_span) = (None::<std::ops::Range<usize>>, None::<std::ops::Range<usize>>);
            while idx < ops.len() {
                match ops[idx] {
                    DiffOp::Delete(i) => {
                        let tok = &left_units[i];
                        if is_pure_whitespace(tok) {
                            left_body.push_str(tok);
                        } else {
                            let start = left_body.len();
                            let _ = write!(left_body, "[-{tok}]");
                            let end = left_body.len();
                            l_span = Some(l_span.map_or(start..end, |r| r.start..end));
                        }
                    }
                    DiffOp::Insert(j) => {
                        let tok = &right_units[j];
                        if is_pure_whitespace(tok) {
                            right_body.push_str(tok);
                        } else {
                            let start = right_body.len();
                            let _ = write!(right_body, "[+{tok}]");
                            let end = right_body.len();
                            r_span = Some(r_span.map_or(start..end, |r| r.start..end));
                        }
                    }
                    DiffOp::Equal(_, _) => break,
                }
                idx += 1;
            }
            if l_span.is_some() || r_span.is_some() {
                body_blocks.push((l_span, r_span));
            }
        }
        let left_prefix = format!("#A {left_label}\nModified: {modified}  Deleted: {deleted}\n---\n");
        let right_prefix = format!("#B {right_label}\nModified: {modified}  Added: {added}\n---\n");
        let blocks = body_blocks.into_iter().map(|(l, r)| {
            (
                l.map(|s| (s.start + left_prefix.len())..(s.end + left_prefix.len())),
                r.map(|s| (s.start + right_prefix.len())..(s.end + right_prefix.len())),
            )
        }).collect();
        (format!("{left_prefix}{left_body}"), format!("{right_prefix}{right_body}"), blocks)
    }
}
