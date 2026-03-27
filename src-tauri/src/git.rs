use crate::errors::{AppError, AppResult};
use crate::execution::{output_command, parse_wsl_unc_path};
use crate::models::DirtyRepoStrategy;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoStatus {
    pub head_sha: Option<String>,
    pub branch: Option<String>,
    pub is_detached: bool,
    pub is_dirty: bool,
    pub origin_url: Option<String>,
}

pub async fn run_git(path: &Path, args: &[&str]) -> AppResult<String> {
    let args_vec: Vec<String> = args.iter().map(|value| (*value).to_string()).collect();
    let output = output_command("git", &args_vec, Some(path)).await?;
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
    let args_vec: Vec<String> = args.iter().map(|value| (*value).to_string()).collect();
    let output = output_command("git", &args_vec, Some(path)).await?;
    if !output.status.success() {
        return Ok(None);
    }
    Ok(Some(
        String::from_utf8_lossy(&output.stdout).trim().to_string(),
    ))
}

async fn run_git_no_cwd(args: &[&str]) -> AppResult<String> {
    let args_vec: Vec<String> = args.iter().map(|value| (*value).to_string()).collect();
    let output = output_command("git", &args_vec, None).await?;
    if !output.status.success() {
        return Err(AppError::Git(format!(
            "{}\n{}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
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
        let owner = caps.name("owner")?.as_str().to_ascii_lowercase();
        let repo = caps.name("repo")?.as_str().to_ascii_lowercase();
        return Some(format!("https://github.com/{owner}/{repo}"));
    }

    let parsed = url::Url::parse(input).ok()?;
    if parsed.host_str()?.eq_ignore_ascii_case("github.com") {
        let segments: Vec<_> = parsed
            .path_segments()?
            .filter(|segment| !segment.is_empty())
            .collect();
        if segments.len() >= 2 {
            let owner = segments[0].to_ascii_lowercase();
            let repo = segments[1].trim_end_matches(".git").to_ascii_lowercase();
            return Some(format!("https://github.com/{owner}/{repo}"));
        }
    }
    None
}

pub async fn ensure_clean_or_apply_strategy(
    path: &Path,
    strategy: &DirtyRepoStrategy,
) -> AppResult<Option<String>> {
    let status = inspect_repo(path).await?;
    if !status.is_dirty {
        return Ok(None);
    }
    match strategy {
        DirtyRepoStrategy::Abort => Err(AppError::Conflict(
            "repository has local changes".to_string(),
        )),
        DirtyRepoStrategy::HardReset => {
            run_git(path, &["reset", "--hard"]).await?;
            run_git(path, &["clean", "-fd"]).await?;
            Ok(None)
        }
        DirtyRepoStrategy::Stash => {
            let out = run_git(
                path,
                &["stash", "push", "-u", "-m", "comfyui-patcher-auto-stash"],
            )
            .await?;
            if out.contains("No local changes") {
                Ok(None)
            } else {
                let stash_id = run_git(path, &["rev-parse", "--verify", "stash@{0}"]).await?;
                if stash_id.is_empty() {
                    Err(AppError::Git(
                        "git stash push succeeded but no stash entry could be resolved".to_string(),
                    ))
                } else {
                    Ok(Some(stash_id))
                }
            }
        }
    }
}

pub async fn clone_repo(url: &str, dest: &Path) -> AppResult<()> {
    let parent = dest
        .parent()
        .ok_or_else(|| AppError::InvalidInput("destination has no parent".to_string()))?;
    std::fs::create_dir_all(parent)?;
    let dir_name = dest
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| AppError::InvalidInput("destination has no final directory name".to_string()))?;
    let args = vec!["clone".to_string(), url.to_string(), dir_name.to_string()];
    let output = output_command("git", &args, Some(parent)).await?;
    if !output.status.success() {
        return Err(AppError::Git(
            String::from_utf8_lossy(&output.stderr).to_string(),
        ));
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
    let output = run_git(path, &["ls-remote", "--heads", "origin", name]).await?;
    Ok(!output.trim().is_empty())
}

pub async fn ls_remote_tag(path: &Path, name: &str) -> AppResult<bool> {
    let output = run_git(path, &["ls-remote", "--tags", "origin", name]).await?;
    Ok(!output.trim().is_empty())
}

pub async fn ls_remote_head_remote(remote: &str, name: &str) -> AppResult<bool> {
    let output = run_git_no_cwd(&["ls-remote", "--heads", remote, name]).await?;
    Ok(!output.trim().is_empty())
}

pub async fn ls_remote_tag_remote(remote: &str, name: &str) -> AppResult<bool> {
    let output = run_git_no_cwd(&["ls-remote", "--tags", remote, name]).await?;
    Ok(!output.trim().is_empty())
}

fn stash_ref_from_list(stash_list: &str, stash_id: &str) -> Option<String> {
    if stash_id.starts_with("stash@{") {
        return Some(stash_id.to_string());
    }

    stash_list.lines().find_map(|line| {
        let mut parts = line.splitn(2, '\t');
        let sha = parts.next()?.trim();
        let stash_ref = parts.next()?.trim();
        (sha == stash_id && !stash_ref.is_empty()).then(|| stash_ref.to_string())
    })
}

pub async fn apply_stash(path: &Path, stash_id: &str) -> AppResult<()> {
    let stash_ref = if stash_id.starts_with("stash@{") {
        stash_id.to_string()
    } else {
        let stash_list = run_git(path, &["stash", "list", "--format=%H%x09%gd"]).await?;
        stash_ref_from_list(&stash_list, stash_id).ok_or_else(|| {
            AppError::Git(format!(
                "could not find saved stash entry for checkpoint stash {stash_id}"
            ))
        })?
    };
    let _ = run_git(path, &["stash", "pop", &stash_ref]).await?;
    Ok(())
}

fn normalize_linux_path(input: &str) -> String {
    let normalized = input.replace('\\', "/");
    let trimmed = normalized.trim_end_matches('/');
    if trimmed.is_empty() {
        "/".to_string()
    } else {
        trimmed.to_string()
    }
}

fn canonicalize_wsl_linux_path(path: &Path) -> Option<(String, String)> {
    let parsed = parse_wsl_unc_path(path)?;
    let canonical_linux_path = std::fs::canonicalize(path)
        .ok()
        .as_deref()
        .and_then(parse_wsl_unc_path)
        .map(|value| normalize_linux_path(&value.linux_path))
        .unwrap_or_else(|| normalize_linux_path(&parsed.linux_path));
    Some((parsed.distro, canonical_linux_path))
}

fn canonicalize_wsl_repo_root_for_distro(distro: &str, repo_root: &str) -> String {
    let normalized_repo_root = normalize_linux_path(repo_root);
    let unc_path = if normalized_repo_root == "/" {
        format!(r"\\wsl.localhost\{distro}")
    } else {
        format!(
            r"\\wsl.localhost\{distro}\{}",
            normalized_repo_root
                .trim_start_matches('/')
                .replace('/', "\\")
        )
    };

    std::fs::canonicalize(Path::new(&unc_path))
        .ok()
        .as_deref()
        .and_then(parse_wsl_unc_path)
        .map(|value| normalize_linux_path(&value.linux_path))
        .unwrap_or(normalized_repo_root)
}

#[cfg(windows)]
fn normalize_native_path(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_ascii_lowercase()
}

#[cfg(not(windows))]
fn normalize_native_path(path: &Path) -> String {
    path.to_string_lossy().trim_end_matches('/').to_string()
}

pub async fn is_git_repo(path: &Path) -> bool {
    let Some(repo_root) = run_git_allow_fail(path, &["rev-parse", "--show-toplevel"])
        .await
        .ok()
        .flatten()
    else {
        return false;
    };

    if let Some((distro, canonical_linux_path)) = canonicalize_wsl_linux_path(path) {
        return canonical_linux_path == canonicalize_wsl_repo_root_for_distro(&distro, &repo_root);
    }

    let canonical_path = match std::fs::canonicalize(path) {
        Ok(path) => path,
        Err(_) => return false,
    };
    let repo_root_path = Path::new(&repo_root);
    if !repo_root_path.is_absolute() {
        return false;
    }
    let canonical_repo_root = match std::fs::canonicalize(repo_root_path) {
        Ok(path) => path,
        Err(_) => return false,
    };

    normalize_native_path(&canonical_path) == normalize_native_path(&canonical_repo_root)
}

pub fn validate_custom_node_dir_name(dir_name: &str) -> AppResult<String> {
    let trimmed = dir_name.trim();
    if trimmed.is_empty() {
        return Err(AppError::InvalidInput(
            "custom node directory name cannot be empty".to_string(),
        ));
    }

    let path = Path::new(trimmed);
    let mut components = path.components();
    match (components.next(), components.next()) {
        (Some(Component::Normal(name)), None) => Ok(name.to_string_lossy().into_owned()),
        _ => Err(AppError::InvalidInput(
            "custom node directory name must be a single folder name inside custom_nodes"
                .to_string(),
        )),
    }
}

pub fn join_custom_node_path(custom_nodes_dir: &Path, dir_name: &str) -> PathBuf {
    custom_nodes_dir.join(dir_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_single_directory_name() {
        assert_eq!(validate_custom_node_dir_name("foo-bar").unwrap(), "foo-bar");
        assert_eq!(validate_custom_node_dir_name(" foo ").unwrap(), "foo");
        assert_eq!(validate_custom_node_dir_name("foo/").unwrap(), "foo");
    }

    #[test]
    fn rejects_empty_directory_name() {
        assert!(matches!(
            validate_custom_node_dir_name("   "),
            Err(AppError::InvalidInput(_))
        ));
    }

    #[test]
    fn rejects_non_local_directory_name() {
        for invalid in ["../foo", "a/b", "."] {
            assert!(matches!(
                validate_custom_node_dir_name(invalid),
                Err(AppError::InvalidInput(_))
            ));
        }
    }

    #[cfg(windows)]
    #[test]
    fn rejects_windows_absolute_directory_name() {
        assert!(matches!(
            validate_custom_node_dir_name(r"C:\temp\foo"),
            Err(AppError::InvalidInput(_))
        ));
    }

    #[cfg(not(windows))]
    #[test]
    fn rejects_unix_absolute_directory_name() {
        assert!(matches!(
            validate_custom_node_dir_name("/tmp/foo"),
            Err(AppError::InvalidInput(_))
        ));
    }

    #[test]
    fn resolves_stash_ref_from_saved_sha() {
        let stash_list = "abc123\tstash@{0}\ndef456\tstash@{1}";
        assert_eq!(
            stash_ref_from_list(stash_list, "def456"),
            Some("stash@{1}".to_string())
        );
    }

    #[test]
    fn preserves_legacy_stash_refs() {
        assert_eq!(
            stash_ref_from_list("", "stash@{2}"),
            Some("stash@{2}".to_string())
        );
    }
}
