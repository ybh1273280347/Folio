//! Discovery and deletion of pi session JSONL files for the active workspace.
use serde_json::Value;
use std::{
    fs::{self, File},
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    time::SystemTime,
};

#[derive(Clone, Debug)]
pub struct SessionSummary {
    pub path: PathBuf,
    pub id: String,
    pub name: Option<String>,
    pub preview: String,
    pub message_count: usize,
    pub modified: SystemTime,
    pub timestamp_label: String,
}

impl SessionSummary {
    pub fn display_title(&self) -> &str {
        if let Some(name) = self.name.as_deref()
            && !name.trim().is_empty()
        {
            return name;
        }
        if !self.preview.trim().is_empty() {
            return &self.preview;
        }
        "未命名会话"
    }
}

pub fn sessions_root() -> Option<PathBuf> {
    if let Some(agent_dir) = std::env::var_os("PI_CODING_AGENT_DIR") {
        return Some(PathBuf::from(agent_dir).join("sessions"));
    }
    dirs::home_dir().map(|h| h.join(".pi").join("agent").join("sessions"))
}

pub fn list_sessions_for_cwd(cwd: &Path) -> Vec<SessionSummary> {
    let Some(root) = sessions_root() else {
        return Vec::new();
    };
    if !root.is_dir() {
        return Vec::new();
    }

    let target_norm = normalize_cwd(cwd);
    let mut results = Vec::new();

    let Ok(dirs) = fs::read_dir(&root) else {
        return Vec::new();
    };
    for dir_entry in dirs.flatten() {
        let dir_path = dir_entry.path();
        if !dir_path.is_dir() {
            continue;
        }
        let Ok(files) = fs::read_dir(&dir_path) else {
            continue;
        };
        for file_entry in files.flatten() {
            let path = file_entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            if let Some(summary) = parse_session_file(&path, &target_norm) {
                results.push(summary);
            }
        }
    }

    results.sort_by(|a, b| b.modified.cmp(&a.modified));
    results
}

pub fn delete_session_file(path: &Path) -> Result<(), String> {
    if path.is_file() {
        fs::remove_file(path).map_err(|e| format!("删除会话失败：{e}"))?;
    }
    Ok(())
}

fn normalize_cwd(path: &Path) -> String {
    let s = dunce::canonicalize(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .replace('/', "\\");
    s.trim_end_matches('\\').to_ascii_lowercase()
}

fn parse_session_file(path: &Path, target_cwd_norm: &str) -> Option<SessionSummary> {
    let file = File::open(path).ok()?;
    let modified = file
        .metadata()
        .and_then(|m| m.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH);
    let mut reader = BufReader::new(file);
    let mut first_line = String::new();
    if reader.read_line(&mut first_line).ok()? == 0 {
        return None;
    }
    let header: Value = serde_json::from_str(&first_line).ok()?;
    if header.get("type").and_then(|v| v.as_str()) != Some("session") {
        return None;
    }
    let session_cwd = header.get("cwd").and_then(|v| v.as_str())?;
    if normalize_cwd(Path::new(session_cwd)) != target_cwd_norm {
        return None;
    }

    let id = header
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_owned();
    let ts_raw = header
        .get("timestamp")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let timestamp_label = format_iso_short(ts_raw);

    let mut name: Option<String> = None;
    let mut preview = String::new();
    let mut message_count = 0usize;

    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                let Ok(entry) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                match entry.get("type").and_then(|v| v.as_str()) {
                    Some("session_info") => {
                        if let Some(n) = entry.get("name").and_then(|v| v.as_str()) {
                            let trimmed = n.trim();
                            name = if trimmed.is_empty() {
                                None
                            } else {
                                Some(trimmed.to_owned())
                            };
                        }
                    }
                    Some("message") => {
                        message_count += 1;
                        if preview.is_empty()
                            && let Some(msg) = entry.get("message")
                            && msg.get("role").and_then(|v| v.as_str()) == Some("user")
                        {
                            preview = extract_user_preview(msg);
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    Some(SessionSummary {
        path: path.to_path_buf(),
        id,
        name,
        preview,
        message_count,
        modified,
        timestamp_label,
    })
}

fn extract_user_preview(msg: &Value) -> String {
    let raw = match msg.get("content") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(arr)) => arr
            .iter()
            .filter_map(|b| {
                if b.get("type").and_then(|v| v.as_str()) == Some("text") {
                    b.get("text").and_then(|v| v.as_str()).map(str::to_owned)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join(" "),
        _ => String::new(),
    };
    let clean = raw
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('@'))
        .collect::<Vec<_>>()
        .join(" ");
    clean.chars().take(48).collect()
}

fn format_iso_short(iso: &str) -> String {
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(iso) {
        let local: chrono::DateTime<chrono::Local> = dt.into();
        return local.format("%m-%d %H:%M").to_string();
    }
    iso.chars().take(16).collect()
}
