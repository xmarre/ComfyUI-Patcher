use chrono::Utc;
use regex::Regex;
use std::path::{Path, PathBuf};
use uuid::Uuid;

pub fn now_rfc3339() -> String {
    Utc::now().to_rfc3339()
}

pub fn new_id() -> String {
    Uuid::new_v4().to_string()
}

pub fn slugify(name: &str) -> String {
    let lower = name.trim().to_lowercase();
    let re = Regex::new(r"[^a-z0-9._-]+").unwrap();
    let compact = re.replace_all(&lower, "-");
    compact.trim_matches('-').to_string()
}

pub fn detect_env_kind(python_exe: &Path) -> String {
    let lower = python_exe.to_string_lossy().to_lowercase();
    if lower.contains("conda") || lower.contains("miniconda") {
        "conda".to_string()
    } else if lower.contains("venv") || lower.contains(".venv") {
        "venv".to_string()
    } else if lower.ends_with("python") || lower.ends_with("python.exe") {
        "system".to_string()
    } else {
        "unknown".to_string()
    }
}

pub fn infer_python(root: &Path) -> Option<PathBuf> {
    let candidates = [
        root.join("venv/bin/python"),
        root.join(".venv/bin/python"),
        root.join("python_embeded/python.exe"),
        root.join("python_embedded/python.exe"),
        root.join("venv/Scripts/python.exe"),
        root.join(".venv/Scripts/python.exe"),
    ];
    for path in candidates {
        if path.exists() {
            return Some(path);
        }
    }
    None
}
