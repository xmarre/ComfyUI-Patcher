use crate::errors::{AppError, AppResult};
use crate::models::DirtyRepoStrategy;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tokio::process::Command;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoStatus {
    pub head_sha: Option<String>,
    pub branch: Option<String>,
    pub is_detached: bool,
    pub is_dirty: bool,
    pub origin_url: Option<String>,
}

pub async fn run_git(path: &Path, args: &[&str]) -> AppResult<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(path)
        .output()
        .await?;
    if !output.status.success() {
        return Err(AppError::Git(format!(
            "{}\n{}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

pub async fn run_git_allow_fail(path: &Path, args: &[&str]) -> AppResult<Option<String>> {
    let output = Command::new("git")
        .args(args)
        .current_dir(path)
        .output()
        .await?;
    if !output.status.success() {
        return Ok(None);
    }
    Ok(Some(String::from_utf8_lossy(&output.stdout).trim().to_string()))
}

pub async fn inspect_repo(path: &Path) -> AppResult<RepoStatus> {
    let head_sha = run_git_allow_fail(path, &["rev-parse", "HEAD"]).await?;
    let branch = run_git_allow_fail(path, &["symbolic-ref", "--short", "-q", "HEAD"]).await?;
    let status = run_git(path, &["status", "--porcelain"]).await?;
    let origin_url = run_git_allow_fail(path, &["remote", "get-url", "origin"]).await?;
    Ok(RepoStatus {
        head_sha,
        branch: branch.clone(),
        is_detached: branch.is_none(),
        is_dirty: !status.trim().is_empty(),
        origin_url: origin_url.and_then(|value| canonicalize_remote(&value)),
    })
}

pub fn canonicalize_remote(input: &str) -> Option<String> {
    let input = input.trim();
    if input.is_empty() {
        return None;
    }

    if let Some(caps) = Regex::new(r"^git@github\.com:(?P<owner>[^/]+)/(?P<repo>[^/]+?)(?:\.git)?$")
        .unwrap()
        .captures(input)
    {
        let owner = caps.name("owner")?.as_str();
        let repo = caps.name("repo")?.as_str();
        return Some(format!("https://github.com/{owner}/{repo}"));
    }

    let parsed = url::Url::parse(input).ok()?;
    if parsed.host_str()?.eq_ignore_ascii_case("github.com") {
        let segments: Vec<_> = parsed
            .path_segments()?
            .filter(|segment| !segment.is_empty())
            .collect();
        if segments.len() >= 2 {
            let owner = segments[0];
            let repo = segments[1].trim_end_matches(".git");
            return Some(format!("https://github.com/{owner}/{repo}"));
        }
    }
    None
}

pub async fn ensure_clean_or_apply_strategy(path: &Path, strategy: &DirtyRepoStrategy) -> AppResult<Option<String>> {
    let status = inspect_repo(path).await?;
    if !status.is_dirty {
        return Ok(None);
    }
    match strategy {
        DirtyRepoStrategy::Abort => Err(AppError::Conflict("repository has local changes".to_string())),
        DirtyRepoStrategy::HardReset => {
            run_git(path, &["reset", "--hard"]).await?;
            run_git(path, &["clean", "-fd"]).await?;
            Ok(None)
        }
        DirtyRepoStrategy::Stash => {
            let out = run_git(path, &["stash", "push", "-u", "-m", "comfyui-patcher-auto-stash"]).await?;
            if out.contains("No local changes") {
                Ok(None)
            } else {
                Ok(Some("stash@{0}".to_string()))
            }
        }
    }
}

pub async fn clone_repo(url: &str, dest: &Path) -> AppResult<()> {
    let parent = dest.parent().ok_or_else(|| AppError::InvalidInput("destination has no parent".to_string()))?;
    std::fs::create_dir_all(parent)?;
    let output = Command::new("git")
        .args(["clone", url, &dest.to_string_lossy()])
        .output()
        .await?;
    if !output.status.success() {
        return Err(AppError::Git(String::from_utf8_lossy(&output.stderr).to_string()));
    }
    Ok(())
}

pub async fn fetch_origin(path: &Path) -> AppResult<()> {
    run_git(path, &["fetch", "--prune", "--tags", "origin"]).await?;
    Ok(())
}

pub async fn fetch_refspec(path: &Path, remote: &str, refspec: &str) -> AppResult<()> {
    run_git(path, &["fetch", remote, refspec]).await?;
    Ok(())
}

pub async fn switch_branch(path: &Path, branch: &str, start_point: Option<&str>) -> AppResult<()> {
    match start_point {
        Some(start) => run_git(path, &["switch", "-C", branch, start]).await?,
        None => run_git(path, &["switch", branch]).await?,
    };
    Ok(())
}

pub async fn switch_detached(path: &Path, target: &str) -> AppResult<()> {
    run_git(path, &["switch", "--detach", target]).await?;
    Ok(())
}

pub async fn reset_hard(path: &Path, target: &str) -> AppResult<()> {
    run_git(path, &["reset", "--hard", target]).await?;
    Ok(())
}

pub async fn submodule_update(path: &Path) -> AppResult<()> {
    run_git(path, &["submodule", "update", "--init", "--recursive"]).await?;
    Ok(())
}

pub async fn ls_remote_head(path: &Path, name: &str) -> AppResult<bool> {
    let output = Command::new("git")
        .args(["ls-remote", "--heads", "origin", name])
        .current_dir(path)
        .output()
        .await?;
    Ok(output.status.success() && !String::from_utf8_lossy(&output.stdout).trim().is_empty())
}

pub async fn ls_remote_tag(path: &Path, name: &str) -> AppResult<bool> {
    let output = Command::new("git")
        .args(["ls-remote", "--tags", "origin", name])
        .current_dir(path)
        .output()
        .await?;
    Ok(output.status.success() && !String::from_utf8_lossy(&output.stdout).trim().is_empty())
}

pub async fn apply_stash(path: &Path) -> AppResult<()> {
    let _ = run_git(path, &["stash", "pop"]).await?;
    Ok(())
}

pub async fn is_git_repo(path: &Path) -> bool {
    run_git_allow_fail(path, &["rev-parse", "--is-inside-work-tree"])
        .await
        .ok()
        .flatten()
        .map(|value| value == "true")
        .unwrap_or(false)
}

pub fn join_custom_node_path(custom_nodes_dir: &Path, dir_name: &str) -> PathBuf {
    custom_nodes_dir.join(dir_name)
}
