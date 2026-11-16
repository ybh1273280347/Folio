//! Pure-Rust line diff computation for native GPUI diff cards.

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiffLine {
    Context(String),
    Removed(String),
    Added(String),
    Ellipsis,
}

#[derive(Clone, Debug)]
pub struct FileDiff {
    pub path: String,
    pub added: usize,
    pub removed: usize,
    pub lines: Vec<DiffLine>,
}

pub fn compute_diff(path: String, old_text: &str, new_text: &str) -> FileDiff {
    let old_lines: Vec<&str> = old_text.lines().collect();
    let new_lines: Vec<&str> = new_text.lines().collect();

    // Guard against pathological O(N*M) DP on huge files.
    let raw = if old_lines.len().saturating_mul(new_lines.len()) > 200_000 {
        let mut out = Vec::with_capacity(old_lines.len().min(60) + new_lines.len().min(60) + 1);
        for line in old_lines.iter().take(60) {
            out.push(DiffLine::Removed((*line).to_owned()));
        }
        for line in new_lines.iter().take(60) {
            out.push(DiffLine::Added((*line).to_owned()));
        }
        if old_lines.len() > 60 || new_lines.len() > 60 {
            out.push(DiffLine::Ellipsis);
        }
        out
    } else {
        lcs_diff(&old_lines, &new_lines)
    };

    let mut added = 0usize;
    let mut removed = 0usize;
    for line in &raw {
        match line {
            DiffLine::Added(_) => added += 1,
            DiffLine::Removed(_) => removed += 1,
            _ => {}
        }
    }

    let lines = collapse_context(raw, 2);
    FileDiff {
        path,
        added,
        removed,
        lines,
    }
}

fn lcs_diff(old: &[&str], new: &[&str]) -> Vec<DiffLine> {
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
            out.push(DiffLine::Context(old[i].to_owned()));
            i += 1;
            j += 1;
        } else if dp[i + 1][j] >= dp[i][j + 1] {
            out.push(DiffLine::Removed(old[i].to_owned()));
            i += 1;
        } else {
            out.push(DiffLine::Added(new[j].to_owned()));
            j += 1;
        }
    }
    while i < m {
        out.push(DiffLine::Removed(old[i].to_owned()));
        i += 1;
    }
    while j < n {
        out.push(DiffLine::Added(new[j].to_owned()));
        j += 1;
    }
    out
}

fn collapse_context(raw: Vec<DiffLine>, radius: usize) -> Vec<DiffLine> {
    let changed: Vec<bool> = raw
        .iter()
        .map(|l| !matches!(l, DiffLine::Context(_)))
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
