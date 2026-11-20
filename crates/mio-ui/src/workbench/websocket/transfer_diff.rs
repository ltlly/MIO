use std::fmt::Write;

use super::*;

const MAX_WORD_UNITS: usize = 2048;
const MAX_BYTE_UNITS: usize = 4096;

pub(super) fn render_simple_diff(left: &[u8], right: &[u8], byte_mode: bool) -> String {
    if byte_mode {
        let l = &left[..left.len().min(MAX_BYTE_UNITS)];
        let r = &right[..right.len().min(MAX_BYTE_UNITS)];
        diff_slices("Hex bytes", l, r, |b| format!("{b:02X}"))
    } else {
        let split = |bytes: &[u8]| -> Vec<String> {
            match std::str::from_utf8(bytes) {
                Ok(s) => s
                    .split_inclusive('\n')
                    .take(MAX_WORD_UNITS)
                    .map(str::to_owned)
                    .collect(),
                Err(_) => bytes
                    .chunks(16)
                    .take(MAX_WORD_UNITS)
                    .map(editing::hex_encode)
                    .collect(),
            }
        };
        let lu = split(left);
        let ru = split(right);
        diff_slices("Words / lines", &lu, &ru, |u| {
            u.trim_end_matches('\n').to_owned()
        })
    }
}

fn diff_slices<T: PartialEq>(
    header: &'static str,
    left: &[T],
    right: &[T],
    mut display: impl FnMut(&T) -> String,
) -> String {
    let (n, m) = (left.len(), right.len());
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
    let (mut i, mut j) = (0, 0);
    let (mut modified, mut added, mut deleted) = (0usize, 0usize, 0usize);
    let mut details = String::new();
    while i < n || j < m {
        if i < n && j < m && left[i] == right[j] {
            i += 1;
            j += 1;
            continue;
        }
        let (mut dels, mut ins) = (Vec::new(), Vec::new());
        while i < n || j < m {
            if i < n && j < m && left[i] == right[j] {
                break;
            }
            if i < n && (j == m || dp[(i + 1) * stride + j] >= dp[i * stride + j + 1]) {
                dels.push(i);
                i += 1;
            } else {
                ins.push(j);
                j += 1;
            }
        }
        let paired = dels.len().min(ins.len());
        for k in 0..paired {
            modified += 1;
            let _ = writeln!(
                details,
                "Modified {:08X}/{:08X}\n- {}\n+ {}",
                dels[k],
                ins[k],
                display(&left[dels[k]]),
                display(&right[ins[k]])
            );
        }
        for &li in &dels[paired..] {
            deleted += 1;
            let _ = writeln!(details, "- {:08X}  {}", li, display(&left[li]));
        }
        for &rj in &ins[paired..] {
            added += 1;
            let _ = writeln!(details, "+ {:08X}  {}", rj, display(&right[rj]));
        }
    }
    format!("{header}\nModified: {modified}  Added: {added}  Deleted: {deleted}\n{details}")
}
