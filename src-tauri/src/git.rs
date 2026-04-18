use crate::errors::{AppError, AppResult};
use crate::execution::{output_command, parse_wsl_unc_path};
use crate::models::{DirtyRepoStrategy, RepoActionPreviewCommit, RepoActionPreviewFile};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoStatus {
    pub head_sha: Option<String>,
    pub branch: Option<String>,
    pub is_detached: bool,
    pub is_dirty: bool,
    pub changed_files: Vec<String>,
    pub tracked_changed_files: Vec<String>,
    pub untracked_files: Vec<String>,
    pub origin_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StatusEntry {
    code: String,
    paths: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct StatusBranchMetadata {
    branch: Option<String>,
    is_detached: bool,
}

fn parse_status_entries(output: &str) -> Vec<StatusEntry> {
    let mut entries = Vec::new();
    for line in output.lines() {
        if line.starts_with("## ") || line.len() < 4 {
            continue;
        }
        let code = line[..2].to_string();
        let path = line[3..].trim();
        if path.is_empty() {
            continue;
        }
        if code.starts_with('R') || code.starts_with('C') {
            if let Some((old_path, new_path)) = path.split_once(" -> ") {
                let old_path = old_path.trim();
                let new_path = new_path.trim();
                let paths = [old_path, new_path]
                    .into_iter()
                    .filter(|value| !value.is_empty())
                    .map(|value| value.to_string())
                    .collect::<Vec<_>>();
                if !paths.is_empty() {
                    entries.push(StatusEntry { code, paths });
                }
                continue;
            }
        }
        entries.push(StatusEntry {
            code,
            paths: vec![path.to_string()],
        });
    }
    entries
}

fn normalize_status_path(path: &str) -> String {
    path.replace('\\', "/")
        .trim_start_matches("./")
        .to_string()
}

fn parse_status_branch_metadata(output: &str) -> StatusBranchMetadata {
    let mut metadata = StatusBranchMetadata::default();
    for line in output.lines() {
        let Some(summary) = line.strip_prefix("## ") else {
            continue;
        };
        let branch_summary = summary.split("...").next().unwrap_or(summary).trim();
        if branch_summary.eq_ignore_ascii_case("HEAD (no branch)")
            || branch_summary.eq_ignore_ascii_case("HEAD")
        {
            metadata.branch = None;
            metadata.is_detached = true;
            continue;
        }
        if let Some(branch) = branch_summary.strip_prefix("No commits yet on ") {
            metadata.branch = Some(branch.trim().to_string());
            metadata.is_detached = false;
            continue;
        }
        if let Some(branch) = branch_summary.strip_prefix("Initial commit on ") {
            metadata.branch = Some(branch.trim().to_string());
            metadata.is_detached = false;
            continue;
        }
        metadata.branch = Some(branch_summary.to_string());
        metadata.is_detached = false;
    }
    metadata
}

fn is_ignorable_generated_untracked_path(path: &str) -> bool {
    let normalized = normalize_status_path(path);
    let trimmed = normalized.trim_matches('/');
    if trimmed.is_empty() {
        return false;
    }

    trimmed == "__pycache__"
        || trimmed.starts_with("__pycache__/")
        || trimmed.contains("/__pycache__/")
        || trimmed.ends_with("/__pycache__")
        || trimmed.ends_with(".pyc")
        || trimmed.ends_with(".pyo")
}

fn filter_meaningful_status_entries(entries: Vec<StatusEntry>) -> Vec<StatusEntry> {
    entries
        .into_iter()
        .filter(|entry| {
            !(entry.code == "??"
                && entry
                    .paths
                    .iter()
                    .all(|path| is_ignorable_generated_untracked_path(path)))
        })
        .collect()
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
    let status = run_git(path, &["status", "--porcelain", "--branch"]).await?;
    let branch_metadata = parse_status_branch_metadata(&status);
    let status_entries = filter_meaningful_status_entries(parse_status_entries(&status));
    let untracked_output = run_git(path, &["ls-files", "--others", "--exclude-standard", "-z"]).await?;
    let untracked_files: Vec<String> = untracked_output
        .split('\0')
        .filter(|path| !path.is_empty())
        .map(normalize_status_path)
        .filter(|path| !is_ignorable_generated_untracked_path(path))
        .collect();
    let tracked_changed_files: Vec<String> = status_entries
        .iter()
        .filter(|entry| entry.code != "??")
        .flat_map(|entry| entry.paths.iter().cloned())
        .collect();
    let mut changed_files = tracked_changed_files.clone();
    changed_files.extend(untracked_files.iter().cloned());
    let origin_url = run_git_allow_fail(path, &["remote", "get-url", "origin"]).await?;
    Ok(RepoStatus {
        head_sha,
        branch: branch_metadata.branch.clone(),
        is_detached: branch_metadata.is_detached,
        is_dirty: !changed_files.is_empty(),
        changed_files,
        tracked_changed_files,
        untracked_files,
        origin_url: origin_url.and_then(|value| canonicalize_remote(&value)),
    })
}

pub async fn rev_parse(path: &Path, rev: &str) -> AppResult<Option<String>> {
    run_git_allow_fail(path, &["rev-parse", rev]).await
}

pub async fn merge_base(path: &Path, left: &str, right: &str) -> AppResult<Option<String>> {
    run_git_allow_fail(path, &["merge-base", left, right]).await
}

pub async fn commits_between(
    path: &Path,
    base: &str,
    head: &str,
    limit: usize,
) -> AppResult<Vec<RepoActionPreviewCommit>> {
    if base == head {
        return Ok(Vec::new());
    }
    let limit_flag = format!("--max-count={limit}");
    let range = format!("{base}..{head}");
    let output = run_git(
        path,
        &["log", "--format=%H%x09%s", &limit_flag, &range],
    )
    .await?;
    Ok(output
        .lines()
        .filter_map(|line| {
            let (sha, subject) = line.split_once('\t')?;
            Some(RepoActionPreviewCommit {
                sha: sha.to_string(),
                subject: subject.to_string(),
            })
        })
        .collect())
}

pub async fn diff_name_status(
    path: &Path,
    base: &str,
    head: &str,
) -> AppResult<Vec<RepoActionPreviewFile>> {
    if base == head {
        return Ok(Vec::new());
    }
    let range = format!("{base}..{head}");
    let output = run_git(path, &["diff", "--name-status", &range]).await?;
    Ok(output
        .lines()
        .filter_map(|line| {
            let mut parts = line.split('\t');
            let status = parts.next()?.trim();
            let path = parts.next_back()?.trim();
            Some(RepoActionPreviewFile {
                status: status.to_string(),
                path: path.to_string(),
            })
        })
        .collect())
}

pub async fn preview_merge_conflicts(
    path: &Path,
    current_head: &str,
    incoming_head: &str,
) -> AppResult<Vec<String>> {
    let Some(base) = merge_base(path, current_head, incoming_head).await? else {
        return Ok(Vec::new());
    };
    let output = output_command(
        "git",
        &[
            "merge-tree".to_string(),
            format!("{base}^{{tree}}"),
            current_head.to_string(),
            incoming_head.to_string(),
        ],
        Some(path),
    )
    .await?;
    if !output.status.success() && output.stdout.is_empty() {
        return Err(AppError::Git(
            String::from_utf8_lossy(&output.stderr).to_string(),
        ));
    }
    let output = String::from_utf8_lossy(&output.stdout).to_string();
    let mut conflicts = Vec::new();
    for line in output.lines() {
        if let Some(path) = line.trim().strip_prefix("CONFLICT (contents): Merge conflict in ") {
            conflicts.push(path.trim().to_string());
        }
    }
    conflicts.sort();
    conflicts.dedup();
    Ok(conflicts)
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

pub async fn merge_no_ff(path: &Path, target: &str, message: &str) -> AppResult<()> {
    run_git(
        path,
        &[
            "-c",
            "user.name=ComfyUI Patcher",
            "-c",
            "user.email=patcher@local.invalid",
            "merge",
            "--no-ff",
            "--no-edit",
            "-m",
            message,
            target,
        ],
    )
    .await?;
    Ok(())
}

pub async fn merge_abort(path: &Path) -> AppResult<()> {
    run_git(path, &["merge", "--abort"]).await?;
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

pub async fn ls_remote_default_branch_remote(remote: &str) -> AppResult<Option<String>> {
    let output = run_git_no_cwd(&["ls-remote", "--symref", remote, "HEAD"]).await?;
    Ok(parse_ls_remote_default_branch(&output))
}

fn parse_ls_remote_default_branch(output: &str) -> Option<String> {
    for line in output.lines() {
        let Some(rest) = line.strip_prefix("ref: refs/heads/") else {
            continue;
        };
        let (branch, head_name) = rest.split_once('\t')?;
        if head_name.trim() == "HEAD" && !branch.trim().is_empty() {
            return Some(branch.trim().to_string());
        }
    }
    None
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

    #[test]
    fn parses_ls_remote_default_branch() {
        let output = "ref: refs/heads/main\tHEAD\n0123456789abcdef\tHEAD";
        assert_eq!(
            parse_ls_remote_default_branch(output),
            Some("main".to_string())
        );
    }

    #[test]
    fn ignores_missing_ls_remote_default_branch() {
        let output = "0123456789abcdef\tHEAD";
        assert_eq!(parse_ls_remote_default_branch(output), None);
    }

    #[test]
    fn parses_status_branch_metadata_for_named_branch() {
        let metadata = parse_status_branch_metadata("## main...origin/main\n M src/lib.rs\n");
        assert_eq!(metadata.branch.as_deref(), Some("main"));
        assert!(!metadata.is_detached);
    }

    #[test]
    fn parses_status_branch_metadata_for_detached_head() {
        let metadata = parse_status_branch_metadata("## HEAD (no branch)\n M src/lib.rs\n");
        assert_eq!(metadata.branch, None);
        assert!(metadata.is_detached);
    }

    #[test]
    fn status_entries_ignore_branch_header_lines() {
        let entries = parse_status_entries(
            "## main...origin/main\n M src/lib.rs\n?? __pycache__/mod.pyc\n",
        );

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].paths, vec!["src/lib.rs".to_string()]);
        assert_eq!(entries[1].paths, vec!["__pycache__/mod.pyc".to_string()]);
    }

    #[test]
    fn parse_status_entries_splits_rename_and_copy_only() {
        let output = "R  old/path.py -> new/path.py\nC  src/a.py -> src/b.py\n?? literal -> arrow.py";
        let entries = parse_status_entries(output);
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].code, "R ");
        assert_eq!(
            entries[0].paths,
            vec!["old/path.py".to_string(), "new/path.py".to_string()]
        );
        assert_eq!(entries[1].code, "C ");
        assert_eq!(
            entries[1].paths,
            vec!["src/a.py".to_string(), "src/b.py".to_string()]
        );
        assert_eq!(entries[2].code, "??");
        assert_eq!(entries[2].paths, vec!["literal -> arrow.py".to_string()]);
    }

    #[test]
    fn normalize_status_path_normalizes_slashes_and_dot_prefix() {
        assert_eq!(
            normalize_status_path(r".\foo\__pycache__\x.pyc"),
            "foo/__pycache__/x.pyc".to_string()
        );
    }

    #[test]
    fn filter_meaningful_status_entries_drops_only_fully_ignorable_untracked_entries() {
        let entries = vec![
            StatusEntry {
                code: "??".to_string(),
                paths: vec!["__pycache__/x.pyc".to_string()],
            },
            StatusEntry {
                code: "??".to_string(),
                paths: vec!["foo.py".to_string()],
            },
            StatusEntry {
                code: "??".to_string(),
                paths: vec!["__pycache__/x.pyc".to_string(), "foo.py".to_string()],
            },
            StatusEntry {
                code: " M".to_string(),
                paths: vec!["__pycache__/tracked.pyc".to_string()],
            },
        ];

        let filtered = filter_meaningful_status_entries(entries);
        assert_eq!(filtered.len(), 3);
        assert_eq!(filtered[0].paths, vec!["foo.py".to_string()]);
        assert_eq!(
            filtered[1].paths,
            vec!["__pycache__/x.pyc".to_string(), "foo.py".to_string()]
        );
        assert_eq!(
            filtered[2].paths,
            vec!["__pycache__/tracked.pyc".to_string()]
        );
    }
}
