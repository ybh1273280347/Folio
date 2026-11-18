//! File diff summaries and previews for native GPUI diff cards.

use gpui_kit::component::diff::{DiffFile, DiffGutterMarker, DiffLinePosition, DiffSide};
use std::rc::Rc;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiffLine {
    Context(Option<usize>, String),
    Removed(Option<usize>, String),
    Added(Option<usize>, String),
    Ellipsis,
}

#[derive(Clone, Default)]
pub struct FileDiff {
    pub path: String,
    pub added: usize,
    pub removed: usize,
    pub lines: Vec<DiffLine>,
    pub patch: Option<EditPatch>,
}

#[derive(Clone)]
pub struct EditPatch {
    pub file: DiffFile,
    pub line_numbers: Rc<[DiffGutterMarker]>,
    pub line_number_digits: u32,
    pub hunk_count: usize,
}

impl std::fmt::Debug for FileDiff {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileDiff")
            .field("path", &self.path)
            .field("added", &self.added)
            .field("removed", &self.removed)
            .finish_non_exhaustive()
    }
}

impl FileDiff {
    /// Pi's result describes the actual file. Tool arguments alone contain only snippets.
    pub fn from_edit_patch(path: String, patch: &str) -> Option<Self> {
        let [file]: [DiffFile; 1] = DiffFile::parse(patch).ok()?.try_into().ok()?;
        let mut line_numbers = Vec::new();
        let mut largest_number = 1;
        let mut hunk_count = 0;
        let (mut old, mut new) = (0usize, 0usize);
        // Pi emits two file headers followed by unified diff hunks.
        for line in patch.lines().skip(2) {
            let (side, number) = match line.as_bytes().first() {
                Some(b'@') => {
                    let mut ranges = line.split_ascii_whitespace().skip(1);
                    old = ranges.next()?.strip_prefix('-')?.split(',').next()?.parse().ok()?;
                    new = ranges.next()?.strip_prefix('+')?.split(',').next()?.parse().ok()?;
                    hunk_count += 1;
                    continue;
                }
                Some(b'-') => {
                    let number = old;
                    old += 1;
                    (DiffSide::Original, number)
                }
                Some(b'+' | b' ') => {
                    // Context advances both versions but displays the modified line number.
                    old += usize::from(line.starts_with(' '));
                    let number = new;
                    new += 1;
                    (DiffSide::Modified, number)
                }
                _ => continue,
            };
            largest_number = largest_number.max(number);
            line_numbers.push(DiffGutterMarker::new(
                ("ai-diff-number", line_numbers.len()),
                DiffLinePosition::new(file.path().clone(), side, number),
            ));
        }
        Some(Self {
            path,
            added: file.additions(),
            removed: file.deletions(),
            lines: Vec::new(),
            patch: Some(EditPatch {
                file,
                line_numbers: line_numbers.into(),
                line_number_digits: largest_number.ilog10() + 1,
                hunk_count,
            }),
        })
    }
    /// Only whole-file inputs can supply absolute line numbers for a preview.
    pub fn append_preview(&mut self, old_text: &str, new_text: &str, numbered: bool) {
        let old_lines: Vec<&str> = old_text.lines().collect();
        let new_lines: Vec<&str> = new_text.lines().collect();

        // Guard against pathological O(N*M) DP on huge files.
        let raw = if old_lines.len().saturating_mul(new_lines.len()) > 200_000 {
            let mut out = Vec::with_capacity(old_lines.len().min(60) + new_lines.len().min(60) + 1);
            for (i, line) in old_lines.iter().take(60).enumerate() {
                out.push(DiffLine::Removed(numbered.then_some(i + 1), (*line).to_owned()));
            }
            for (i, line) in new_lines.iter().take(60).enumerate() {
                out.push(DiffLine::Added(numbered.then_some(i + 1), (*line).to_owned()));
            }
            if old_lines.len() > 60 || new_lines.len() > 60 {
                out.push(DiffLine::Ellipsis);
            }
            out
        } else {
            lcs_diff(&old_lines, &new_lines, numbered)
        };

        for line in &raw {
            match line {
                DiffLine::Added(..) => self.added += 1,
                DiffLine::Removed(..) => self.removed += 1,
                _ => {}
            }
        }

        for line in collapse_context(raw, 2) {
            if line != DiffLine::Ellipsis || self.lines.last() != Some(&DiffLine::Ellipsis) {
                self.lines.push(line);
            }
        }
    }
}

fn lcs_diff(old: &[&str], new: &[&str], numbered: bool) -> Vec<DiffLine> {
    let m = old.len();
    let n = new.len();
    let mut dp = vec![vec![0usize; n + 1]; m + 1];
    for i in (0..m).rev() {
        for j in (0..n).rev() {
            dp[i][j] = if old[i] == new[j] {
                dp[i + 1][j + 1] + 1
            } else {
                dp[i + 1][j].max(dp[i][j + 1])
            };
        }
    }

    let mut out = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < m && j < n {
        if old[i] == new[j] {
            out.push(DiffLine::Context(numbered.then_some(j + 1), old[i].to_owned()));
            i += 1;
            j += 1;
        } else if dp[i + 1][j] >= dp[i][j + 1] {
            out.push(DiffLine::Removed(numbered.then_some(i + 1), old[i].to_owned()));
            i += 1;
        } else {
            out.push(DiffLine::Added(numbered.then_some(j + 1), new[j].to_owned()));
            j += 1;
        }
    }
    while i < m {
        out.push(DiffLine::Removed(numbered.then_some(i + 1), old[i].to_owned()));
        i += 1;
    }
    while j < n {
        out.push(DiffLine::Added(numbered.then_some(j + 1), new[j].to_owned()));
        j += 1;
    }
    out
}

fn collapse_context(raw: Vec<DiffLine>, radius: usize) -> Vec<DiffLine> {
    let changed: Vec<bool> = raw
        .iter()
        .map(|l| !matches!(l, DiffLine::Context(..)))
        .collect();
    if !changed.iter().any(|&c| c) {
        return raw.into_iter().take(6).collect();
    }

    let len = raw.len();
    let mut keep = vec![false; len];
    for (idx, &is_changed) in changed.iter().enumerate() {
        if is_changed {
            let start = idx.saturating_sub(radius);
            let end = (idx + radius + 1).min(len);
            for k in &mut keep[start..end] {
                *k = true;
            }
        }
    }

    let mut out = Vec::new();
    let mut skipped = false;
    for (line, show) in raw.into_iter().zip(keep) {
        if show {
            if skipped {
                out.push(DiffLine::Ellipsis);
                skipped = false;
            }
            out.push(line);
        } else {
            skipped = true;
        }
    }
    if skipped {
        out.push(DiffLine::Ellipsis);
    }
    out
}
