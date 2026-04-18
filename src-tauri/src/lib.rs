mod db;
mod deps;
mod errors;
mod execution;
mod git;
mod github;
mod models;
mod process;
mod registry;
mod state;
mod util;

use crate::deps::{execute_dependency_sync, plan_dependency_sync};
use crate::errors::{AppError, AppResult};
use crate::git::{
    apply_stash, canonicalize_remote, clone_repo, commits_between, diff_name_status,
    ensure_clean_or_apply_strategy, fetch_origin, fetch_refspec, inspect_repo, is_git_repo,
    RepoStatus,
    join_custom_node_path, merge_abort, merge_no_ff, preview_merge_conflicts, reset_hard,
    rev_parse, run_git_allow_fail, submodule_update, switch_branch, switch_detached,
    validate_custom_node_dir_name,
};
use crate::models::*;
use crate::state::AppState;
use crate::util::{detect_env_kind, infer_python};
use std::collections::{hash_map::Entry, HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use tauri::{AppHandle, Emitter, Manager, State};

const STACK_TRACKING_VERSION: u32 = 1;
const STACK_BRANCH_NAME: &str = "patcher/stack";

fn emit_operation(
    app: &AppHandle,
    operation_id: &str,
    phase: &str,
    level: &str,
    message: impl Into<String>,
) {
    let payload = OperationEvent {
        operation_id: operation_id.to_string(),
        phase: phase.to_string(),
        level: level.to_string(),
        message: message.into(),
        timestamp: crate::util::now_rfc3339(),
    };
    let _ = app.emit("operation-event", payload);
}

fn log_operation(
    state: &AppState,
    app: &AppHandle,
    operation_id: &str,
    phase: &str,
    level: &str,
    message: impl Into<String>,
) {
    let message = message.into();
    let _ = state.db.append_operation_log(
        operation_id,
        &format!("[{}] [{}] {}", phase, level, message),
    );
    emit_operation(app, operation_id, phase, level, message);
}

async fn is_tracking_managed_dir(path: &Path) -> bool {
    path.is_dir() && path.join(".tracking").is_file() && !is_git_repo(path).await
}

fn has_git_marker(path: &Path) -> bool {
    let dot_git = path.join(".git");
    dot_git.is_dir() || dot_git.is_file()
}

fn choose_tracking_backup_path(path: &Path, backup_root: &Path) -> PathBuf {
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("custom-node")
        .to_string();
    let parent = backup_root;
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_secs())
        .unwrap_or(0);
    for attempt in 0..1000usize {
        let suffix = if attempt == 0 {
            format!("{name}.tracking-backup-{seconds}")
        } else {
            format!("{name}.tracking-backup-{seconds}-{attempt}")
        };
        let candidate = parent.join(suffix);
        if !candidate.exists() {
            return candidate;
        }
    }
    parent.join(format!("{name}.tracking-backup"))
}

fn installation_retained_backup_root(installation: &Installation, kind: &str) -> PathBuf {
    let comfy_root = PathBuf::from(&installation.comfy_root);
    let backup_parent = comfy_root
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| comfy_root.clone());
    let installation_dir_name = comfy_root
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or("installation");
    backup_parent
        .join(".comfyui-patcher-backups")
        .join(installation_dir_name)
        .join(kind)
}

fn ensure_repo_lifecycle_supported(repo: &ManagedRepo, action: &str) -> AppResult<()> {
    if matches!(repo.kind, RepoKind::Core) {
        return Err(AppError::Conflict(format!(
            "{} is not supported for the primary ComfyUI checkout",
            action
        )));
    }
    Ok(())
}

fn checkpoint_label_and_reason(operation: &OperationRecord, repo: &ManagedRepo) -> (String, String) {
    let reason = operation
        .requested_input
        .clone()
        .unwrap_or_else(|| repo.display_name.clone());
    let label = match operation.kind {
        OperationKind::PatchCore
        | OperationKind::PatchFrontend
        | OperationKind::PatchCustomNode
        | OperationKind::ManageRepoStack => format!("Before applying {}", reason),
        OperationKind::InstallFrontend | OperationKind::InstallCustomNode => {
            format!("Before installing {}", reason)
        }
        OperationKind::UpdateRepo | OperationKind::UpdateAll => {
            format!("Before updating {}", repo.display_name)
        }
        OperationKind::RollbackRepo | OperationKind::RestoreCheckpoint => {
            format!("Before restoring {}", repo.display_name)
        }
        OperationKind::DisableRepo => format!("Before disabling {}", repo.display_name),
        OperationKind::UninstallRepo => format!("Before uninstalling {}", repo.display_name),
        OperationKind::UntrackRepo => format!("Before untracking {}", repo.display_name),
        OperationKind::StartInstallation
        | OperationKind::StopInstallation
        | OperationKind::RestartInstallation => format!("Before lifecycle action on {}", repo.display_name),
    };
    (label, reason)
}

#[derive(Debug, Clone)]
struct DiscoveredRepoState {
    repo: ManagedRepo,
    status: RepoStatus,
}

fn repo_has_tracked_local_changes(status: &RepoStatus) -> bool {
    !status.tracked_changed_files.is_empty()
}

async fn discover_repositories_for_installation(
    state: &AppState,
    installation: &Installation,
    prune_missing: bool,
) -> AppResult<(
    Option<DiscoveredRepoState>,
    Option<DiscoveredRepoState>,
    Vec<DiscoveredRepoState>,
)> {
    let root = PathBuf::from(&installation.comfy_root);
    let mut core_repo = None;
    let mut frontend_repo = None;
    let mut custom_node_repos = Vec::new();
    let ignored_paths: HashSet<String> = state
        .db
        .list_ignored_repo_paths(&installation.id)?
        .into_iter()
        .collect();
    let root_string = root.to_string_lossy().to_string();

    if !ignored_paths.contains(&root_string) && has_git_marker(&root) && is_git_repo(&root).await {
        let status = inspect_repo(&root).await?;
        let repo = state.db.upsert_repo(
            &installation.id,
            RepoKind::Core,
            "ComfyUI",
            &root_string,
            status.origin_url.as_deref(),
            status.head_sha.as_deref(),
            status.branch.as_deref(),
            status.is_detached,
            repo_has_tracked_local_changes(&status),
        )?;
        core_repo = Some(DiscoveredRepoState { repo, status });
    }

    if let Some(frontend_settings) = installation.frontend_settings.as_ref() {
        let frontend_root = PathBuf::from(&frontend_settings.repo_root);
        let frontend_root_string = frontend_root.to_string_lossy().to_string();
        if !ignored_paths.contains(&frontend_root_string)
            && has_git_marker(&frontend_root)
            && is_git_repo(&frontend_root).await
        {
            let status = inspect_repo(&frontend_root).await?;
            let repo = state.db.upsert_repo(
                &installation.id,
                RepoKind::Frontend,
                "ComfyUI Frontend",
                &frontend_root_string,
                status.origin_url.as_deref(),
                status.head_sha.as_deref(),
                status.branch.as_deref(),
                status.is_detached,
                repo_has_tracked_local_changes(&status),
            )?;
            frontend_repo = Some(DiscoveredRepoState { repo, status });
        }
    }

    let custom_nodes_dir = PathBuf::from(&installation.custom_nodes_dir);
    if custom_nodes_dir.exists() {
        for entry in std::fs::read_dir(&custom_nodes_dir)? {
            let entry = entry?;
            let path = entry.path();
            let path_string = path.to_string_lossy().to_string();
            if ignored_paths.contains(&path_string)
                || !path.is_dir()
                || !has_git_marker(&path)
                || !is_git_repo(&path).await
            {
                continue;
            }
            let display_name = path
                .file_name()
                .and_then(|v| v.to_str())
                .unwrap_or("custom-node")
                .to_string();
            let status = inspect_repo(&path).await?;
            let repo = state.db.upsert_repo(
                &installation.id,
                RepoKind::CustomNode,
                &display_name,
                &path_string,
                status.origin_url.as_deref(),
                status.head_sha.as_deref(),
                status.branch.as_deref(),
                status.is_detached,
                repo_has_tracked_local_changes(&status),
            )?;
            custom_node_repos.push(DiscoveredRepoState { repo, status });
        }
    }

    let mut discovered_paths: HashSet<String> = HashSet::new();
    if let Some(repo) = core_repo.as_ref() {
        discovered_paths.insert(repo.repo.local_path.clone());
    }
    if let Some(repo) = frontend_repo.as_ref() {
        discovered_paths.insert(repo.repo.local_path.clone());
    }
    for repo in &custom_node_repos {
        discovered_paths.insert(repo.repo.local_path.clone());
    }

    if prune_missing {
        for repo in state.db.list_repos_by_installation(&installation.id)? {
            if !discovered_paths.contains(&repo.local_path) {
                state.db.delete_repo(&repo.id)?;
            }
        }
    }

    Ok((core_repo, frontend_repo, custom_node_repos))
}

async fn discover_custom_node_repos_best_effort(
    state: &AppState,
    installation: &Installation,
) -> AppResult<Vec<ManagedRepo>> {
    let mut custom_node_repos = Vec::new();
    let ignored_paths: HashSet<String> = state
        .db
        .list_ignored_repo_paths(&installation.id)?
        .into_iter()
        .collect();
    let custom_nodes_dir = PathBuf::from(&installation.custom_nodes_dir);
    if !custom_nodes_dir.exists() {
        return Ok(custom_node_repos);
    }

    let entries = match std::fs::read_dir(&custom_nodes_dir) {
        Ok(entries) => entries,
        Err(err) => {
            eprintln!(
                "failed to scan custom_nodes directory {} during registry browsing: {}",
                custom_nodes_dir.to_string_lossy(),
                err
            );
            return Ok(custom_node_repos);
        }
    };

    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(err) => {
                eprintln!(
                    "failed to read an entry under {} during registry browsing: {}",
                    custom_nodes_dir.to_string_lossy(),
                    err
                );
                continue;
            }
        };
        let path = entry.path();
        let path_string = path.to_string_lossy().to_string();
        if ignored_paths.contains(&path_string)
            || !path.is_dir()
            || !has_git_marker(&path)
            || !is_git_repo(&path).await
        {
            continue;
        }

        let display_name = path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("custom-node")
            .to_string();

        let status = match inspect_repo(&path).await {
            Ok(status) => status,
            Err(err) => {
                eprintln!(
                    "skipping unreadable custom node repo {} during registry browsing: {}",
                    path.to_string_lossy(),
                    err
                );
                continue;
            }
        };

        let repo = state.db.upsert_repo(
            &installation.id,
            RepoKind::CustomNode,
            &display_name,
            &path_string,
            status.origin_url.as_deref(),
            status.head_sha.as_deref(),
            status.branch.as_deref(),
            status.is_detached,
            repo_has_tracked_local_changes(&status),
        )?;
        custom_node_repos.push(repo);
    }

    Ok(custom_node_repos)
}

fn dependency_manifest_files(repo: &ManagedRepo, repo_path: &Path) -> Vec<String> {
    dependency_manifest_candidates(&repo.kind)
        .iter()
        .filter_map(|relative| {
            let candidate = repo_path.join(relative);
            candidate.exists().then(|| candidate.to_string_lossy().to_string())
        })
        .collect()
}

fn dependency_manifest_candidates(repo_kind: &RepoKind) -> &'static [&'static str] {
    match repo_kind {
        RepoKind::Core | RepoKind::CustomNode => &["requirements.txt", "pyproject.toml"],
        RepoKind::Frontend => &[
            "package.json",
            "package-lock.json",
            "pnpm-lock.yaml",
            "yarn.lock",
        ],
    }
}

fn relevant_dependency_changes(
    repo: &ManagedRepo,
    changed_files: &[String],
) -> Vec<String> {
    let manifest_candidates = dependency_manifest_candidates(&repo.kind);
    changed_files
        .iter()
        .filter_map(|path| {
            let normalized = path.replace('\\', "/");
            let trimmed = normalized.trim_start_matches("./");
            manifest_candidates
                .iter()
                .any(|candidate| trimmed.eq_ignore_ascii_case(candidate))
                .then(|| path.clone())
        })
        .collect()
}

fn build_repo_dependency_state(
    installation: &Installation,
    repo: &ManagedRepo,
    repo_path: &Path,
    changed_files: &[String],
) -> Option<RepoDependencyState> {
    let manifest_files = dependency_manifest_files(repo, repo_path);
    let relevant_changed_files = relevant_dependency_changes(repo, changed_files);
    match plan_dependency_sync(installation, repo, repo_path) {
        Ok(plan) => Some(RepoDependencyState {
            plan: Some(plan),
            error: None,
            manifest_files,
            relevant_changed_files,
        }),
        Err(error) => {
            if manifest_files.is_empty() && relevant_changed_files.is_empty() {
                None
            } else {
                Some(RepoDependencyState {
                    plan: None,
                    error: Some(error.to_string()),
                    manifest_files,
                    relevant_changed_files,
                })
            }
        }
    }
}

async fn enrich_managed_repo(
    state: &AppState,
    installation: &Installation,
    repo: &ManagedRepo,
    precomputed_status: Option<RepoStatus>,
) -> AppResult<ManagedRepo> {
    let mut repo = repo.clone();
    let path = PathBuf::from(&repo.local_path);
    repo.last_scanned_at = Some(crate::util::now_rfc3339());
    repo.live_warnings.clear();

    if !path.exists() {
        repo.current_head_sha = None;
        repo.current_branch = None;
        repo.changed_files.clear();
        repo.dependency_state = None;
        repo.live_status = RepoLiveStatus::Missing;
        repo.live_warnings
            .push("Repository path no longer exists on disk.".to_string());
        return Ok(repo);
    }

    let status = if let Some(status) = precomputed_status {
        status
    } else {
        if !has_git_marker(&path) || !is_git_repo(&path).await {
            repo.current_head_sha = None;
            repo.current_branch = None;
            repo.changed_files.clear();
            repo.dependency_state = None;
            repo.live_status = RepoLiveStatus::NotGit;
            repo.live_warnings
                .push("Tracked path is no longer a git repository.".to_string());
            return Ok(repo);
        }
        inspect_repo(&path).await?
    };
    state.db.update_repo_state(
        &repo.id,
        status.origin_url.as_deref(),
        status.head_sha.as_deref(),
        status.branch.as_deref(),
        status.is_detached,
        repo_has_tracked_local_changes(&status),
    )?;

    let tracked_dirty = repo_has_tracked_local_changes(&status);

    repo.canonical_remote = status.origin_url.clone();
    repo.current_head_sha = status.head_sha.clone();
    repo.current_branch = status.branch.clone();
    repo.is_detached = status.is_detached;
    repo.is_dirty = tracked_dirty;
    repo.changed_files = status.changed_files.clone();
    repo.dependency_state =
        build_repo_dependency_state(installation, &repo, &path, &status.changed_files);

    let mut live_status = if tracked_dirty {
        RepoLiveStatus::Dirty
    } else {
        RepoLiveStatus::Clean
    };

    if tracked_dirty {
        repo.live_warnings
            .push("Worktree has local tracked changes outside ComfyUI Patcher.".to_string());
    }

    if let Some(expected_sha) = repo.tracked_target_resolved_sha.as_deref() {
        if status.head_sha.as_deref() != Some(expected_sha) {
            live_status = RepoLiveStatus::Drifted;
            repo.live_warnings.push(format!(
                "Tracked state no longer matches disk: expected HEAD {}, found {}.",
                expected_sha,
                status.head_sha.as_deref().unwrap_or("unknown")
            ));
        }
    }

    if let Some(tracked_state) = repo.tracked_state.as_ref() {
        if let Some(expected_remote) = canonicalize_remote(&tracked_state.base.canonical_repo_url) {
            if status.origin_url.as_deref() != Some(expected_remote.as_str()) {
                live_status = RepoLiveStatus::Drifted;
                repo.live_warnings.push(format!(
                    "Tracked remote {} no longer matches the repo remote on disk.",
                    expected_remote
                ));
            }
        }

        if let Some(materialized_branch) = tracked_state.materialized_branch.as_deref() {
            if status.branch.as_deref() != Some(materialized_branch) {
                live_status = RepoLiveStatus::Drifted;
                repo.live_warnings.push(format!(
                    "Overlay stack expects branch {}, but disk is on {}.",
                    materialized_branch,
                    status.branch.as_deref().unwrap_or("detached HEAD")
                ));
            }
        } else {
            match tracked_state.base.target_kind {
                TargetKind::Branch | TargetKind::DefaultBranch | TargetKind::NamedRef => {
                    if status.branch.as_deref() != Some(tracked_state.base.checkout_ref.as_str()) {
                        live_status = RepoLiveStatus::Drifted;
                        repo.live_warnings.push(format!(
                            "Tracked base expects branch {}, but disk is on {}.",
                            tracked_state.base.checkout_ref,
                            status.branch.as_deref().unwrap_or("detached HEAD")
                        ));
                    }
                }
                TargetKind::Tag | TargetKind::Commit => {
                    if !status.is_detached {
                        live_status = RepoLiveStatus::Drifted;
                        repo.live_warnings.push(
                            "Tracked base expects a detached checkout, but disk is on a branch."
                                .to_string(),
                        );
                    }
                }
                TargetKind::Pr => {}
            }
        }
    }

    if let Some(dependency_state) = repo.dependency_state.as_ref() {
        if !dependency_state.relevant_changed_files.is_empty() {
            live_status = RepoLiveStatus::Drifted;
            repo.live_warnings.push(format!(
                "Dependency manifests changed outside the app: {}.",
                dependency_state.relevant_changed_files.join(", ")
            ));
        }
    }

    repo.live_status = live_status;
    Ok(repo)
}

async fn hydrate_installation_detail(
    state: &AppState,
    installation_id: &str,
) -> AppResult<InstallationDetail> {
    let installation = state
        .db
        .get_installation(installation_id)?
        .ok_or_else(|| AppError::NotFound("installation not found".to_string()))?;
    let (core_repo, frontend_repo, discovered_custom_nodes) =
        discover_repositories_for_installation(state, &installation, false).await?;
    let mut discovered_statuses = HashMap::new();
    if let Some(repo) = core_repo {
        discovered_statuses.insert(repo.repo.id.clone(), repo.status);
    }
    if let Some(repo) = frontend_repo {
        discovered_statuses.insert(repo.repo.id.clone(), repo.status);
    }
    for repo in discovered_custom_nodes {
        discovered_statuses.insert(repo.repo.id.clone(), repo.status);
    }
    let mut detail = state.db.get_installation_detail(installation_id)?;

    if let Some(repo) = detail.core_repo.take() {
        let repo = enrich_managed_repo(
            state,
            &installation,
            &repo,
            discovered_statuses.get(&repo.id).cloned(),
        )
        .await?;
        for warning in &repo.live_warnings {
            detail
                .warnings
                .push(format!("{}: {}", repo.display_name, warning));
        }
        detail.core_repo = Some(repo);
    }

    if let Some(repo) = detail.frontend_repo.take() {
        let repo = enrich_managed_repo(
            state,
            &installation,
            &repo,
            discovered_statuses.get(&repo.id).cloned(),
        )
        .await?;
        for warning in &repo.live_warnings {
            detail
                .warnings
                .push(format!("{}: {}", repo.display_name, warning));
        }
        detail.frontend_repo = Some(repo);
    }

    let mut enriched_custom_nodes = Vec::with_capacity(detail.custom_node_repos.len());
    for repo in detail.custom_node_repos.drain(..) {
        let repo = enrich_managed_repo(
            state,
            &installation,
            &repo,
            discovered_statuses.get(&repo.id).cloned(),
        )
        .await?;
        for warning in &repo.live_warnings {
            detail
                .warnings
                .push(format!("{}: {}", repo.display_name, warning));
        }
        enriched_custom_nodes.push(repo);
    }
    detail.custom_node_repos = enriched_custom_nodes;
    Ok(detail)
}

async fn refresh_repo_state(state: &AppState, repo_id: &str) -> AppResult<()> {
    let repo = state
        .db
        .get_repo(repo_id)?
        .ok_or_else(|| AppError::NotFound("managed repo not found".to_string()))?;
    let status = inspect_repo(Path::new(&repo.local_path)).await?;
    state.db.update_repo_state(
        &repo.id,
        status.origin_url.as_deref(),
        status.head_sha.as_deref(),
        status.branch.as_deref(),
        status.is_detached,
        repo_has_tracked_local_changes(&status),
    )?;
    Ok(())
}

fn stack_preview_items(tracked_state: &TrackedRepoState) -> Vec<RepoStackPreviewItem> {
    let mut items = vec![RepoStackPreviewItem {
        kind: "base".to_string(),
        label: tracked_state.base.summary_label.clone(),
        enabled: true,
        status: None,
    }];
    items.extend(tracked_state.overlays.iter().map(|overlay| RepoStackPreviewItem {
        kind: "overlay".to_string(),
        label: overlay.summary_label.clone(),
        enabled: overlay.enabled,
        status: overlay
            .last_apply_status
            .as_ref()
            .map(|status| serde_json::to_string(status).unwrap_or_default())
            .map(|value| value.trim_matches('"').to_string()),
    }));
    items
}

fn preview_ref_name(prefix: &str, suffix: &str) -> String {
    let sanitized: String = suffix
        .chars()
        .map(|char| {
            if char.is_ascii_alphanumeric() || matches!(char, '-' | '_' | '.') {
                char
            } else {
                '-'
            }
        })
        .collect();
    format!("refs/patcher/previews/{prefix}-{sanitized}")
}

async fn ensure_preview_target_available(
    path: &Path,
    resolved: &ResolvedTarget,
) -> AppResult<String> {
    match resolved.target_kind {
        TargetKind::Pr => {
            let pr_number = resolved
                .pr_number
                .ok_or_else(|| AppError::Github("missing PR number".to_string()))?;
            let local_ref = preview_ref_name("pr", &pr_number.to_string());
            let refspec = format!("pull/{pr_number}/head:{local_ref}");
            fetch_refspec(path, "origin", &refspec).await?;
            Ok(local_ref)
        }
        TargetKind::Branch | TargetKind::DefaultBranch | TargetKind::NamedRef => {
            fetch_origin(path).await?;
            Ok(format!("origin/{}", resolved.checkout_ref))
        }
        TargetKind::Tag => {
            fetch_origin(path).await?;
            Ok(resolved.checkout_ref.clone())
        }
        TargetKind::Commit => {
            fetch_origin(path).await?;
            let resolved_sha = resolved
                .resolved_sha
                .clone()
                .unwrap_or_else(|| resolved.checkout_ref.clone());
            if rev_parse(path, &resolved_sha).await?.is_some() {
                return Ok(resolved_sha);
            }
            let local_ref = preview_ref_name("commit", &resolved_sha);
            let refspec = format!("{resolved_sha}:{local_ref}");
            fetch_refspec(path, "origin", &refspec).await?;
            Ok(local_ref)
        }
    }
}

#[cfg(windows)]
fn normalize_repo_relative_path(path: &str) -> String {
    path.replace('\\', "/")
        .trim_start_matches("./")
        .trim_matches('/')
        .to_ascii_lowercase()
        .to_string()
}

#[cfg(not(windows))]
fn normalize_repo_relative_path(path: &str) -> String {
    path.replace('\\', "/")
        .trim_start_matches("./")
        .trim_matches('/')
        .to_string()
}

fn file_change_writes_path(status: &str) -> bool {
    !matches!(status.chars().next(), Some('D'))
}

fn repo_relative_paths_collide(left: &str, right: &str) -> bool {
    let left = normalize_repo_relative_path(left);
    let right = normalize_repo_relative_path(right);
    if left.is_empty() || right.is_empty() {
        return false;
    }
    left == right
        || left.strip_prefix(&(right.clone() + "/")).is_some()
        || right.strip_prefix(&(left + "/")).is_some()
}

async fn incoming_write_paths_for_tracked_state(
    state: &AppState,
    installation: &Installation,
    repo: &ManagedRepo,
    tracked_state: &TrackedRepoState,
    current_head: &str,
) -> AppResult<HashSet<String>> {
    let path = Path::new(&repo.local_path);
    let base_resolved = resolve_existing_base_target(state, installation, repo, tracked_state).await?;
    let base_ref = ensure_preview_target_available(path, &base_resolved).await?;
    let mut write_paths = HashSet::new();

    for file_change in diff_name_status(path, current_head, &base_ref).await? {
        if file_change_writes_path(&file_change.status) {
            write_paths.insert(normalize_repo_relative_path(&file_change.path));
        }
    }

    for overlay in tracked_state.overlays.iter().filter(|overlay| overlay.enabled) {
        let overlay_resolved = resolve_target_for_context(
            state,
            installation,
            &repo.kind,
            Some(&repo.id),
            &overlay.source_input,
        )
        .await?;

        if !matches!(overlay_resolved.target_kind, TargetKind::Pr) {
            return Err(AppError::Conflict(format!(
                "overlay '{}' no longer resolves to a pull request",
                overlay.source_input
            )));
        }

        ensure_remote_matches(repo.canonical_remote.as_deref(), &overlay_resolved)?;

        let overlay_base_ref = overlay_resolved.pr_base_ref.clone().unwrap_or_default();
        if overlay_base_ref != base_resolved.checkout_ref {
            return Err(AppError::Conflict(format!(
                "PR #{} targets base branch '{}' but this stack is based on '{}'",
                overlay_resolved.pr_number.unwrap_or_default(),
                overlay_base_ref,
                base_resolved.checkout_ref
            )));
        }

        let overlay_ref = ensure_preview_target_available(path, &overlay_resolved).await?;
        for file_change in diff_name_status(path, &base_ref, &overlay_ref).await? {
            if file_change_writes_path(&file_change.status) {
                write_paths.insert(normalize_repo_relative_path(&file_change.path));
            }
        }
    }

    Ok(write_paths)
}

async fn preflight_abort_strategy_for_tracked_state(
    state: &AppState,
    installation: &Installation,
    repo: &ManagedRepo,
    tracked_state: &TrackedRepoState,
    status: &crate::git::RepoStatus,
) -> AppResult<()> {
    if !status.is_dirty {
        return Ok(());
    }

    if !status.tracked_changed_files.is_empty() {
        return Err(AppError::Conflict(format!(
            "repository has tracked local changes: {}",
            status.tracked_changed_files.join(", ")
        )));
    }

    if status.untracked_files.is_empty() {
        return Ok(());
    }

    let current_head = status
        .head_sha
        .as_deref()
        .ok_or_else(|| AppError::Git("repository has no HEAD".to_string()))?;
    let incoming_write_paths =
        incoming_write_paths_for_tracked_state(state, installation, repo, tracked_state, current_head)
            .await?;

    let mut colliding_paths = status
        .untracked_files
        .iter()
        .filter(|untracked| {
            incoming_write_paths
                .iter()
                .any(|incoming| repo_relative_paths_collide(untracked, incoming))
        })
        .map(|path| normalize_repo_relative_path(path))
        .collect::<Vec<_>>();
    colliding_paths.sort();
    colliding_paths.dedup();

    if colliding_paths.is_empty() {
        return Ok(());
    }

    Err(AppError::Conflict(format!(
        "repository has untracked local paths that would be overwritten by the incoming update: {}",
        colliding_paths.join(", ")
    )))
}

async fn preview_tracked_state_application(
    state: &AppState,
    installation: &Installation,
    repo: &ManagedRepo,
    tracked_state: &TrackedRepoState,
    action: &str,
) -> AppResult<RepoActionPreview> {
    let live_repo = enrich_managed_repo(state, installation, repo, None).await?;
    let path = Path::new(&repo.local_path);
    let base_resolved = resolve_existing_base_target(state, installation, repo, tracked_state).await?;
    let enabled_overlays: Vec<&TrackedPrOverlay> =
        tracked_state.overlays.iter().filter(|overlay| overlay.enabled).collect();
    let has_enabled_overlays = !enabled_overlays.is_empty();
    let target_head_sha = if has_enabled_overlays {
        None
    } else {
        base_resolved.resolved_sha.clone()
    };
    let target_summary = if enabled_overlays.is_empty() {
        tracked_state.base.summary_label.clone()
    } else {
        format!(
            "{} + {} overlay{}",
            tracked_state.base.summary_label,
            enabled_overlays.len(),
            if enabled_overlays.len() == 1 { "" } else { "s" }
        )
    };
    let target_ref = tracked_state
        .materialized_branch
        .clone()
        .or_else(|| Some(base_resolved.checkout_ref.clone()));
    let mut warnings = live_repo.live_warnings.clone();
    if matches!(live_repo.live_status, RepoLiveStatus::Missing | RepoLiveStatus::NotGit) {
        return Ok(RepoActionPreview {
            action: action.to_string(),
            repo_id: Some(repo.id.clone()),
            repo_display_name: repo.display_name.clone(),
            current_head_sha: None,
            target_head_sha,
            target_summary,
            target_ref,
            commits: Vec::new(),
            file_changes: Vec::new(),
            warnings,
            conflict_files: Vec::new(),
            stack_preview: stack_preview_items(tracked_state),
            dependency_state: None,
        });
    }

    let mut commits = Vec::new();
    let mut file_changes = Vec::new();
    if has_enabled_overlays {
        warnings.push(
            "Full commit/file preview is unavailable for overlay stacks because the final merged stack state is not synthesized during preview."
                .to_string(),
        );
    } else if let (Some(current_head), Some(_target_head)) = (
        live_repo.current_head_sha.as_deref(),
        target_head_sha.as_deref(),
    ) {
        match ensure_preview_target_available(path, &base_resolved).await {
            Ok(base_ref) => {
                match commits_between(path, current_head, &base_ref, 20).await {
                    Ok(next_commits) => commits = next_commits,
                    Err(error) => warnings.push(format!(
                        "Unable to inspect commits for this preview target: {}",
                        error
                    )),
                }
                match diff_name_status(path, current_head, &base_ref).await {
                    Ok(next_file_changes) => file_changes = next_file_changes,
                    Err(error) => warnings.push(format!(
                        "Unable to inspect changed files for this preview target: {}",
                        error
                    )),
                }
            }
            Err(error) => warnings.push(format!(
                "Unable to fetch the preview base into the local repo; commit and file previews are incomplete: {}",
                error
            )),
        }
    }

    if !has_enabled_overlays && live_repo.current_head_sha == target_head_sha {
        warnings.push("Current HEAD already matches this preview target.".to_string());
    }

    let mut conflict_files = Vec::new();
    if !enabled_overlays.is_empty() {
        let base_probe_head = match ensure_preview_target_available(path, &base_resolved).await {
            Ok(base_ref) => Some(base_ref),
            Err(error) => {
                warnings.push(format!(
                    "Unable to fetch the preview base for conflict probing: {}",
                    error
                ));
                None
            }
        };
        if let Some(base_probe_head) = base_probe_head.as_deref() {
            for overlay in &enabled_overlays {
                let overlay_resolved = resolve_target_for_context(
                    state,
                    installation,
                    &repo.kind,
                    Some(&repo.id),
                    &overlay.source_input,
                )
                .await?;
                match ensure_preview_target_available(path, &overlay_resolved).await {
                    Ok(overlay_ref) => match preview_merge_conflicts(path, base_probe_head, &overlay_ref).await {
                        Ok(next_conflict_files) => conflict_files.extend(next_conflict_files),
                        Err(error) => warnings.push(format!(
                            "Unable to probe conflicts for {}: {}",
                            overlay.summary_label, error
                        )),
                    },
                    Err(error) => warnings.push(format!(
                        "Unable to fetch preview objects for {} during conflict probing: {}",
                        overlay.summary_label, error
                    )),
                }
            }
        }
        if enabled_overlays.len() > 1 {
            warnings.push(
                "Conflict probing is computed per overlay against the resolved base before any stack mutation."
                    .to_string(),
            );
        }
    }
    conflict_files.sort();
    conflict_files.dedup();

    Ok(RepoActionPreview {
        action: action.to_string(),
        repo_id: Some(repo.id.clone()),
        repo_display_name: repo.display_name.clone(),
        current_head_sha: live_repo.current_head_sha.clone(),
        target_head_sha,
        target_summary,
        target_ref,
        commits,
        file_changes,
        warnings,
        conflict_files,
        stack_preview: stack_preview_items(tracked_state),
        dependency_state: live_repo.dependency_state.clone(),
    })
}

async fn preview_resolved_install_target(
    state: &AppState,
    installation: &Installation,
    kind: RepoKind,
    input: &str,
) -> AppResult<RepoActionPreview> {
    let resolved = resolve_target_for_context(state, installation, &kind, None, input).await?;
    let stack_preview = if matches!(resolved.target_kind, TargetKind::Pr) {
        vec![
            RepoStackPreviewItem {
                kind: "base".to_string(),
                label: resolved
                    .pr_base_ref
                    .clone()
                    .map(|base| format!("branch {base}"))
                    .unwrap_or_else(|| "base branch".to_string()),
                enabled: true,
                status: None,
            },
            RepoStackPreviewItem {
                kind: "overlay".to_string(),
                label: resolved.summary_label.clone(),
                enabled: true,
                status: Some("pending".to_string()),
            },
        ]
    } else {
        vec![RepoStackPreviewItem {
            kind: "base".to_string(),
            label: resolved.summary_label.clone(),
            enabled: true,
            status: None,
        }]
    };
    Ok(RepoActionPreview {
        action: "Preview target".to_string(),
        repo_id: None,
        repo_display_name: match kind {
            RepoKind::Core => "ComfyUI".to_string(),
            RepoKind::Frontend => "ComfyUI Frontend".to_string(),
            RepoKind::CustomNode => resolved.suggested_local_dir_name.clone(),
        },
        current_head_sha: None,
        target_head_sha: resolved.resolved_sha.clone(),
        target_summary: resolved.summary_label.clone(),
        target_ref: Some(resolved.checkout_ref.clone()),
        commits: Vec::new(),
        file_changes: Vec::new(),
        warnings: vec!["No local checkout exists yet, so this preview only shows the resolved target.".to_string()],
        conflict_files: Vec::new(),
        stack_preview,
        dependency_state: None,
    })
}

async fn resolve_target_for_context(
    state: &AppState,
    installation: &Installation,
    kind: &RepoKind,
    repo_id: Option<&str>,
    input: &str,
) -> AppResult<ResolvedTarget> {
    let repo = match repo_id {
        Some(id) => state.db.get_repo(id)?,
        None => match kind {
            RepoKind::Core => {
                state
                    .db
                    .get_installation_detail(&installation.id)?
                    .core_repo
            }
            RepoKind::Frontend => {
                state
                    .db
                    .get_installation_detail(&installation.id)?
                    .frontend_repo
            }
            RepoKind::CustomNode => None,
        },
    };
    let current_remote = repo.as_ref().and_then(|r| r.canonical_remote.as_deref());
    let current_repo_path = repo.as_ref().map(|r| Path::new(&r.local_path));
    state
        .github
        .resolve_target(input, current_remote, current_repo_path)
        .await
}

fn ensure_remote_matches(current: Option<&str>, target: &ResolvedTarget) -> AppResult<()> {
    let current = current.ok_or_else(|| {
        AppError::Conflict(
            "managed repository has no canonical remote; cannot validate resolved target repo"
                .to_string(),
        )
    })?;
    let current = canonicalize_remote(current).ok_or_else(|| {
        AppError::Conflict(format!(
            "managed repository remote '{}' could not be canonicalized for validation",
            current
        ))
    })?;
    let target_remote = canonicalize_remote(&target.canonical_repo_url).ok_or_else(|| {
        AppError::Conflict(format!(
            "resolved target repo '{}' could not be canonicalized for validation",
            target.canonical_repo_url
        ))
    })?;
    if current != target_remote {
        return Err(AppError::Conflict(format!(
            "resolved target repo {} does not match managed repo remote {}",
            target_remote, current
        )));
    }
    Ok(())
}


fn canonical_target_remote(resolved: &ResolvedTarget) -> AppResult<String> {
    canonicalize_remote(&resolved.canonical_repo_url).ok_or_else(|| {
        AppError::Conflict(format!(
            "resolved target repo '{}' could not be canonicalized",
            resolved.canonical_repo_url
        ))
    })
}

fn make_tracked_base_target(resolved: &ResolvedTarget) -> AppResult<TrackedBaseTarget> {
    if matches!(resolved.target_kind, TargetKind::Pr) {
        return Err(AppError::Conflict(
            "pull requests cannot be stored as a repo base target".to_string(),
        ));
    }
    Ok(TrackedBaseTarget {
        source_input: resolved.source_input.clone(),
        target_kind: resolved.target_kind.clone(),
        canonical_repo_url: canonical_target_remote(resolved)?,
        checkout_ref: resolved.checkout_ref.clone(),
        resolved_sha: resolved.resolved_sha.clone(),
        summary_label: resolved.summary_label.clone(),
    })
}

fn make_overlay_id(pr_number: u64) -> String {
    format!("pr-{pr_number}")
}

fn make_tracked_overlay(resolved: &ResolvedTarget, position: usize) -> AppResult<TrackedPrOverlay> {
    let pr_number = resolved
        .pr_number
        .ok_or_else(|| AppError::Github("missing PR number".to_string()))?;
    let pr_base_repo_url = resolved
        .pr_base_repo_url
        .as_deref()
        .and_then(canonicalize_remote)
        .ok_or_else(|| AppError::Github("missing PR base repository".to_string()))?;
    let pr_base_ref = resolved
        .pr_base_ref
        .clone()
        .ok_or_else(|| AppError::Github("missing PR base ref".to_string()))?;
    Ok(TrackedPrOverlay {
        id: make_overlay_id(pr_number),
        source_input: resolved.source_input.clone(),
        canonical_repo_url: canonical_target_remote(resolved)?,
        pr_number,
        pr_base_repo_url,
        pr_base_ref,
        pr_head_repo_url: resolved
            .pr_head_repo_url
            .as_deref()
            .and_then(canonicalize_remote),
        pr_head_ref: resolved.pr_head_ref.clone(),
        resolved_sha: resolved.resolved_sha.clone(),
        summary_label: resolved.summary_label.clone(),
        position,
        enabled: true,
        last_apply_status: None,
        last_error: None,
    })
}

fn normalize_overlay_positions(tracked_state: &mut TrackedRepoState) {
    for (index, overlay) in tracked_state.overlays.iter_mut().enumerate() {
        overlay.position = index;
    }
}

fn branch_like_target_kind(kind: &TargetKind) -> bool {
    matches!(
        kind,
        TargetKind::Branch | TargetKind::DefaultBranch | TargetKind::NamedRef
    )
}

async fn load_repo_tracked_state(
    state: &AppState,
    installation: &Installation,
    repo: &ManagedRepo,
) -> AppResult<Option<TrackedRepoState>> {
    if let Some(tracked_state) = repo.tracked_state.clone() {
        return Ok(Some(tracked_state));
    }

    let Some(raw_input) = repo.tracked_target_input.as_deref().map(str::trim) else {
        return Ok(None);
    };
    if raw_input.is_empty() {
        return Ok(None);
    }

    let Some(target_kind) = repo.tracked_target_kind.as_ref() else {
        return Err(AppError::InvalidInput(
            "repo has tracked input but no tracked target kind".to_string(),
        ));
    };

    if !matches!(target_kind, TargetKind::Pr) {
        let canonical_repo_url = repo
            .canonical_remote
            .as_deref()
            .and_then(canonicalize_remote)
            .ok_or_else(|| {
                AppError::Conflict(
                    "tracked base target cannot be reconstructed because the repo remote is missing"
                        .to_string(),
                )
            })?;
        return Ok(Some(TrackedRepoState {
            version: STACK_TRACKING_VERSION,
            base: TrackedBaseTarget {
                source_input: raw_input.to_string(),
                target_kind: target_kind.clone(),
                canonical_repo_url,
                checkout_ref: raw_input.to_string(),
                resolved_sha: repo.tracked_target_resolved_sha.clone(),
                summary_label: raw_input.to_string(),
            },
            overlays: Vec::new(),
            materialized_branch: None,
        }));
    }

    let resolved =
        resolve_target_for_context(state, installation, &repo.kind, Some(&repo.id), raw_input).await?;
    ensure_remote_matches(repo.canonical_remote.as_deref(), &resolved)?;
    let base_ref = resolved
        .pr_base_ref
        .clone()
        .ok_or_else(|| AppError::Github("tracked PR is missing a base ref".to_string()))?;
    let canonical_repo_url = resolved
        .pr_base_repo_url
        .as_deref()
        .and_then(canonicalize_remote)
        .or_else(|| canonicalize_remote(&resolved.canonical_repo_url))
        .ok_or_else(|| AppError::Github("tracked PR base repo could not be canonicalized".to_string()))?;
    Ok(Some(TrackedRepoState {
        version: STACK_TRACKING_VERSION,
        base: TrackedBaseTarget {
            source_input: base_ref.clone(),
            target_kind: TargetKind::Branch,
            canonical_repo_url,
            checkout_ref: base_ref.clone(),
            resolved_sha: None,
            summary_label: format!("branch {base_ref}"),
        },
        overlays: vec![make_tracked_overlay(&resolved, 0)?],
        materialized_branch: Some(STACK_BRANCH_NAME.to_string()),
    }))
}

async fn resolve_existing_base_target(
    state: &AppState,
    installation: &Installation,
    repo: &ManagedRepo,
    tracked_state: &TrackedRepoState,
) -> AppResult<ResolvedTarget> {
    let resolved = resolve_target_for_context(
        state,
        installation,
        &repo.kind,
        Some(&repo.id),
        &tracked_state.base.source_input,
    )
    .await?;
    if matches!(resolved.target_kind, TargetKind::Pr) {
        return Err(AppError::Conflict(
            "tracked repo base resolved to a pull request; base targets must be branch-like, tag, or commit targets"
                .to_string(),
        ));
    }
    ensure_remote_matches(repo.canonical_remote.as_deref(), &resolved)?;
    Ok(resolved)
}

fn restore_checkpoint_error(primary: AppError, restore_error: AppError) -> AppError {
    AppError::Git(format!(
        "{}; additionally failed to restore the previous checkout: {}",
        primary, restore_error
    ))
}

async fn restore_checkpoint_state(
    state: &AppState,
    path: &Path,
    repo_id: &str,
    checkpoint: &RepoCheckpoint,
    restore_stash: bool,
) -> AppResult<()> {
    let mut restore_errors = Vec::new();

    if checkpoint.old_is_detached {
        if let Err(err) = switch_detached(path, &checkpoint.old_head_sha).await {
            restore_errors.push(format!("failed to restore detached HEAD: {err}"));
        }
    } else if let Some(branch) = checkpoint.old_branch.as_deref() {
        if let Err(err) = switch_branch(path, branch, Some(&checkpoint.old_head_sha)).await {
            restore_errors.push(format!("failed to restore branch '{}': {err}", branch));
        }
    } else {
        if let Err(err) = switch_detached(path, &checkpoint.old_head_sha).await {
            restore_errors.push(format!("failed to restore detached HEAD: {err}"));
        }
    }
    if let Err(err) = reset_hard(path, &checkpoint.old_head_sha).await {
        restore_errors.push(format!("failed to reset HEAD {}: {err}", checkpoint.old_head_sha));
    }

    if restore_stash && checkpoint.stash_created {
        let Some(stash_ref) = checkpoint.stash_ref.as_deref() else {
            restore_errors.push(
                "checkpoint indicates a stash was created but no stash reference was stored"
                    .to_string(),
            );
            if checkpoint.has_tracked_target_snapshot {
                state.db.restore_repo_tracked_target(
                    repo_id,
                    checkpoint.old_tracked_target_kind.as_ref(),
                    checkpoint.old_tracked_target_input.as_deref(),
                    checkpoint.old_tracked_target_resolved_sha.as_deref(),
                )?;
            }
            return Err(AppError::Git(restore_errors.join("; ")));
        };
        if let Err(err) = apply_stash(path, stash_ref).await {
            restore_errors.push(format!("failed to restore stash: {err}"));
        }
    }

    if checkpoint.has_tracked_target_snapshot {
        if let Err(err) = state.db.restore_repo_tracked_target(
            repo_id,
            checkpoint.old_tracked_target_kind.as_ref(),
            checkpoint.old_tracked_target_input.as_deref(),
            checkpoint.old_tracked_target_resolved_sha.as_deref(),
        ) {
            restore_errors.push(format!("failed to restore tracked target metadata: {err}"));
        }
    }

    if restore_errors.is_empty() {
        Ok(())
    } else {
        Err(AppError::Git(restore_errors.join("; ")))
    }
}

fn update_overlay_from_resolved(
    overlay: &mut TrackedPrOverlay,
    resolved: &ResolvedTarget,
    position: usize,
    status: OverlayApplyStatus,
    last_error: Option<String>,
) -> AppResult<()> {
    let pr_number = resolved
        .pr_number
        .ok_or_else(|| AppError::Github("missing PR number".to_string()))?;
    let pr_base_repo_url = resolved
        .pr_base_repo_url
        .as_deref()
        .and_then(canonicalize_remote)
        .ok_or_else(|| AppError::Github("missing PR base repository".to_string()))?;
    let pr_base_ref = resolved
        .pr_base_ref
        .clone()
        .ok_or_else(|| AppError::Github("missing PR base ref".to_string()))?;
    overlay.id = make_overlay_id(pr_number);
    overlay.source_input = resolved.source_input.clone();
    overlay.canonical_repo_url = canonical_target_remote(resolved)?;
    overlay.pr_number = pr_number;
    overlay.pr_base_repo_url = pr_base_repo_url;
    overlay.pr_base_ref = pr_base_ref;
    overlay.pr_head_repo_url = resolved
        .pr_head_repo_url
        .as_deref()
        .and_then(canonicalize_remote);
    overlay.pr_head_ref = resolved.pr_head_ref.clone();
    overlay.resolved_sha = resolved.resolved_sha.clone();
    overlay.summary_label = resolved.summary_label.clone();
    overlay.position = position;
    overlay.last_apply_status = Some(status);
    overlay.last_error = last_error;
    Ok(())
}

async fn materialize_tracked_state(
    state: &AppState,
    app: &AppHandle,
    operation_id: &str,
    installation: &Installation,
    repo: &ManagedRepo,
    tracked_state: &TrackedRepoState,
) -> Result<TrackedRepoState, (TrackedRepoState, AppError)> {
    let path = Path::new(&repo.local_path);
    let mut next_state = tracked_state.clone();
    next_state.version = STACK_TRACKING_VERSION;
    normalize_overlay_positions(&mut next_state);
    for overlay in &mut next_state.overlays {
        overlay.last_error = None;
        overlay.last_apply_status = Some(if overlay.enabled {
            OverlayApplyStatus::Pending
        } else {
            OverlayApplyStatus::Disabled
        });
    }

    let base_resolved = match resolve_existing_base_target(state, installation, repo, &next_state).await {
        Ok(resolved) => resolved,
        Err(err) => return Err((next_state, err)),
    };
    next_state.base = match make_tracked_base_target(&base_resolved) {
        Ok(base) => base,
        Err(err) => return Err((next_state, err)),
    };

    if next_state.overlays.is_empty() {
        next_state.materialized_branch = None;
        if let Err(err) = apply_resolved_target(state, app, operation_id, repo, &base_resolved).await {
            return Err((next_state, err));
        }
        return Ok(next_state);
    }

    if !branch_like_target_kind(&base_resolved.target_kind) {
        return Err((
            next_state,
            AppError::Conflict(
                "PR overlay stacks require a branch-like base target".to_string(),
            ),
        ));
    }

    log_operation(
        state,
        app,
        operation_id,
        "fetch",
        "info",
        format!("materializing stack from {}", next_state.base.summary_label),
    );
    if let Err(err) = fetch_origin(path).await {
        return Err((next_state, err));
    }
    let remote_ref = format!("origin/{}", base_resolved.checkout_ref);
    log_operation(
        state,
        app,
        operation_id,
        "checkout",
        "info",
        format!("resetting {} from {}", STACK_BRANCH_NAME, remote_ref),
    );
    if let Err(err) = switch_branch(path, STACK_BRANCH_NAME, Some(&remote_ref)).await {
        return Err((next_state, err));
    }
    if let Err(err) = reset_hard(path, &remote_ref).await {
        return Err((next_state, err));
    }
    next_state.materialized_branch = Some(STACK_BRANCH_NAME.to_string());

    for (position, overlay) in next_state.overlays.iter_mut().enumerate() {
        overlay.position = position;
        if !overlay.enabled {
            overlay.last_apply_status = Some(OverlayApplyStatus::Disabled);
            overlay.last_error = None;
            continue;
        }

        let overlay_resolved = match resolve_target_for_context(
            state,
            installation,
            &repo.kind,
            Some(&repo.id),
            &overlay.source_input,
        )
        .await
        {
            Ok(resolved) => resolved,
            Err(err) => {
                overlay.last_apply_status = Some(OverlayApplyStatus::Error);
                overlay.last_error = Some(err.to_string());
                return Err((next_state, err));
            }
        };

        if !matches!(overlay_resolved.target_kind, TargetKind::Pr) {
            let err = AppError::Conflict(format!(
                "overlay '{}' no longer resolves to a pull request",
                overlay.source_input
            ));
            overlay.last_apply_status = Some(OverlayApplyStatus::Error);
            overlay.last_error = Some(err.to_string());
            return Err((next_state, err));
        }

        if let Err(err) = ensure_remote_matches(repo.canonical_remote.as_deref(), &overlay_resolved) {
            overlay.last_apply_status = Some(OverlayApplyStatus::Error);
            overlay.last_error = Some(err.to_string());
            return Err((next_state, err));
        }

        let overlay_base_ref = overlay_resolved.pr_base_ref.clone().unwrap_or_default();
        if overlay_base_ref != base_resolved.checkout_ref {
            let err = AppError::Conflict(format!(
                "PR #{} targets base branch '{}' but this stack is based on '{}'",
                overlay_resolved.pr_number.unwrap_or_default(),
                overlay_base_ref,
                base_resolved.checkout_ref
            ));
            overlay.last_apply_status = Some(OverlayApplyStatus::Conflict);
            overlay.last_error = Some(err.to_string());
            return Err((next_state, err));
        }

        if let Err(err) = update_overlay_from_resolved(
            overlay,
            &overlay_resolved,
            position,
            OverlayApplyStatus::Pending,
            None,
        ) {
            overlay.last_apply_status = Some(OverlayApplyStatus::Error);
            overlay.last_error = Some(err.to_string());
            return Err((next_state, err));
        }

        let pr_number = overlay.pr_number;
        let overlay_ref = format!("patcher/pr-{pr_number}");
        let refspec = format!("pull/{pr_number}/head:{overlay_ref}");
        log_operation(
            state,
            app,
            operation_id,
            "fetch",
            "info",
            format!("fetching PR #{pr_number}"),
        );
        if let Err(err) = fetch_refspec(path, "origin", &refspec).await {
            overlay.last_apply_status = Some(OverlayApplyStatus::Error);
            overlay.last_error = Some(err.to_string());
            return Err((next_state, err));
        }
        log_operation(
            state,
            app,
            operation_id,
            "checkout",
            "info",
            format!("merging PR #{pr_number} into {}", STACK_BRANCH_NAME),
        );
        if let Err(err) = merge_no_ff(
            path,
            &overlay_ref,
            &format!("patcher stack: merge PR #{pr_number}"),
        )
        .await
        {
            let merge_in_progress = match run_git_allow_fail(
                path,
                &["rev-parse", "-q", "--verify", "MERGE_HEAD"],
            )
            .await
            {
                Ok(result) => result.is_some(),
                Err(probe_err) => {
                    let merge_error = restore_checkpoint_error(err, probe_err);
                    overlay.last_apply_status = Some(OverlayApplyStatus::Conflict);
                    overlay.last_error = Some(merge_error.to_string());
                    return Err((next_state, merge_error));
                }
            };
            let merge_error = if merge_in_progress {
                match merge_abort(path).await {
                    Ok(()) => err,
                    Err(abort_err) => restore_checkpoint_error(err, abort_err),
                }
            } else {
                err
            };
            overlay.last_apply_status = Some(OverlayApplyStatus::Conflict);
            overlay.last_error = Some(merge_error.to_string());
            return Err((next_state, merge_error));
        }
        overlay.last_apply_status = Some(OverlayApplyStatus::Applied);
        overlay.last_error = None;
    }

    log_operation(
        state,
        app,
        operation_id,
        "submodules",
        "info",
        "updating submodules",
    );
    let _ = submodule_update(path).await;
    Ok(next_state)
}

async fn apply_repo_tracking_state(
    state: &AppState,
    app: &AppHandle,
    operation_id: &str,
    installation: &Installation,
    repo: &ManagedRepo,
    tracked_state: &TrackedRepoState,
    dirty_repo_strategy: &DirtyRepoStrategy,
    sync_dependencies: bool,
    write_tracked_target: bool,
) -> AppResult<RepoCheckpoint> {
    let checkpoint = create_checkpoint_if_needed(
        state,
        installation,
        repo,
        Some(tracked_state),
        operation_id,
        dirty_repo_strategy,
    )
    .await?;
    log_operation(
        state,
        app,
        operation_id,
        "checkpoint",
        "info",
        format!("checkpoint {} created", checkpoint.id),
    );

    let path = Path::new(&repo.local_path);
    let result = async {
        let final_state = materialize_tracked_state(
            state,
            app,
            operation_id,
            installation,
            repo,
            tracked_state,
        )
        .await
        .map_err(|(_, err)| err)?;

        maybe_sync_dependencies(
            state,
            app,
            operation_id,
            installation,
            repo,
            path,
            sync_dependencies,
        )
        .await?;
        refresh_repo_state(state, &repo.id).await?;
        let refreshed_repo = state.db.get_repo(&repo.id)?.ok_or_else(|| {
            AppError::NotFound("managed repo not found after stack apply".to_string())
        })?;
        if write_tracked_target {
            state.db.set_repo_tracked_state(
                &repo.id,
                Some(&final_state),
                refreshed_repo.current_head_sha.as_deref(),
            )?;
        }

        Ok::<(), AppError>(())
    }
    .await;

    match result {
        Ok(()) => Ok(checkpoint),
        Err(err) => {
            let restore_result =
                restore_checkpoint_state(state, path, &repo.id, &checkpoint, true).await;
            if let Err(refresh_err) = refresh_repo_state(state, &repo.id).await {
                log_operation(
                    state,
                    app,
                    operation_id,
                    "error",
                    "warn",
                    format!("failed to refresh repo state after stack error: {refresh_err}"),
                );
            }
            match restore_result {
                Ok(()) => Err(err),
                Err(restore_err) => Err(restore_checkpoint_error(err, restore_err)),
            }
        }
    }
}

async fn build_requested_tracked_state_for_input(
    state: &AppState,
    installation: &Installation,
    repo: &ManagedRepo,
    input: &str,
    clear_overlays_on_base_change: bool,
) -> AppResult<TrackedRepoState> {
    let resolved =
        resolve_target_for_context(state, installation, &repo.kind, Some(&repo.id), input).await?;
    ensure_remote_matches(repo.canonical_remote.as_deref(), &resolved)?;

    if matches!(resolved.target_kind, TargetKind::Pr) {
        let Some(base_ref) = resolved.pr_base_ref.clone() else {
            return Err(AppError::Github("resolved PR is missing a base ref".to_string()));
        };
        let canonical_repo_url = resolved
            .pr_base_repo_url
            .as_deref()
            .and_then(canonicalize_remote)
            .or_else(|| canonicalize_remote(&resolved.canonical_repo_url))
            .ok_or_else(|| AppError::Github("resolved PR base repo could not be canonicalized".to_string()))?;
        let mut tracked_state = match load_repo_tracked_state(state, installation, repo).await? {
            Some(mut existing) => {
                let existing_base = resolve_existing_base_target(state, installation, repo, &existing).await?;
                existing.base = make_tracked_base_target(&existing_base)?;
                if !branch_like_target_kind(&existing.base.target_kind) {
                    return Err(AppError::Conflict(
                        "PR overlays require a branch-like base target".to_string(),
                    ));
                }
                if existing.base.checkout_ref != base_ref {
                    return Err(AppError::Conflict(format!(
                        "PR overlays for this repo must target base branch '{}', but this PR targets '{}'",
                        existing.base.checkout_ref, base_ref
                    )));
                }
                existing
            }
            None => TrackedRepoState {
                version: STACK_TRACKING_VERSION,
                base: TrackedBaseTarget {
                    source_input: base_ref.clone(),
                    target_kind: TargetKind::Branch,
                    canonical_repo_url,
                    checkout_ref: base_ref.clone(),
                    resolved_sha: None,
                    summary_label: format!("branch {base_ref}"),
                },
                overlays: Vec::new(),
                materialized_branch: Some(STACK_BRANCH_NAME.to_string()),
            },
        };
        let overlay = make_tracked_overlay(&resolved, tracked_state.overlays.len())?;
        if tracked_state
            .overlays
            .iter()
            .any(|existing| existing.pr_number == overlay.pr_number)
        {
            return Ok(tracked_state);
        }
        tracked_state.overlays.push(overlay);
        normalize_overlay_positions(&mut tracked_state);
        tracked_state.materialized_branch = Some(STACK_BRANCH_NAME.to_string());
        return Ok(tracked_state);
    }

    build_base_tracked_state_for_input(
        state,
        installation,
        repo,
        &resolved,
        clear_overlays_on_base_change,
    )
    .await
}

async fn build_base_tracked_state_for_input(
    state: &AppState,
    installation: &Installation,
    repo: &ManagedRepo,
    resolved: &ResolvedTarget,
    clear_overlays_on_base_change: bool,
) -> AppResult<TrackedRepoState> {
    if matches!(resolved.target_kind, TargetKind::Pr) {
        return Err(AppError::Conflict(
            "pull requests cannot be applied through the base-target flow".to_string(),
        ));
    }

    let existing_state = load_repo_tracked_state(state, installation, repo).await?;
    if existing_state
        .as_ref()
        .is_some_and(|tracked| !tracked.overlays.is_empty() && !clear_overlays_on_base_change)
    {
        return Err(AppError::Conflict(
            "repo already tracks PR overlays; use the explicit base replacement flow to clear them before changing the base target"
                .to_string(),
        ));
    }

    Ok(TrackedRepoState {
        version: STACK_TRACKING_VERSION,
        base: make_tracked_base_target(resolved)?,
        overlays: Vec::new(),
        materialized_branch: None,
    })
}

fn overlay_index(tracked_state: &TrackedRepoState, overlay_id: &str) -> AppResult<usize> {
    tracked_state
        .overlays
        .iter()
        .position(|overlay| overlay.id == overlay_id)
        .ok_or_else(|| AppError::NotFound(format!("overlay '{}' not found", overlay_id)))
}

fn stack_operation_summary(action: &str, repo: &ManagedRepo) -> String {
    format!("{action} for {}", repo.display_name)
}

async fn find_existing_custom_node_repo_by_remote(
    state: &AppState,
    installation: &Installation,
    resolved: &ResolvedTarget,
) -> AppResult<Option<ManagedRepo>> {
    let target_remote = canonical_target_remote(resolved)?;
    let mut matches: Vec<ManagedRepo> = Vec::new();

    let custom_nodes_dir = PathBuf::from(&installation.custom_nodes_dir);
    if custom_nodes_dir.exists() {
        let entries = match std::fs::read_dir(&custom_nodes_dir) {
            Ok(entries) => entries,
            Err(err) => {
                eprintln!(
                    "failed to scan custom_nodes directory {} during custom node remote-match probe: {}",
                    custom_nodes_dir.to_string_lossy(),
                    err
                );
                return Ok(None);
            }
        };
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(err) => {
                    eprintln!(
                        "failed to read an entry under {} during custom node remote-match probe: {}",
                        custom_nodes_dir.to_string_lossy(),
                        err
                    );
                    continue;
                }
            };
            let path = entry.path();
            if !path.is_dir() || !is_git_repo(&path).await {
                continue;
            }
            let status = match inspect_repo(&path).await {
                Ok(status) => status,
                Err(err) => {
                    eprintln!(
                        "skipping unreadable custom node repo {} during custom node remote-match probe: {}",
                        path.to_string_lossy(),
                        err
                    );
                    continue;
                }
            };
            let Some(live_remote) = status.origin_url.as_deref().and_then(canonicalize_remote) else {
                continue;
            };
            let remote_aliases = if live_remote == target_remote {
                vec![live_remote.clone()]
            } else {
                state.manager_registry.remote_aliases(&live_remote).await
            };
            if !remote_aliases.iter().any(|value| value == &target_remote) {
                continue;
            }
            let repo = state.db.upsert_repo(
                &installation.id,
                RepoKind::CustomNode,
                path.file_name()
                    .and_then(|value| value.to_str())
                    .unwrap_or("custom-node"),
                &path.to_string_lossy(),
                status.origin_url.as_deref(),
                status.head_sha.as_deref(),
                status.branch.as_deref(),
                status.is_detached,
                repo_has_tracked_local_changes(&status),
            )?;
            if !matches.iter().any(|existing| existing.id == repo.id) {
                matches.push(repo);
            }
        }
    }

    match matches.len() {
        0 => Ok(None),
        1 => Ok(matches.into_iter().next()),
        _ => Err(AppError::Conflict(format!(
            "multiple custom node directories match remote {}; resolve duplicates manually",
            target_remote
        ))),
    }
}

async fn preferred_custom_node_dir_name(
    state: &AppState,
    resolved: &ResolvedTarget,
) -> String {
    match state
        .manager_registry
        .preferred_dir_name_for_target(resolved)
        .await
    {
        Ok(Some(value)) => value,
        _ => resolved.suggested_local_dir_name.clone(),
    }
}

fn absolutize_path_against(root: &Path, value: &str) -> AppResult<PathBuf> {
    let path = PathBuf::from(value);
    let absolute = if path.is_absolute() {
        path
    } else {
        root.join(path)
    };
    if absolute.exists() {
        Ok(absolute.canonicalize()?)
    } else {
        Ok(absolute)
    }
}

fn normalize_path_components(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    let mut has_root = false;

    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => {
                normalized.push(component.as_os_str());
                has_root = true;
            }
            Component::CurDir => {}
            Component::ParentDir => {
                if normalized.file_name().is_some() {
                    normalized.pop();
                } else if !has_root {
                    normalized.push(component.as_os_str());
                }
            }
            Component::Normal(_) => normalized.push(component.as_os_str()),
        }
    }

    normalized
}


fn normalize_frontend_settings(
    root: &Path,
    custom_nodes_dir: &Path,
    frontend_settings: Option<FrontendSettings>,
) -> AppResult<Option<FrontendSettings>> {
    let Some(frontend_settings) = frontend_settings else {
        return Ok(None);
    };

    let repo_root_input = frontend_settings.repo_root.trim();
    if repo_root_input.is_empty() {
        return Ok(None);
    }

    let repo_root = normalize_path_components(&absolutize_path_against(root, repo_root_input)?);
    let normalized_root = normalize_path_components(root);
    let normalized_custom_nodes_dir = normalize_path_components(custom_nodes_dir);
    if repo_root == normalized_root {
        return Err(AppError::InvalidInput(
            "managed frontend repo root cannot be the ComfyUI root".to_string(),
        ));
    }
    if repo_root.starts_with(&normalized_custom_nodes_dir) {
        return Err(AppError::InvalidInput(
            "managed frontend repo root cannot be inside custom_nodes".to_string(),
        ));
    }

    let dist_input = frontend_settings.dist_path.trim();
    let dist_path = if dist_input.is_empty() {
        repo_root.join("dist")
    } else {
        normalize_path_components(&absolutize_path_against(&repo_root, dist_input)?)
    };

    Ok(Some(FrontendSettings {
        repo_root: repo_root.to_string_lossy().to_string(),
        dist_path: dist_path.to_string_lossy().to_string(),
        package_manager: frontend_settings.package_manager,
    }))
}

fn default_frontend_settings_for_installation(
    installation: &Installation,
    repo_root_override: Option<&Path>,
) -> AppResult<FrontendSettings> {
    let root = PathBuf::from(&installation.comfy_root);
    let custom_nodes_dir = PathBuf::from(&installation.custom_nodes_dir);
    let existing_settings = installation.frontend_settings.as_ref();
    let package_manager = existing_settings
        .map(|settings| settings.package_manager.clone())
        .unwrap_or(FrontendPackageManager::Auto);
    let repo_root = repo_root_override
        .map(Path::to_path_buf)
        .unwrap_or_else(|| {
            root.parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| root.clone())
                .join("ComfyUI_frontend")
        });
    normalize_frontend_settings(
        &root,
        &custom_nodes_dir,
        Some(FrontendSettings {
            repo_root: repo_root.to_string_lossy().to_string(),
            dist_path: existing_settings
                .map(|settings| settings.dist_path.trim().to_string())
                .filter(|dist_path| !dist_path.is_empty())
                .unwrap_or_default(),
            package_manager,
        }),
    )?
    .ok_or_else(|| {
        AppError::InvalidInput("failed to derive managed frontend settings".to_string())
    })
}

fn strip_frontend_root_args(args: &[String]) -> Vec<String> {
    let mut stripped = Vec::with_capacity(args.len());
    let mut index = 0usize;
    while index < args.len() {
        let current = &args[index];
        if current == "--front-end-root" {
            index += 1;
            if index < args.len() {
                index += 1;
            }
            continue;
        }
        if current.starts_with("--front-end-root=") {
            index += 1;
            continue;
        }
        stripped.push(current.clone());
        index += 1;
    }
    stripped
}

fn apply_managed_frontend_root(profile: &mut LaunchProfile, dist_path: &str) {
    profile.args = strip_frontend_root_args(&profile.args);
    profile.extra_args = Some({
        let mut extra = strip_frontend_root_args(profile.extra_args.as_deref().unwrap_or(&[]));
        extra.push("--front-end-root".to_string());
        extra.push(dist_path.to_string());
        extra
    });
    if let Some(restart_args) = profile.restart_args.as_ref() {
        profile.restart_args = Some(strip_frontend_root_args(restart_args));
    }
}

fn effective_launch_profile(
    installation: &Installation,
    require_managed_frontend_dist: bool,
) -> AppResult<LaunchProfile> {
    let mut profile = installation
        .launch_profile
        .clone()
        .ok_or_else(|| AppError::Process("installation has no launch profile".to_string()))?;
    let Some(frontend_settings) = installation.frontend_settings.as_ref() else {
        return Ok(profile);
    };
    let dist_path = PathBuf::from(&frontend_settings.dist_path);
    if !dist_path.exists() {
        if require_managed_frontend_dist {
            return Err(AppError::Process(format!(
                "managed frontend dist does not exist: {}",
                dist_path.to_string_lossy()
            )));
        }
        return Ok(profile);
    }
    apply_managed_frontend_root(&mut profile, &frontend_settings.dist_path);
    Ok(profile)
}

async fn acquire_installation_repo_locks(
    state: &AppState,
    installation_id: &str,
) -> AppResult<Vec<tokio::sync::OwnedMutexGuard<()>>> {
    let mut repos = state.db.list_repos_by_installation(installation_id)?;
    repos.sort_by(|left, right| left.id.cmp(&right.id));

    let mut guards = Vec::with_capacity(repos.len());
    for repo in repos {
        let repo_lock = state.repo_lock(&repo.id).await;
        guards.push(repo_lock.lock_owned().await);
    }

    Ok(guards)
}

fn normalize_python_executable(root: &Path, python_exe: Option<&str>) -> AppResult<PathBuf> {
    match python_exe.map(str::trim) {
        Some(value) if !value.is_empty() => {
            let candidate = PathBuf::from(value);
            if candidate.is_absolute() {
                if !candidate.is_file() {
                    return Err(AppError::InvalidInput(format!(
                        "python executable is not a file: {}",
                        candidate.to_string_lossy()
                    )));
                }
                return Ok(candidate.canonicalize()?);
            }
            if candidate.components().count() > 1 || value.contains(std::path::MAIN_SEPARATOR) {
                let resolved = absolutize_path_against(root, value)?;
                if !resolved.is_file() {
                    return Err(AppError::InvalidInput(format!(
                        "python executable is not a file: {}",
                        resolved.to_string_lossy()
                    )));
                }
                return Ok(resolved.canonicalize()?);
            }
            Ok(candidate)
        }
        _ => Ok(infer_python(root).unwrap_or_else(|| PathBuf::from("python"))),
    }
}

fn normalize_launch_profile(
    root: &Path,
    launch_profile: Option<LaunchProfile>,
) -> AppResult<Option<LaunchProfile>> {
    launch_profile
        .map(|mut profile| {
            profile.cwd = match profile.cwd.as_deref().map(str::trim) {
                None | Some("") => None,
                Some(cwd) => Some(
                    absolutize_path_against(root, cwd)?
                        .to_string_lossy()
                        .to_string(),
                ),
            };
            Ok::<LaunchProfile, AppError>(profile)
        })
        .transpose()
}

fn is_default_register_launcher(root: &Path, launch_profile: &LaunchProfile) -> bool {
    launch_profile.command == "python"
        && launch_profile.args.as_slice() == ["main.py"]
        && match launch_profile.cwd.as_deref() {
            None => true,
            Some(cwd) => Path::new(cwd) == root,
        }
}

fn merge_launch_profile_overrides(
    root: &Path,
    existing: Option<&LaunchProfile>,
    launch_profile: Option<LaunchProfile>,
) -> Option<LaunchProfile> {
    match (existing, launch_profile) {
        (Some(existing), Some(mut launch_profile)) => {
            if is_default_register_launcher(root, &launch_profile) {
                launch_profile.command = existing.command.clone();
                launch_profile.args = existing.args.clone();
                launch_profile.cwd = existing.cwd.clone();
            }
            launch_profile.mode = existing.mode.clone();
            if launch_profile
                .env
                .as_ref()
                .is_none_or(|values| values.is_empty())
            {
                launch_profile.env = existing.env.clone();
            }
            if launch_profile.extra_args.is_none() {
                launch_profile.extra_args = existing.extra_args.clone();
            }
            if launch_profile.stop_command.is_none() {
                launch_profile.stop_command = existing.stop_command.clone();
            }
            if launch_profile.stop_args.is_none() {
                launch_profile.stop_args = existing.stop_args.clone();
            }
            if launch_profile.restart_command.is_none() {
                launch_profile.restart_command = existing.restart_command.clone();
            }
            if launch_profile.restart_args.is_none() {
                launch_profile.restart_args = existing.restart_args.clone();
            }
            Some(launch_profile)
        }
        (_, launch_profile) => launch_profile,
    }
}

async fn create_checkpoint_if_needed(
    state: &AppState,
    installation: &Installation,
    repo: &ManagedRepo,
    tracked_state: Option<&TrackedRepoState>,
    operation_id: &str,
    strategy: &DirtyRepoStrategy,
) -> AppResult<RepoCheckpoint> {
    let path = Path::new(&repo.local_path);
    let status = inspect_repo(path).await?;
    let head = status
        .head_sha
        .clone()
        .ok_or_else(|| AppError::Git("repository has no HEAD".to_string()))?;

    if status.is_dirty && matches!(strategy, DirtyRepoStrategy::Abort) {
        let tracked_state = tracked_state.ok_or_else(|| {
            AppError::Conflict(
                "tracked target state is required for abort-strategy dirtiness preflight"
                    .to_string(),
            )
        })?;
        preflight_abort_strategy_for_tracked_state(
            state,
            installation,
            repo,
            tracked_state,
            &status,
        )
        .await?;
    }

    let operation = state
        .db
        .get_operation(operation_id)?
        .ok_or_else(|| AppError::NotFound("operation not found".to_string()))?;
    let (label, reason) = checkpoint_label_and_reason(&operation, repo);
    let dependency_state = build_repo_dependency_state(installation, repo, path, &status.changed_files);

    let checkpoint = state.db.create_checkpoint(
        &repo.id,
        operation_id,
        &head,
        status.branch.as_deref(),
        status.is_detached,
        true,
        repo.tracked_target_kind.as_ref(),
        repo.tracked_target_input.as_deref(),
        repo.tracked_target_resolved_sha.as_deref(),
        false,
        None,
        Some(&label),
        Some(&reason),
        dependency_state.as_ref(),
    )?;

    let stash_ref = if matches!(strategy, DirtyRepoStrategy::Abort) {
        None
    } else {
        ensure_clean_or_apply_strategy(path, strategy).await?
    };
    if let Some(stash_ref) = stash_ref {
        if let Err(db_err) =
            state
                .db
                .update_checkpoint_stash(&checkpoint.id, true, Some(&stash_ref))
        {
            let compensation = apply_stash(path, &stash_ref).await;
            return match compensation {
                Ok(()) => Err(db_err),
                Err(stash_err) => Err(AppError::Db(format!(
                    "{}; additionally failed to restore stashed worktree: {}",
                    db_err, stash_err
                ))),
            };
        }
    }

    state
        .db
        .get_checkpoint(&checkpoint.id)?
        .ok_or_else(|| AppError::Db("failed to reload checkpoint".to_string()))
}

async fn apply_resolved_target(
    state: &AppState,
    app: &AppHandle,
    operation_id: &str,
    repo: &ManagedRepo,
    resolved: &ResolvedTarget,
) -> AppResult<()> {
    let path = Path::new(&repo.local_path);
    log_operation(
        state,
        app,
        operation_id,
        "fetch",
        "info",
        format!("fetching {}", resolved.summary_label),
    );
    match resolved.target_kind {
        TargetKind::Pr => {
            let pr_number = resolved
                .pr_number
                .ok_or_else(|| AppError::Github("missing PR number".to_string()))?;
            let refspec = format!("pull/{pr_number}/head:{}", resolved.checkout_ref);
            fetch_refspec(path, "origin", &refspec).await?;
            log_operation(
                state,
                app,
                operation_id,
                "checkout",
                "info",
                format!("switching to {}", resolved.checkout_ref),
            );
            switch_branch(path, &resolved.checkout_ref, Some("FETCH_HEAD")).await?;
            reset_hard(path, "FETCH_HEAD").await?;
        }
        TargetKind::Branch | TargetKind::DefaultBranch => {
            fetch_origin(path).await?;
            let remote_ref = format!("origin/{}", resolved.checkout_ref);
            log_operation(
                state,
                app,
                operation_id,
                "checkout",
                "info",
                format!("switching to {remote_ref}"),
            );
            switch_branch(path, &resolved.checkout_ref, Some(&remote_ref)).await?;
            reset_hard(path, &remote_ref).await?;
        }
        TargetKind::Tag | TargetKind::Commit => {
            fetch_origin(path).await?;
            log_operation(
                state,
                app,
                operation_id,
                "checkout",
                "info",
                format!("detaching at {}", resolved.checkout_ref),
            );
            switch_detached(path, &resolved.checkout_ref).await?;
            reset_hard(path, &resolved.checkout_ref).await?;
        }
        TargetKind::NamedRef => {
            fetch_origin(path).await?;
            let remote_ref = format!("origin/{}", resolved.checkout_ref);
            switch_branch(path, &resolved.checkout_ref, Some(&remote_ref)).await?;
            reset_hard(path, &remote_ref).await?;
        }
    }
    log_operation(
        state,
        app,
        operation_id,
        "submodules",
        "info",
        "updating submodules",
    );
    let _ = submodule_update(path).await;
    Ok(())
}

async fn maybe_sync_dependencies(
    state: &AppState,
    app: &AppHandle,
    operation_id: &str,
    installation: &Installation,
    repo: &ManagedRepo,
    repo_path: &Path,
    enabled: bool,
) -> AppResult<()> {
    if !enabled {
        return Ok(());
    }
    let plan = plan_dependency_sync(installation, repo, repo_path)?;
    log_operation(
        state,
        app,
        operation_id,
        "dependency_plan",
        "info",
        format!("dependency plan: {} ({})", plan.strategy, plan.reason),
    );
    if plan.steps.is_empty() {
        return Ok(());
    }
    for step in &plan.steps {
        log_operation(
            state,
            app,
            operation_id,
            "dependency_sync",
            "info",
            format!("{} step: {} ({})", step.phase, step.strategy, step.reason),
        );
    }
    execute_dependency_sync(&plan).await?;
    Ok(())
}

async fn maybe_restart_installation(
    state: &AppState,
    app: &AppHandle,
    operation_id: &str,
    installation: &Installation,
    enabled: bool,
) -> AppResult<()> {
    if !enabled {
        return Ok(());
    }
    let lifecycle_lock = state.lifecycle_lock();
    let _lifecycle_guard = lifecycle_lock.lock().await;
    if !state.processes.is_running(&installation.id).await? {
        log_operation(
            state,
            app,
            operation_id,
            "restart",
            "info",
            "restart skipped because installation is not running",
        );
        return Ok(());
    }
    let require_managed_frontend_dist = state
        .db
        .get_installation_detail(&installation.id)?
        .frontend_repo
        .is_some();
    let profile = effective_launch_profile(installation, require_managed_frontend_dist)?;
    log_operation(
        state,
        app,
        operation_id,
        "restart",
        "info",
        "restarting installation",
    );
    state.processes.restart(&installation.id, &profile).await?;
    Ok(())
}

async fn acquire_background_work_guard(
    state: &AppState,
) -> tokio::sync::OwnedMutexGuard<()> {
    state.background_work_lock().lock_owned().await
}

fn validate_frontend_restart_preconditions(
    frontend_settings: &FrontendSettings,
    is_running: bool,
    sync_dependencies: bool,
    restart_after_success: bool,
) -> AppResult<()> {
    if restart_after_success
        && is_running
        && !sync_dependencies
        && !Path::new(&frontend_settings.dist_path).exists()
    {
        return Err(AppError::Conflict(
            "restart_after_success on a running installation requires an existing managed frontend dist; enable dependency sync or build the frontend first".to_string(),
        ));
    }
    Ok(())
}

async fn apply_repo_target_update(
    state: &AppState,
    app: &AppHandle,
    operation_id: &str,
    installation: &Installation,
    repo: &ManagedRepo,
    target_input: &str,
    dirty_repo_strategy: &DirtyRepoStrategy,
    sync_dependencies: bool,
    write_tracked_target: bool,
) -> AppResult<RepoCheckpoint> {
    let tracked_state = build_requested_tracked_state_for_input(
        state,
        installation,
        repo,
        target_input,
        false,
    )
    .await?;
    apply_repo_tracking_state(
        state,
        app,
        operation_id,
        installation,
        repo,
        &tracked_state,
        dirty_repo_strategy,
        sync_dependencies,
        write_tracked_target,
    )
    .await
}

#[tauri::command]
async fn register_installation(
    app: AppHandle,
    state: State<'_, AppState>,
    input: RegisterInstallationInput,
) -> Result<RegisterInstallationResult, String> {
    let _ = app;
    register_installation_impl(&state, input)
        .await
        .map_err(|e| e.to_string())
}

async fn register_installation_impl(
    state: &AppState,
    input: RegisterInstallationInput,
) -> AppResult<RegisterInstallationResult> {
    let requested_root = PathBuf::from(&input.comfy_root);
    if !requested_root.exists() || !requested_root.is_dir() {
        return Err(AppError::InvalidInput(
            "ComfyUI root does not exist or is not a directory".to_string(),
        ));
    }
    let root = requested_root.canonicalize()?;
    let root_string = root.to_string_lossy().to_string();
    let existing_installation = state.db.get_installation_by_root(&root_string)?;
    let installation_lock = match existing_installation.as_ref() {
        Some(existing) => Some(state.installation_lock(&existing.id).await),
        None => None,
    };
    let _installation_guard = match installation_lock.as_ref() {
        Some(lock) => Some(lock.lock().await),
        None => None,
    };
    let existing_installation = match existing_installation {
        Some(existing) => state.db.get_installation(&existing.id)?,
        None => None,
    };

    let python = if input.python_exe.is_some() || existing_installation.is_none() {
        Some(normalize_python_executable(
            &root,
            input.python_exe.as_deref(),
        )?)
    } else {
        None
    };

    let custom_nodes_dir = root.join("custom_nodes");
    std::fs::create_dir_all(&custom_nodes_dir)?;

    let launch_profile = if input.launch_profile.is_some() || existing_installation.is_none() {
        let launch_profile = normalize_launch_profile(&root, input.launch_profile.clone())?;
        merge_launch_profile_overrides(
            &root,
            existing_installation
                .as_ref()
                .and_then(|installation| installation.launch_profile.as_ref()),
            launch_profile,
        )
    } else {
        None
    };
    let frontend_settings = if input.frontend_settings.is_some() || existing_installation.is_none() {
        normalize_frontend_settings(
            &root,
            &custom_nodes_dir,
            input.frontend_settings.clone(),
        )?
    } else {
        existing_installation
            .as_ref()
            .and_then(|installation| installation.frontend_settings.clone())
    };
    let detected_env_kind = python.as_ref().map(|value| detect_env_kind(value));
    let python_string = python
        .as_ref()
        .map(|value| value.to_string_lossy().to_string());
    let custom_nodes_dir_string = custom_nodes_dir.to_string_lossy().to_string();
    let is_root_git_repo = is_git_repo(&root).await;

    let installation = state.db.upsert_installation_by_root(
        &input.name,
        &root_string,
        python_string.as_deref(),
        &custom_nodes_dir_string,
        launch_profile.as_ref(),
        frontend_settings.as_ref(),
        detected_env_kind.as_deref(),
        is_root_git_repo,
    )?;
    let _repo_guards = if existing_installation.is_some() {
        acquire_installation_repo_locks(state, &installation.id).await?
    } else {
        Vec::new()
    };
    let (core_repo, frontend_repo, discovered_custom_nodes) =
        discover_repositories_for_installation(state, &installation, true).await?;
    Ok(RegisterInstallationResult {
        installation,
        core_repo: core_repo.map(|repo| repo.repo),
        frontend_repo: frontend_repo.map(|repo| repo.repo),
        discovered_custom_nodes: discovered_custom_nodes
            .into_iter()
            .map(|repo| repo.repo)
            .collect(),
        warnings: Vec::new(),
    })
}

#[tauri::command]
fn list_installations(state: State<'_, AppState>) -> Result<Vec<Installation>, String> {
    state.db.list_installations().map_err(|e| e.to_string())
}

async fn load_installation_detail_response(
    state: &AppState,
    installation_id: &str,
    mark_reconciled: bool,
) -> Result<InstallationDetail, String> {
    let mut detail = hydrate_installation_detail(state, installation_id)
        .await
        .map_err(|e| e.to_string())?;
    detail.last_reconciled_at = mark_reconciled.then(crate::util::now_rfc3339);
    detail.is_running = state
        .processes
        .is_running(installation_id)
        .await
        .map_err(|e| e.to_string())?;
    Ok(detail)
}

#[tauri::command]
async fn get_installation_detail(
    state: State<'_, AppState>,
    installation_id: String,
) -> Result<InstallationDetail, String> {
    load_installation_detail_response(&state, &installation_id, false).await
}

#[tauri::command]
async fn reconcile_installation(
    state: State<'_, AppState>,
    installation_id: String,
) -> Result<InstallationDetail, String> {
    load_installation_detail_response(&state, &installation_id, true).await
}

#[tauri::command]
async fn save_installation(
    state: State<'_, AppState>,
    input: SaveInstallationInput,
) -> Result<Installation, String> {
    let installation_lock = state.installation_lock(&input.installation_id).await;
    let _installation_guard = installation_lock.lock().await;
    let existing = state
        .db
        .get_installation(&input.installation_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "installation not found".to_string())?;
    let root = PathBuf::from(&existing.comfy_root);
    if !root.exists() || !root.is_dir() {
        return Err("installation root no longer exists".to_string());
    }
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    let (python_string, detected_env_kind) = if let Some(python_input) = input.python_exe.as_deref()
    {
        let python = normalize_python_executable(&root, Some(python_input))
            .map_err(|e| e.to_string())?;
        (
            python.to_string_lossy().to_string(),
            detect_env_kind(&python),
        )
    } else {
        (existing.python_exe.clone(), existing.detected_env_kind.clone())
    };
    let launch_profile = if input.launch_profile.is_some() {
        normalize_launch_profile(&root, input.launch_profile)
            .map_err(|e| e.to_string())?
    } else {
        existing.launch_profile.clone()
    };
    let custom_nodes_dir = PathBuf::from(&existing.custom_nodes_dir);
    let frontend_settings = if input.frontend_settings.is_some() {
        normalize_frontend_settings(&root, &custom_nodes_dir, input.frontend_settings)
            .map_err(|e| e.to_string())?
    } else {
        existing.frontend_settings.clone()
    };
    let installation = state
        .db
        .update_installation(
            &existing.id,
            &input.name,
            &python_string,
            launch_profile.as_ref(),
            frontend_settings.as_ref(),
            &detected_env_kind,
            is_git_repo(&root).await,
        )
        .map_err(|e| e.to_string())?;
    let _repo_guards = acquire_installation_repo_locks(&state, &installation.id)
        .await
        .map_err(|e| e.to_string())?;
    let _ = discover_repositories_for_installation(&state, &installation, true)
        .await
        .map_err(|e| e.to_string())?;
    let installation = state
        .db
        .get_installation(&installation.id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "installation not found after save".to_string())?;
    Ok(installation)
}

#[tauri::command]
async fn delete_installation(
    state: State<'_, AppState>,
    input: DeleteInstallationInput,
) -> Result<(), String> {
    let installation = state
        .db
        .get_installation(&input.installation_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "installation not found".to_string())?;
    let lock = state.installation_lock(&installation.id).await;
    let _guard = lock.lock().await;
    let _repo_guards = acquire_installation_repo_locks(&state, &installation.id)
        .await
        .map_err(|e| e.to_string())?;
    if let Some(profile) = installation.launch_profile.as_ref() {
        match state.processes.stop(&installation.id, profile).await {
            Ok(_) => {}
            Err(_) => {
                state
                    .processes
                    .force_stop(&installation.id)
                    .await
                    .map_err(|e| e.to_string())?;
            }
        }
    } else {
        state
            .processes
            .force_stop(&installation.id)
            .await
            .map_err(|e| e.to_string())?;
    }
    state
        .db
        .delete_installation(&installation.id)
        .map_err(|e| e.to_string())
}

fn collect_local_dir_matches(
    expected_dir_names: &[String],
    dirs_by_name: &HashMap<String, Vec<String>>,
) -> Vec<String> {
    let mut matches = Vec::new();
    for expected_dir_name in expected_dir_names {
        if let Some(paths) = dirs_by_name.get(&expected_dir_name.to_ascii_lowercase()) {
            for path in paths {
                if !matches.iter().any(|existing| existing == path) {
                    matches.push(path.clone());
                }
            }
        }
    }
    matches
}

async fn collect_manager_custom_node_items(
    state: &AppState,
    installation: &Installation,
    query: Option<&str>,
    limit: usize,
) -> AppResult<Vec<ManagerRegistryCustomNode>> {
    let discovered_custom_nodes = discover_custom_node_repos_best_effort(state, installation).await?;

    let mut tracking_managed_dirs: HashMap<String, Vec<String>> = HashMap::new();
    let mut present_non_git_dirs: HashMap<String, Vec<String>> = HashMap::new();
    let custom_nodes_dir = PathBuf::from(&installation.custom_nodes_dir);
    if custom_nodes_dir.exists() {
        match std::fs::read_dir(&custom_nodes_dir) {
            Ok(entries) => {
                for entry in entries {
                    let entry = match entry {
                        Ok(entry) => entry,
                        Err(err) => {
                            eprintln!(
                                "failed to read an entry under {} during registry browsing: {}",
                                custom_nodes_dir.to_string_lossy(),
                                err
                            );
                            continue;
                        }
                    };
                    let path = entry.path();
                    if !path.is_dir() {
                        continue;
                    }
                    let Some(dir_name) = path.file_name().and_then(|value| value.to_str()) else {
                        continue;
                    };
                    let has_tracking = path.join(".tracking").is_file();
                    let has_git = has_git_marker(&path);
                    let key = dir_name.to_ascii_lowercase();
                    if has_git && is_git_repo(&path).await {
                        continue;
                    }
                    let path_string = path.to_string_lossy().to_string();
                    if has_tracking {
                        tracking_managed_dirs.entry(key).or_default().push(path_string);
                    } else if !has_git {
                        present_non_git_dirs.entry(key).or_default().push(path_string);
                    }
                }
            }
            Err(err) => {
                eprintln!(
                    "failed to scan custom_nodes directory {} during registry browsing: {}",
                    custom_nodes_dir.to_string_lossy(),
                    err
                );
            }
        }
    }

    let mut installed_by_remote: HashMap<String, Option<ManagedRepo>> = HashMap::new();
    for repo in discovered_custom_nodes {
        if let Some(remote) = repo.canonical_remote.as_deref().and_then(canonicalize_remote) {
            let remote_aliases = state.manager_registry.remote_aliases(&remote).await;
            for alias in remote_aliases {
                match installed_by_remote.entry(alias) {
                    Entry::Vacant(slot) => {
                        slot.insert(Some(repo.clone()));
                    }
                    Entry::Occupied(mut slot) => {
                        let same_repo = slot
                            .get()
                            .as_ref()
                            .is_some_and(|existing| existing.id == repo.id);
                        if !same_repo {
                            slot.insert(None);
                        }
                    }
                }
            }
        }
    }

    let limit = limit.clamp(1, 10000);
    let entries = state.manager_registry.search_entries(query, usize::MAX).await?;
    let mut items = Vec::with_capacity(entries.len());
    for entry in entries {
        let install_type = entry.install_type_label();
        let canonical_repo_url = entry.canonical_git_remote();
        let is_installable = install_type == "git-clone" && canonical_repo_url.is_some();
        let source_input = if is_installable {
            canonical_repo_url.clone()
        } else {
            entry.source_input()
        };
        let installed_state = canonical_repo_url
            .as_ref()
            .and_then(|remote| installed_by_remote.get(remote));
        let expected_dir_names = state.manager_registry.expected_dir_names_for_entry(&entry);
        let tracking_local_matches =
            collect_local_dir_matches(&expected_dir_names, &tracking_managed_dirs);
        let present_local_matches =
            collect_local_dir_matches(&expected_dir_names, &present_non_git_dirs);
        let (tracking_local_path, has_ambiguous_tracking_local_path) =
            match tracking_local_matches.len() {
                0 => (None, false),
                1 => (tracking_local_matches.into_iter().next(), false),
                _ => (None, true),
            };
        let (present_local_path, has_ambiguous_present_local_path) =
            match present_local_matches.len() {
                0 => (None, false),
                1 => (present_local_matches.into_iter().next(), false),
                _ => (None, true),
            };
        let installed = installed_state.and_then(|state| state.as_ref());
        let has_ambiguous_installation = matches!(installed_state, Some(None))
            || has_ambiguous_tracking_local_path
            || has_ambiguous_present_local_path;
        items.push(ManagerRegistryCustomNode {
            registry_id: entry.registry_id(),
            title: entry.title(),
            author: entry.author(),
            description: entry.description(),
            install_type: install_type.clone(),
            source_input,
            canonical_repo_url,
            is_installable,
            is_installed: installed_state.is_some() || tracking_local_path.is_some(),
            is_tracking_managed: tracking_local_path.is_some(),
            tracking_local_path,
            is_present_non_git: present_local_path.is_some(),
            present_local_path,
            has_ambiguous_installation,
            installed_repo_id: installed.map(|repo| repo.id.clone()),
            installed_display_name: installed.map(|repo| repo.display_name.clone()),
            installed_local_path: installed.map(|repo| repo.local_path.clone()),
        });
    }

    items.sort_by(|left, right| {
        right
            .is_installed
            .cmp(&left.is_installed)
            .then_with(|| left.title.to_ascii_lowercase().cmp(&right.title.to_ascii_lowercase()))
            .then_with(|| left.registry_id.cmp(&right.registry_id))
    });
    items.truncate(limit);
    Ok(items)
}

#[tauri::command]
async fn list_manager_custom_nodes(
    state: State<'_, AppState>,
    input: ListManagerCustomNodesInput,
) -> Result<Vec<ManagerRegistryCustomNode>, String> {
    let installation = state
        .db
        .get_installation(&input.installation_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "installation not found".to_string())?;

    collect_manager_custom_node_items(
        &state,
        &installation,
        input.query.as_deref(),
        input.limit.unwrap_or(1000),
    )
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
async fn resolve_target(
    state: State<'_, AppState>,
    input: ResolveTargetInput,
) -> Result<ResolvedTarget, String> {
    let installation = state
        .db
        .get_installation(&input.installation_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "installation not found".to_string())?;
    resolve_target_for_context(
        &state,
        &installation,
        &input.kind,
        input.repo_id.as_deref(),
        &input.input,
    )
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
async fn preview_repo_target(
    state: State<'_, AppState>,
    input: PreviewRepoTargetInput,
) -> Result<RepoActionPreview, String> {
    let installation = state
        .db
        .get_installation(&input.installation_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "installation not found".to_string())?;
    if let Some(repo_id) = input.repo_id.as_deref() {
        let repo = state
            .db
            .get_repo(repo_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "repo not found".to_string())?;
        if repo.installation_id != installation.id {
            return Err("repo does not belong to the selected installation".to_string());
        }
        let tracked_state = build_requested_tracked_state_for_input(
            &state,
            &installation,
            &repo,
            &input.input,
            input.clear_overlays.unwrap_or(false),
        )
        .await
        .map_err(|e| e.to_string())?;
        let repo_lock = state.repo_lock(&repo.id).await;
        let _guard = repo_lock.lock().await;
        preview_tracked_state_application(
            &state,
            &installation,
            &repo,
            &tracked_state,
            "Preview target",
        )
        .await
        .map_err(|e| e.to_string())
    } else {
        preview_resolved_install_target(&state, &installation, input.kind, &input.input)
            .await
            .map_err(|e| e.to_string())
    }
}

#[tauri::command]
async fn preview_tracked_repo_update(
    state: State<'_, AppState>,
    repo_id: String,
) -> Result<RepoActionPreview, String> {
    let repo = state
        .db
        .get_repo(&repo_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "repo not found".to_string())?;
    let installation = state
        .db
        .get_installation(&repo.installation_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "installation not found".to_string())?;
    let tracked_state = load_repo_tracked_state(&state, &installation, &repo)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "repo has no tracked target".to_string())?;
    let repo_lock = state.repo_lock(&repo.id).await;
    let _guard = repo_lock.lock().await;
    preview_tracked_state_application(&state, &installation, &repo, &tracked_state, "Preview update")
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn patch_core(
    app: AppHandle,
    state: State<'_, AppState>,
    input: PatchCoreInput,
) -> Result<OperationStart, String> {
    let installation = state
        .db
        .get_installation(&input.installation_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "installation not found".to_string())?;
    let detail = state
        .db
        .get_installation_detail(&installation.id)
        .map_err(|e| e.to_string())?;
    let repo = detail
        .core_repo
        .ok_or_else(|| "core repository is not registered".to_string())?;
    let background_work_lock = state.background_work_lock();
    let _background_work_accept_guard = background_work_lock.lock().await;
    let op = state
        .db
        .create_operation(
            &installation.id,
            Some(&repo.id),
            OperationKind::PatchCore,
            Some(&input.input),
        )
        .map_err(|e| e.to_string())?;
    let op_id = op.id.clone();
    let state_handle = app.state::<AppState>().inner().clone();
    tauri::async_runtime::spawn(async move {
        let _ = run_patch_core(app, state_handle, installation, repo, input, op_id).await;
    });
    Ok(OperationStart {
        operation_id: op.id,
    })
}

async fn run_patch_core(
    app: AppHandle,
    state: AppState,
    installation: Installation,
    repo: ManagedRepo,
    input: PatchCoreInput,
    operation_id: String,
) -> AppResult<()> {
    let _background_work_guard = acquire_background_work_guard(&state).await;
    state.db.set_operation_running(&operation_id)?;
    let repo_lock = state.repo_lock(&repo.id).await;
    let _guard = repo_lock.lock().await;
    log_operation(
        &state,
        &app,
        &operation_id,
        "preflight",
        "info",
        "patching core repository",
    );
    let result = async {
        let checkpoint = apply_repo_target_update(
            &state,
            &app,
            &operation_id,
            &installation,
            &repo,
            &input.input,
            &input.dirty_repo_strategy,
            input.sync_dependencies,
            input.set_tracked_target,
        )
        .await?;
        maybe_restart_installation(
            &state,
            &app,
            &operation_id,
            &installation,
            input.restart_after_success,
        )
        .await?;
        state.db.finish_operation(
            &operation_id,
            OperationStatus::Succeeded,
            None,
            Some(&checkpoint.id),
        )?;
        log_operation(
            &state,
            &app,
            &operation_id,
            "done",
            "info",
            "core patch completed",
        );
        Ok::<(), AppError>(())
    }
    .await;
    if let Err(err) = result {
        state.db.finish_operation(
            &operation_id,
            OperationStatus::Failed,
            Some(&err.to_string()),
            None,
        )?;
        log_operation(
            &state,
            &app,
            &operation_id,
            "error",
            "error",
            err.to_string(),
        );
        return Err(err);
    }
    Ok(())
}

#[tauri::command]
async fn install_or_patch_frontend(
    app: AppHandle,
    state: State<'_, AppState>,
    input: PatchFrontendInput,
) -> Result<OperationStart, String> {
    let installation_lock = state.installation_lock(&input.installation_id).await;
    let _installation_guard = installation_lock.lock().await;
    let mut installation = state
        .db
        .get_installation(&input.installation_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "installation not found".to_string())?;
    let mut detail = state
        .db
        .get_installation_detail(&installation.id)
        .map_err(|e| e.to_string())?;
    let frontend_settings_missing = installation
        .frontend_settings
        .as_ref()
        .map_or(true, |settings| settings.repo_root.trim().is_empty());
    if frontend_settings_missing {
        let frontend_settings = default_frontend_settings_for_installation(
            &installation,
            detail
                .frontend_repo
                .as_ref()
                .map(|repo| Path::new(&repo.local_path)),
        )
        .map_err(|e| e.to_string())?;
        installation = state
            .db
            .update_installation(
                &installation.id,
                &installation.name,
                &installation.python_exe,
                installation.launch_profile.as_ref(),
                Some(&frontend_settings),
                &installation.detected_env_kind,
                installation.is_git_repo,
            )
            .map_err(|e| e.to_string())?;
        detail = state
            .db
            .get_installation_detail(&installation.id)
            .map_err(|e| e.to_string())?;
    }
    let repo_id = detail.frontend_repo.as_ref().map(|repo| repo.id.as_str());
    let operation_kind = if detail.frontend_repo.is_some() {
        OperationKind::PatchFrontend
    } else {
        OperationKind::InstallFrontend
    };
    let background_work_lock = state.background_work_lock();
    let _background_work_accept_guard = background_work_lock.lock().await;
    let op = state
        .db
        .create_operation(
            &installation.id,
            repo_id,
            operation_kind,
            Some(&input.input),
        )
        .map_err(|e| e.to_string())?;
    let op_id = op.id.clone();
    let state_handle = app.state::<AppState>().inner().clone();
    tauri::async_runtime::spawn(async move {
        let _ = run_install_or_patch_frontend(app, state_handle, installation, input, op_id).await;
    });
    Ok(OperationStart {
        operation_id: op.id,
    })
}

async fn run_install_or_patch_frontend(
    app: AppHandle,
    state: AppState,
    installation: Installation,
    input: PatchFrontendInput,
    operation_id: String,
) -> AppResult<()> {
    let _background_work_guard = acquire_background_work_guard(&state).await;
    state.db.set_operation_running(&operation_id)?;
    let installation_lock = state.installation_lock(&installation.id).await;
    let _installation_guard = installation_lock.lock().await;
    log_operation(
        &state,
        &app,
        &operation_id,
        "preflight",
        "info",
        "installing or patching managed frontend",
    );
    let result = async {
        let restore_replaced_path = |backup_path: &Path, target_path: &Path| -> AppResult<()> {
            if target_path.exists() {
                if target_path.is_dir() {
                    std::fs::remove_dir_all(target_path)?;
                } else {
                    std::fs::remove_file(target_path)?;
                }
            }
            std::fs::rename(backup_path, target_path)?;
            Ok(())
        };
        let frontend_settings = installation.frontend_settings.as_ref().ok_or_else(|| {
            AppError::InvalidInput(
                "configure a managed frontend repo root in installation settings first"
                    .to_string(),
            )
        })?;
        let target_path = PathBuf::from(&frontend_settings.repo_root);
        let detail = state.db.get_installation_detail(&installation.id)?;
        validate_frontend_restart_preconditions(
            frontend_settings,
            state.processes.is_running(&installation.id).await?,
            input.sync_dependencies,
            input.restart_after_success,
        )?;
        let mut replaced_backup_path: Option<PathBuf> = None;
        let mut rollback_repo_id: Option<String> = None;
        let mut rollback_checkpoint_id: Option<String> = None;

        let result = async {
            let resolved = resolve_target_for_context(
                &state,
                &installation,
                &RepoKind::Frontend,
                detail.frontend_repo.as_ref().map(|repo| repo.id.as_str()),
                &input.input,
            )
            .await?;
            let resolved_remote = canonical_target_remote(&resolved)?;

            if target_path.exists() {
                let mut existing_git_repo = false;
                if is_git_repo(&target_path).await {
                    let status = inspect_repo(&target_path).await?;
                    let existing_remote = status.origin_url.as_deref().and_then(canonicalize_remote);
                    if existing_remote.as_deref() == Some(resolved_remote.as_str()) {
                        log_operation(
                            &state,
                            &app,
                            &operation_id,
                            "preflight",
                            "info",
                            format!(
                                "reusing managed frontend directory {}",
                                target_path.to_string_lossy()
                            ),
                        );
                        state.db.unignore_repo_path(
                            &installation.id,
                            &target_path.to_string_lossy(),
                        )?;
                        let repo = state.db.upsert_repo(
                            &installation.id,
                            RepoKind::Frontend,
                            "ComfyUI Frontend",
                            &target_path.to_string_lossy(),
                            status.origin_url.as_deref(),
                            status.head_sha.as_deref(),
                            status.branch.as_deref(),
                            status.is_detached,
                            repo_has_tracked_local_changes(&status),
                        )?;
                        let repo_lock = state.repo_lock(&repo.id).await;
                        let _guard = repo_lock.lock().await;
                        let tracked_state = build_requested_tracked_state_for_input(
                            &state,
                            &installation,
                            &repo,
                            &input.input,
                            false,
                        )
                        .await?;
                        let checkpoint = apply_repo_tracking_state(
                            &state,
                            &app,
                            &operation_id,
                            &installation,
                            &repo,
                            &tracked_state,
                            &input.dirty_repo_strategy,
                            input.sync_dependencies,
                            input.set_tracked_target,
                        )
                        .await?;
                        maybe_restart_installation(
                            &state,
                            &app,
                            &operation_id,
                            &installation,
                            input.restart_after_success,
                        )
                        .await?;
                        state.db.finish_operation(
                            &operation_id,
                            OperationStatus::Succeeded,
                            None,
                            Some(&checkpoint.id),
                        )?;
                        log_operation(
                            &state,
                            &app,
                            &operation_id,
                            "done",
                            "info",
                            "frontend patch completed",
                        );
                        return Ok::<(), AppError>(());
                    }
                    existing_git_repo = true;
                }

                match input.existing_repo_conflict_strategy {
                    ExistingRepoConflictStrategy::Abort => {
                        return Err(AppError::Conflict(
                            if existing_git_repo {
                                "managed frontend repo root already contains a different git repository"
                                    .to_string()
                            } else {
                                "managed frontend repo root already exists and is not a git repository"
                                    .to_string()
                            },
                        ));
                    }
                    ExistingRepoConflictStrategy::InstallWithSuffix => {
                        return Err(AppError::Conflict(
                            "managed frontend repo root is fixed; choose Replace or configure a different frontend repo root in installation settings"
                                .to_string(),
                        ));
                    }
                    ExistingRepoConflictStrategy::Replace => {
                        let backup_root = installation_retained_backup_root(&installation, "frontend");
                        std::fs::create_dir_all(&backup_root)?;
                        let backup_path = choose_tracking_backup_path(&target_path, &backup_root);
                        log_operation(
                            &state,
                            &app,
                            &operation_id,
                            "preflight",
                            "info",
                            format!(
                                "creating retained backup of existing frontend path {} at {} before replacement",
                                target_path.to_string_lossy(),
                                backup_path.to_string_lossy()
                            ),
                        );
                        std::fs::rename(&target_path, &backup_path)?;
                        replaced_backup_path = Some(backup_path);
                    }
                }
            } else {
                log_operation(
                    &state,
                    &app,
                    &operation_id,
                    "preflight",
                    "info",
                    format!(
                        "selected managed frontend directory {}",
                        target_path.to_string_lossy()
                    ),
                );
            }

            log_operation(
                &state,
                &app,
                &operation_id,
                "clone",
                "info",
                format!("cloning {}", resolved.canonical_repo_url),
            );
            clone_repo(&resolved.fetch_url, &target_path).await?;
            let status = inspect_repo(&target_path).await?;
            state.db.unignore_repo_path(
                &installation.id,
                &target_path.to_string_lossy(),
            )?;
            let repo = state.db.upsert_repo(
                &installation.id,
                RepoKind::Frontend,
                "ComfyUI Frontend",
                &target_path.to_string_lossy(),
                status.origin_url.as_deref(),
                status.head_sha.as_deref(),
                status.branch.as_deref(),
                status.is_detached,
                repo_has_tracked_local_changes(&status),
            )?;
            if replaced_backup_path.is_some() {
                rollback_repo_id = Some(repo.id.clone());
            }
            let repo_lock = state.repo_lock(&repo.id).await;
            let _guard = repo_lock.lock().await;
            let tracked_state = build_requested_tracked_state_for_input(
                &state,
                &installation,
                &repo,
                &input.input,
                false,
            )
            .await?;
            let checkpoint = apply_repo_tracking_state(
                &state,
                &app,
                &operation_id,
                &installation,
                &repo,
                &tracked_state,
                &DirtyRepoStrategy::Abort,
                input.sync_dependencies,
                input.set_tracked_target,
            )
            .await?;
            if replaced_backup_path.is_some() {
                rollback_checkpoint_id = Some(checkpoint.id.clone());
            }
            maybe_restart_installation(
                &state,
                &app,
                &operation_id,
                &installation,
                input.restart_after_success,
            )
            .await?;
            state.db.finish_operation(
                &operation_id,
                OperationStatus::Succeeded,
                None,
                Some(&checkpoint.id),
            )?;
            log_operation(
                &state,
                &app,
                &operation_id,
                "done",
                "info",
                "frontend install completed",
            );
            if let Some(backup_path) = replaced_backup_path.as_ref() {
                log_operation(
                    &state,
                    &app,
                    &operation_id,
                    "done",
                    "info",
                    format!(
                        "replaced frontend source was preserved at {}. Review and manually migrate any local data from that backup before deleting it.",
                        backup_path.to_string_lossy()
                    ),
                );
            }
            Ok::<(), AppError>(())
        }
        .await;

        if let Err(err) = result {
            if let Some(backup_path) = replaced_backup_path.as_ref() {
                if let Err(restore_err) = restore_replaced_path(backup_path, &target_path) {
                    log_operation(
                        &state,
                        &app,
                        &operation_id,
                        "error",
                        "error",
                        format!(
                            "failed to restore frontend backup {} after install error: {}",
                            backup_path.to_string_lossy(),
                            restore_err
                        ),
                    );
                } else {
                    log_operation(
                        &state,
                        &app,
                        &operation_id,
                        "preflight",
                        "warn",
                        format!(
                            "restored frontend backup {} after install error",
                            backup_path.to_string_lossy()
                        ),
                    );
                }
            }
            if let Some(checkpoint_id) = rollback_checkpoint_id.as_ref() {
                if let Err(db_err) = state.db.delete_checkpoint(checkpoint_id) {
                    log_operation(
                        &state,
                        &app,
                        &operation_id,
                        "error",
                        "error",
                        format!(
                            "failed to delete rollback checkpoint {}: {}",
                            checkpoint_id, db_err
                        ),
                    );
                }
            }
            if let Some(repo_id) = rollback_repo_id.as_ref() {
                if let Err(db_err) = state.db.delete_repo(repo_id) {
                    log_operation(
                        &state,
                        &app,
                        &operation_id,
                        "error",
                        "error",
                        format!("failed to delete rollback repo {}: {}", repo_id, db_err),
                    );
                }
            }
            return Err(err);
        }

        Ok::<(), AppError>(())
    }
    .await;

    if let Err(err) = result {
        state.db.finish_operation(
            &operation_id,
            OperationStatus::Failed,
            Some(&err.to_string()),
            None,
        )?;
        log_operation(
            &state,
            &app,
            &operation_id,
            "error",
            "error",
            err.to_string(),
        );
        return Err(err);
    }
    Ok(())
}

struct CustomNodeInstallRunResult {
    checkpoint_id: String,
    done_message: &'static str,
}

async fn execute_install_or_patch_custom_node(
    app: &AppHandle,
    state: &AppState,
    installation: &Installation,
    input: &PatchCustomNodeInput,
    operation_id: &str,
) -> AppResult<CustomNodeInstallRunResult> {
    let restore_tracking_backup = |backup_path: &Path, target_path: &Path| -> AppResult<()> {
        if target_path.exists() {
            std::fs::remove_dir_all(target_path)?;
        }
        std::fs::rename(backup_path, target_path)?;
        Ok(())
    };
    let mut tracking_backup_path: Option<PathBuf> = None;
    let mut tracking_target_path: Option<PathBuf> = None;
    let mut rollback_repo_id: Option<String> = None;
    let mut rollback_checkpoint_id: Option<String> = None;
    let result = async {
        let resolved = resolve_target_for_context(
            state,
            installation,
            &RepoKind::CustomNode,
            None,
            &input.input,
        )
        .await?;
        let custom_nodes_dir = PathBuf::from(&installation.custom_nodes_dir);
        let existing_repo_match =
            find_existing_custom_node_repo_by_remote(state, installation, &resolved).await?;
        if let (Some(requested), Some(repo)) = (
            input
                .target_local_dir_name
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty()),
            existing_repo_match.as_ref(),
        ) {
            let existing_name = Path::new(&repo.local_path)
                .file_name()
                .and_then(|value| value.to_str());
            if existing_name != Some(requested) {
                return Err(AppError::Conflict(format!(
                    "matching repo already exists at {}; refusing to ignore requested directory {}",
                    repo.local_path, requested
                )));
            }
        }
        let requested_dir_name = match input
            .target_local_dir_name
            .clone()
            .filter(|value| !value.trim().is_empty())
        {
            Some(value) => value,
            None => match existing_repo_match.as_ref() {
                Some(repo) => Path::new(&repo.local_path)
                    .file_name()
                    .and_then(|value| value.to_str())
                    .unwrap_or(&resolved.suggested_local_dir_name)
                    .to_string(),
                None => preferred_custom_node_dir_name(state, &resolved).await,
            },
        };
        let base_dir_name = validate_custom_node_dir_name(&requested_dir_name)?;
        let mut target_path = existing_repo_match
            .as_ref()
            .map(|repo| PathBuf::from(&repo.local_path))
            .unwrap_or_else(|| join_custom_node_path(&custom_nodes_dir, &base_dir_name));

        if let Some(repo) = existing_repo_match.as_ref() {
            log_operation(
                state,
                app,
                operation_id,
                "preflight",
                "info",
                format!("reusing existing custom node directory {}", repo.local_path),
            );
        } else {
            log_operation(
                state,
                app,
                operation_id,
                "preflight",
                "info",
                format!("selected custom node directory {}", target_path.to_string_lossy()),
            );
        }

        if target_path.exists() {
            if is_git_repo(&target_path).await {
                let status = inspect_repo(&target_path).await?;
                ensure_remote_matches(status.origin_url.as_deref(), &resolved)?;
                state.db.unignore_repo_path(
                    &installation.id,
                    &target_path.to_string_lossy(),
                )?;
                let repo = state.db.upsert_repo(
                    &installation.id,
                    RepoKind::CustomNode,
                    target_path
                        .file_name()
                        .and_then(|v| v.to_str())
                        .unwrap_or("custom-node"),
                    &target_path.to_string_lossy(),
                    status.origin_url.as_deref(),
                    status.head_sha.as_deref(),
                    status.branch.as_deref(),
                    status.is_detached,
                    repo_has_tracked_local_changes(&status),
                )?;
                let repo_lock = state.repo_lock(&repo.id).await;
                let _guard = repo_lock.lock().await;
                let tracked_state = build_requested_tracked_state_for_input(
                    state,
                    installation,
                    &repo,
                    &input.input,
                    false,
                )
                .await?;
                let checkpoint = apply_repo_tracking_state(
                    state,
                    app,
                    operation_id,
                    installation,
                    &repo,
                    &tracked_state,
                    &input.dirty_repo_strategy,
                    input.sync_dependencies,
                    input.set_tracked_target,
                )
                .await?;
                maybe_restart_installation(
                    state,
                    app,
                    operation_id,
                    installation,
                    input.restart_after_success,
                )
                .await?;
                return Ok::<CustomNodeInstallRunResult, AppError>(CustomNodeInstallRunResult {
                    checkpoint_id: checkpoint.id.clone(),
                    done_message: "custom node patch completed",
                });
            }

            if is_tracking_managed_dir(&target_path).await {
                if !input.adopt_tracking_install {
                    return Err(AppError::Conflict(
                        "target custom node directory is a tracking-managed install; enable tracking adoption to replace it with a managed git clone"
                            .to_string(),
                    ));
                }
                let backup_root = installation_retained_backup_root(installation, "custom_nodes");
                std::fs::create_dir_all(&backup_root)?;
                let backup_path = choose_tracking_backup_path(&target_path, &backup_root);
                log_operation(
                    state,
                    app,
                    operation_id,
                    "preflight",
                    "info",
                    format!(
                        "creating retained backup of tracking-managed install {} at {} before adopting it as a git repo",
                        target_path.to_string_lossy(),
                        backup_path.to_string_lossy()
                    ),
                );
                std::fs::rename(&target_path, &backup_path)?;
                tracking_target_path = Some(target_path.clone());
                tracking_backup_path = Some(backup_path);
            } else {
                match input.existing_repo_conflict_strategy {
                    ExistingRepoConflictStrategy::Abort => {
                        return Err(AppError::Conflict(
                            "target custom node directory already exists and is not a git repo"
                                .to_string(),
                        ));
                    }
                    ExistingRepoConflictStrategy::Replace => {
                        std::fs::remove_dir_all(&target_path)?;
                    }
                    ExistingRepoConflictStrategy::InstallWithSuffix => {
                        let mut idx = 2usize;
                        loop {
                            let candidate = join_custom_node_path(
                                &custom_nodes_dir,
                                &format!("{base_dir_name}-{idx}"),
                            );
                            if !candidate.exists() {
                                target_path = candidate;
                                break;
                            }
                            idx += 1;
                        }
                    }
                }
            }
        }

        log_operation(
            state,
            app,
            operation_id,
            "clone",
            "info",
            format!("cloning {}", resolved.canonical_repo_url),
        );
        clone_repo(&resolved.fetch_url, &target_path).await?;
        let status = inspect_repo(&target_path).await?;
        state.db.unignore_repo_path(
            &installation.id,
            &target_path.to_string_lossy(),
        )?;
        let repo = state.db.upsert_repo(
            &installation.id,
            RepoKind::CustomNode,
            target_path
                .file_name()
                .and_then(|v| v.to_str())
                .unwrap_or("custom-node"),
            &target_path.to_string_lossy(),
            status.origin_url.as_deref(),
            status.head_sha.as_deref(),
            status.branch.as_deref(),
            status.is_detached,
            repo_has_tracked_local_changes(&status),
        )?;
        if tracking_backup_path.is_some() {
            rollback_repo_id = Some(repo.id.clone());
        }
        let repo_lock = state.repo_lock(&repo.id).await;
        let _guard = repo_lock.lock().await;
        let tracked_state = build_requested_tracked_state_for_input(
            state,
            installation,
            &repo,
            &input.input,
            false,
        )
        .await?;
        let checkpoint = apply_repo_tracking_state(
            state,
            app,
            operation_id,
            installation,
            &repo,
            &tracked_state,
            &DirtyRepoStrategy::Abort,
            input.sync_dependencies,
            input.set_tracked_target,
        )
        .await?;
        if tracking_backup_path.is_some() {
            rollback_checkpoint_id = Some(checkpoint.id.clone());
        }
        maybe_restart_installation(
            state,
            app,
            operation_id,
            installation,
            input.restart_after_success,
        )
        .await?;
        if let Some(backup_path) = tracking_backup_path.as_ref() {
            log_operation(
                state,
                app,
                operation_id,
                "done",
                "info",
                format!(
                    "tracking-managed source was preserved at {}. Review and manually migrate any user data from that backup before deleting it.",
                    backup_path.to_string_lossy()
                ),
            );
        }
        Ok::<CustomNodeInstallRunResult, AppError>(CustomNodeInstallRunResult {
            checkpoint_id: checkpoint.id.clone(),
            done_message: "custom node install completed",
        })
    }
    .await;

    match result {
        Ok(outcome) => Ok(outcome),
        Err(err) => {
            if let (Some(backup_path), Some(target_path)) =
                (tracking_backup_path.as_ref(), tracking_target_path.as_ref())
            {
                if let Err(restore_err) = restore_tracking_backup(backup_path, target_path) {
                    log_operation(
                        state,
                        app,
                        operation_id,
                        "error",
                        "error",
                        format!(
                            "failed to restore tracking-managed backup {} after install error: {}",
                            backup_path.to_string_lossy(),
                            restore_err
                        ),
                    );
                } else {
                    log_operation(
                        state,
                        app,
                        operation_id,
                        "preflight",
                        "warn",
                        format!(
                            "restored tracking-managed backup {} after install error",
                            backup_path.to_string_lossy()
                        ),
                    );
                }
            }
            if let Some(checkpoint_id) = rollback_checkpoint_id.as_ref() {
                if let Err(db_err) = state.db.delete_checkpoint(checkpoint_id) {
                    log_operation(
                        state,
                        app,
                        operation_id,
                        "error",
                        "error",
                        format!(
                            "failed to delete rollback checkpoint {}: {}",
                            checkpoint_id, db_err
                        ),
                    );
                }
            }
            if let Some(repo_id) = rollback_repo_id.as_ref() {
                if let Err(db_err) = state.db.delete_repo(repo_id) {
                    log_operation(
                        state,
                        app,
                        operation_id,
                        "error",
                        "error",
                        format!("failed to delete rollback repo {}: {}", repo_id, db_err),
                    );
                }
            }
            Err(err)
        }
    }
}

#[tauri::command]
async fn install_or_patch_custom_node(
    app: AppHandle,
    state: State<'_, AppState>,
    input: PatchCustomNodeInput,
) -> Result<OperationStart, String> {
    let installation = state
        .db
        .get_installation(&input.installation_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "installation not found".to_string())?;
    let background_work_lock = state.background_work_lock();
    let _background_work_accept_guard = background_work_lock.lock().await;
    let op = state
        .db
        .create_operation(
            &installation.id,
            None,
            OperationKind::InstallCustomNode,
            Some(&input.input),
        )
        .map_err(|e| e.to_string())?;
    let op_id = op.id.clone();
    let state_handle = app.state::<AppState>().inner().clone();
    tauri::async_runtime::spawn(async move {
        let _ =
            run_install_or_patch_custom_node(app, state_handle, installation, input, op_id).await;
    });
    Ok(OperationStart {
        operation_id: op.id,
    })
}

async fn run_install_or_patch_custom_node(
    app: AppHandle,
    state: AppState,
    installation: Installation,
    input: PatchCustomNodeInput,
    operation_id: String,
) -> AppResult<()> {
    let _background_work_guard = acquire_background_work_guard(&state).await;
    state.db.set_operation_running(&operation_id)?;
    let installation_lock = state.installation_lock(&installation.id).await;
    let _installation_guard = installation_lock.lock().await;
    log_operation(
        &state,
        &app,
        &operation_id,
        "preflight",
        "info",
        "installing or patching custom node",
    );

    match execute_install_or_patch_custom_node(&app, &state, &installation, &input, &operation_id).await {
        Ok(outcome) => {
            state.db.finish_operation(
                &operation_id,
                OperationStatus::Succeeded,
                None,
                Some(&outcome.checkpoint_id),
            )?;
            log_operation(
                &state,
                &app,
                &operation_id,
                "done",
                "info",
                outcome.done_message,
            );
            Ok(())
        }
        Err(err) => {
            state.db.finish_operation(
                &operation_id,
                OperationStatus::Failed,
                Some(&err.to_string()),
                None,
            )?;
            log_operation(
                &state,
                &app,
                &operation_id,
                "error",
                "error",
                err.to_string(),
            );
            Err(err)
        }
    }
}

#[tauri::command]
async fn adopt_tracked_custom_nodes(
    app: AppHandle,
    state: State<'_, AppState>,
    input: AdoptTrackedCustomNodesInput,
) -> Result<OperationStart, String> {
    let installation = state
        .db
        .get_installation(&input.installation_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "installation not found".to_string())?;
    let background_work_lock = state.background_work_lock();
    let _background_work_accept_guard = background_work_lock.lock().await;
    let op = state
        .db
        .create_operation(
            &installation.id,
            None,
            OperationKind::InstallCustomNode,
            Some("adopt all tracked installs"),
        )
        .map_err(|e| e.to_string())?;
    let op_id = op.id.clone();
    let state_handle = app.state::<AppState>().inner().clone();
    tauri::async_runtime::spawn(async move {
        let _ = run_adopt_tracked_custom_nodes(app, state_handle, installation, op_id).await;
    });
    Ok(OperationStart {
        operation_id: op.id,
    })
}

async fn run_adopt_tracked_custom_nodes(
    app: AppHandle,
    state: AppState,
    installation: Installation,
    operation_id: String,
) -> AppResult<()> {
    let _background_work_guard = acquire_background_work_guard(&state).await;
    state.db.set_operation_running(&operation_id)?;
    let installation_lock = state.installation_lock(&installation.id).await;
    let _installation_guard = installation_lock.lock().await;
    log_operation(
        &state,
        &app,
        &operation_id,
        "preflight",
        "info",
        "adopting all tracked custom node installs",
    );

    let result = async {
        let items = collect_manager_custom_node_items(&state, &installation, None, 10_000).await?;
        let mut candidates_by_path: HashMap<String, Vec<ManagerRegistryCustomNode>> = HashMap::new();
        for entry in items {
            let has_git_install = entry.installed_repo_id.is_some() || entry.installed_local_path.is_some();
            let source_input = entry
                .source_input
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty());
            let tracking_local_path = entry
                .tracking_local_path
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty());
            if !entry.is_tracking_managed
                || has_git_install
                || !entry.is_installable
                || entry.has_ambiguous_installation
                || source_input.is_none()
                || tracking_local_path.is_none()
            {
                continue;
            }
            candidates_by_path
                .entry(tracking_local_path.unwrap().to_ascii_lowercase())
                .or_default()
                .push(entry);
        }

        let mut ambiguous_path_count = 0usize;
        let mut candidates = Vec::new();
        for mut entries in candidates_by_path.into_values() {
            if entries.len() > 1 {
                ambiguous_path_count += 1;
                let tracking_local_path = entries
                    .first()
                    .and_then(|entry| entry.tracking_local_path.as_deref())
                    .unwrap_or("unknown path")
                    .to_string();
                let titles = entries
                    .iter()
                    .map(|entry| entry.title.clone())
                    .collect::<Vec<_>>()
                    .join(", ");
                log_operation(
                    &state,
                    &app,
                    &operation_id,
                    "preflight",
                    "warn",
                    format!(
                        "skipping tracking-managed install at {} because multiple registry entries matched it: {}",
                        tracking_local_path, titles
                    ),
                );
                continue;
            }
            if let Some(entry) = entries.pop() {
                candidates.push(entry);
            }
        }

        candidates.sort_by(|left, right| {
            left.title
                .to_ascii_lowercase()
                .cmp(&right.title.to_ascii_lowercase())
                .then_with(|| left.registry_id.cmp(&right.registry_id))
        });

        if candidates.is_empty() {
            if ambiguous_path_count == 0 {
                return Ok::<String, AppError>("no adoptable tracked installs found".to_string());
            }
            return Err(AppError::Conflict(format!(
                "found no uniquely mappable tracked installs to adopt; skipped {} ambiguous path(s)",
                ambiguous_path_count
            )));
        }

        let total = candidates.len();
        let mut adopted_count = 0usize;
        let mut failure_count = 0usize;

        for (index, entry) in candidates.into_iter().enumerate() {
            let source_input = match entry
                .source_input
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                Some(value) => value.to_string(),
                None => {
                    failure_count += 1;
                    log_operation(
                        &state,
                        &app,
                        &operation_id,
                        "preflight",
                        "warn",
                        format!(
                            "skipping tracked install {} because no source URL was available",
                            entry.title
                        ),
                    );
                    continue;
                }
            };
            let target_local_dir_name = match entry
                .tracking_local_path
                .as_deref()
                .and_then(|value| Path::new(value).file_name().and_then(|name| name.to_str()))
                .map(str::to_string)
            {
                Some(value) => value,
                None => {
                    failure_count += 1;
                    log_operation(
                        &state,
                        &app,
                        &operation_id,
                        "preflight",
                        "warn",
                        format!(
                            "skipping tracked install {} because its local directory name could not be determined",
                            entry.title
                        ),
                    );
                    continue;
                }
            };

            log_operation(
                &state,
                &app,
                &operation_id,
                "preflight",
                "info",
                format!(
                    "adopting tracked install {}/{}: {}",
                    index + 1,
                    total,
                    entry.title
                ),
            );

            let patch_input = PatchCustomNodeInput {
                installation_id: installation.id.clone(),
                input: source_input,
                target_local_dir_name: Some(target_local_dir_name),
                existing_repo_conflict_strategy: ExistingRepoConflictStrategy::InstallWithSuffix,
                dirty_repo_strategy: DirtyRepoStrategy::Abort,
                set_tracked_target: true,
                sync_dependencies: true,
                restart_after_success: false,
                adopt_tracking_install: true,
            };

            match execute_install_or_patch_custom_node(
                &app,
                &state,
                &installation,
                &patch_input,
                &operation_id,
            )
            .await
            {
                Ok(outcome) => {
                    adopted_count += 1;
                    log_operation(
                        &state,
                        &app,
                        &operation_id,
                        "preflight",
                        "info",
                        format!("{} ({})", outcome.done_message, entry.title),
                    );
                }
                Err(err) => {
                    failure_count += 1;
                    log_operation(
                        &state,
                        &app,
                        &operation_id,
                        "preflight",
                        "warn",
                        format!("failed to adopt tracked install {}: {}", entry.title, err),
                    );
                }
            }
        }

        let mut summary = format!("adopted {} of {} tracked install(s)", adopted_count, total);
        if ambiguous_path_count > 0 {
            summary.push_str(&format!("; skipped {} ambiguous path(s)", ambiguous_path_count));
        }
        if failure_count > 0 {
            summary.push_str(&format!("; {} failed", failure_count));
            return Err(AppError::Conflict(summary));
        }
        Ok::<String, AppError>(summary)
    }
    .await;

    match result {
        Ok(summary) => {
            state
                .db
                .finish_operation(&operation_id, OperationStatus::Succeeded, None, None)?;
            log_operation(&state, &app, &operation_id, "done", "info", summary);
            Ok(())
        }
        Err(err) => {
            state.db.finish_operation(
                &operation_id,
                OperationStatus::Failed,
                Some(&err.to_string()),
                None,
            )?;
            log_operation(
                &state,
                &app,
                &operation_id,
                "error",
                "error",
                err.to_string(),
            );
            Err(err)
        }
    }
}

#[tauri::command]
async fn set_repo_base_target(
    app: AppHandle,
    state: State<'_, AppState>,
    input: SetRepoBaseTargetInput,
) -> Result<OperationStart, String> {
    let repo = state
        .db
        .get_repo(&input.repo_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "repo not found".to_string())?;
    let installation = state
        .db
        .get_installation(&repo.installation_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "installation not found".to_string())?;
    let background_work_lock = state.background_work_lock();
    let _background_work_accept_guard = background_work_lock.lock().await;
    let op = state
        .db
        .create_operation(
            &installation.id,
            Some(&repo.id),
            OperationKind::ManageRepoStack,
            Some(&input.input),
        )
        .map_err(|e| e.to_string())?;
    let op_id = op.id.clone();
    let state_handle = app.state::<AppState>().inner().clone();
    tauri::async_runtime::spawn(async move {
        let _ = run_set_repo_base_target(app, state_handle, installation, repo, input, op_id).await;
    });
    Ok(OperationStart {
        operation_id: op.id,
    })
}

async fn run_set_repo_base_target(
    app: AppHandle,
    state: AppState,
    installation: Installation,
    repo: ManagedRepo,
    input: SetRepoBaseTargetInput,
    operation_id: String,
) -> AppResult<()> {
    let _background_work_guard = acquire_background_work_guard(&state).await;
    state.db.set_operation_running(&operation_id)?;
    let repo_lock = state.repo_lock(&repo.id).await;
    let _guard = repo_lock.lock().await;
    log_operation(
        &state,
        &app,
        &operation_id,
        "preflight",
        "info",
        stack_operation_summary("setting repo base target", &repo),
    );
    let result = async {
        let resolved = resolve_target_for_context(
            &state,
            &installation,
            &repo.kind,
            Some(&repo.id),
            &input.input,
        )
        .await?;
        ensure_remote_matches(repo.canonical_remote.as_deref(), &resolved)?;
        let tracked_state = build_base_tracked_state_for_input(
            &state,
            &installation,
            &repo,
            &resolved,
            input.clear_overlays,
        )
        .await?;
        let checkpoint = apply_repo_tracking_state(
            &state,
            &app,
            &operation_id,
            &installation,
            &repo,
            &tracked_state,
            &input.dirty_repo_strategy,
            input.sync_dependencies,
            true,
        )
        .await?;
        state.db.finish_operation(
            &operation_id,
            OperationStatus::Succeeded,
            None,
            Some(&checkpoint.id),
        )?;
        log_operation(
            &state,
            &app,
            &operation_id,
            "done",
            "info",
            "repo base target updated",
        );
        Ok::<(), AppError>(())
    }
    .await;
    if let Err(err) = result {
        state.db.finish_operation(
            &operation_id,
            OperationStatus::Failed,
            Some(&err.to_string()),
            None,
        )?;
        log_operation(
            &state,
            &app,
            &operation_id,
            "error",
            "error",
            err.to_string(),
        );
        return Err(err);
    }
    Ok(())
}

#[tauri::command]
async fn add_repo_overlay(
    app: AppHandle,
    state: State<'_, AppState>,
    input: AddRepoOverlayInput,
) -> Result<OperationStart, String> {
    let repo = state
        .db
        .get_repo(&input.repo_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "repo not found".to_string())?;
    let installation = state
        .db
        .get_installation(&repo.installation_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "installation not found".to_string())?;
    let background_work_lock = state.background_work_lock();
    let _background_work_accept_guard = background_work_lock.lock().await;
    let op = state
        .db
        .create_operation(
            &installation.id,
            Some(&repo.id),
            OperationKind::ManageRepoStack,
            Some(&input.input),
        )
        .map_err(|e| e.to_string())?;
    let op_id = op.id.clone();
    let state_handle = app.state::<AppState>().inner().clone();
    tauri::async_runtime::spawn(async move {
        let _ = run_add_repo_overlay(app, state_handle, installation, repo, input, op_id).await;
    });
    Ok(OperationStart {
        operation_id: op.id,
    })
}

async fn run_add_repo_overlay(
    app: AppHandle,
    state: AppState,
    installation: Installation,
    repo: ManagedRepo,
    input: AddRepoOverlayInput,
    operation_id: String,
) -> AppResult<()> {
    let _background_work_guard = acquire_background_work_guard(&state).await;
    state.db.set_operation_running(&operation_id)?;
    let repo_lock = state.repo_lock(&repo.id).await;
    let _guard = repo_lock.lock().await;
    log_operation(
        &state,
        &app,
        &operation_id,
        "preflight",
        "info",
        stack_operation_summary("adding repo overlay", &repo),
    );
    let result = async {
        let tracked_state = build_requested_tracked_state_for_input(
            &state,
            &installation,
            &repo,
            &input.input,
            false,
        )
        .await?;
        let checkpoint = apply_repo_tracking_state(
            &state,
            &app,
            &operation_id,
            &installation,
            &repo,
            &tracked_state,
            &input.dirty_repo_strategy,
            input.sync_dependencies,
            true,
        )
        .await?;
        state.db.finish_operation(
            &operation_id,
            OperationStatus::Succeeded,
            None,
            Some(&checkpoint.id),
        )?;
        log_operation(
            &state,
            &app,
            &operation_id,
            "done",
            "info",
            "repo overlay added",
        );
        Ok::<(), AppError>(())
    }
    .await;
    if let Err(err) = result {
        state.db.finish_operation(
            &operation_id,
            OperationStatus::Failed,
            Some(&err.to_string()),
            None,
        )?;
        log_operation(
            &state,
            &app,
            &operation_id,
            "error",
            "error",
            err.to_string(),
        );
        return Err(err);
    }
    Ok(())
}

#[tauri::command]
async fn set_repo_overlay_enabled(
    app: AppHandle,
    state: State<'_, AppState>,
    input: SetRepoOverlayEnabledInput,
) -> Result<OperationStart, String> {
    let repo = state
        .db
        .get_repo(&input.repo_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "repo not found".to_string())?;
    let installation = state
        .db
        .get_installation(&repo.installation_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "installation not found".to_string())?;
    let background_work_lock = state.background_work_lock();
    let _background_work_accept_guard = background_work_lock.lock().await;
    let op = state
        .db
        .create_operation(
            &installation.id,
            Some(&repo.id),
            OperationKind::ManageRepoStack,
            Some(&input.overlay_id),
        )
        .map_err(|e| e.to_string())?;
    let op_id = op.id.clone();
    let state_handle = app.state::<AppState>().inner().clone();
    tauri::async_runtime::spawn(async move {
        let _ =
            run_set_repo_overlay_enabled(app, state_handle, installation, repo, input, op_id).await;
    });
    Ok(OperationStart {
        operation_id: op.id,
    })
}

async fn run_set_repo_overlay_enabled(
    app: AppHandle,
    state: AppState,
    installation: Installation,
    repo: ManagedRepo,
    input: SetRepoOverlayEnabledInput,
    operation_id: String,
) -> AppResult<()> {
    let _background_work_guard = acquire_background_work_guard(&state).await;
    state.db.set_operation_running(&operation_id)?;
    let repo_lock = state.repo_lock(&repo.id).await;
    let _guard = repo_lock.lock().await;
    log_operation(
        &state,
        &app,
        &operation_id,
        "preflight",
        "info",
        stack_operation_summary("updating overlay activation", &repo),
    );
    let result = async {
        let mut tracked_state = load_repo_tracked_state(&state, &installation, &repo)
            .await?
            .ok_or_else(|| AppError::InvalidInput("repo has no tracked stack".to_string()))?;
        let index = overlay_index(&tracked_state, &input.overlay_id)?;
        tracked_state.overlays[index].enabled = input.enabled;
        let checkpoint = apply_repo_tracking_state(
            &state,
            &app,
            &operation_id,
            &installation,
            &repo,
            &tracked_state,
            &input.dirty_repo_strategy,
            input.sync_dependencies,
            true,
        )
        .await?;
        state.db.finish_operation(
            &operation_id,
            OperationStatus::Succeeded,
            None,
            Some(&checkpoint.id),
        )?;
        log_operation(
            &state,
            &app,
            &operation_id,
            "done",
            "info",
            "overlay activation updated",
        );
        Ok::<(), AppError>(())
    }
    .await;
    if let Err(err) = result {
        state.db.finish_operation(
            &operation_id,
            OperationStatus::Failed,
            Some(&err.to_string()),
            None,
        )?;
        log_operation(
            &state,
            &app,
            &operation_id,
            "error",
            "error",
            err.to_string(),
        );
        return Err(err);
    }
    Ok(())
}

#[tauri::command]
async fn remove_repo_overlay(
    app: AppHandle,
    state: State<'_, AppState>,
    input: RemoveRepoOverlayInput,
) -> Result<OperationStart, String> {
    let repo = state
        .db
        .get_repo(&input.repo_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "repo not found".to_string())?;
    let installation = state
        .db
        .get_installation(&repo.installation_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "installation not found".to_string())?;
    let background_work_lock = state.background_work_lock();
    let _background_work_accept_guard = background_work_lock.lock().await;
    let op = state
        .db
        .create_operation(
            &installation.id,
            Some(&repo.id),
            OperationKind::ManageRepoStack,
            Some(&input.overlay_id),
        )
        .map_err(|e| e.to_string())?;
    let op_id = op.id.clone();
    let state_handle = app.state::<AppState>().inner().clone();
    tauri::async_runtime::spawn(async move {
        let _ = run_remove_repo_overlay(app, state_handle, installation, repo, input, op_id).await;
    });
    Ok(OperationStart {
        operation_id: op.id,
    })
}

async fn run_remove_repo_overlay(
    app: AppHandle,
    state: AppState,
    installation: Installation,
    repo: ManagedRepo,
    input: RemoveRepoOverlayInput,
    operation_id: String,
) -> AppResult<()> {
    let _background_work_guard = acquire_background_work_guard(&state).await;
    state.db.set_operation_running(&operation_id)?;
    let repo_lock = state.repo_lock(&repo.id).await;
    let _guard = repo_lock.lock().await;
    log_operation(
        &state,
        &app,
        &operation_id,
        "preflight",
        "info",
        stack_operation_summary("removing repo overlay", &repo),
    );
    let result = async {
        let mut tracked_state = load_repo_tracked_state(&state, &installation, &repo)
            .await?
            .ok_or_else(|| AppError::InvalidInput("repo has no tracked stack".to_string()))?;
        let index = overlay_index(&tracked_state, &input.overlay_id)?;
        tracked_state.overlays.remove(index);
        normalize_overlay_positions(&mut tracked_state);
        tracked_state.materialized_branch =
            (!tracked_state.overlays.is_empty()).then(|| STACK_BRANCH_NAME.to_string());
        let checkpoint = apply_repo_tracking_state(
            &state,
            &app,
            &operation_id,
            &installation,
            &repo,
            &tracked_state,
            &input.dirty_repo_strategy,
            input.sync_dependencies,
            true,
        )
        .await?;
        state.db.finish_operation(
            &operation_id,
            OperationStatus::Succeeded,
            None,
            Some(&checkpoint.id),
        )?;
        log_operation(
            &state,
            &app,
            &operation_id,
            "done",
            "info",
            "repo overlay removed",
        );
        Ok::<(), AppError>(())
    }
    .await;
    if let Err(err) = result {
        state.db.finish_operation(
            &operation_id,
            OperationStatus::Failed,
            Some(&err.to_string()),
            None,
        )?;
        log_operation(
            &state,
            &app,
            &operation_id,
            "error",
            "error",
            err.to_string(),
        );
        return Err(err);
    }
    Ok(())
}

#[tauri::command]
async fn move_repo_overlay(
    app: AppHandle,
    state: State<'_, AppState>,
    input: MoveRepoOverlayInput,
) -> Result<OperationStart, String> {
    let repo = state
        .db
        .get_repo(&input.repo_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "repo not found".to_string())?;
    let installation = state
        .db
        .get_installation(&repo.installation_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "installation not found".to_string())?;
    let background_work_lock = state.background_work_lock();
    let _background_work_accept_guard = background_work_lock.lock().await;
    let op = state
        .db
        .create_operation(
            &installation.id,
            Some(&repo.id),
            OperationKind::ManageRepoStack,
            Some(&input.overlay_id),
        )
        .map_err(|e| e.to_string())?;
    let op_id = op.id.clone();
    let state_handle = app.state::<AppState>().inner().clone();
    tauri::async_runtime::spawn(async move {
        let _ = run_move_repo_overlay(app, state_handle, installation, repo, input, op_id).await;
    });
    Ok(OperationStart {
        operation_id: op.id,
    })
}

async fn run_move_repo_overlay(
    app: AppHandle,
    state: AppState,
    installation: Installation,
    repo: ManagedRepo,
    input: MoveRepoOverlayInput,
    operation_id: String,
) -> AppResult<()> {
    let _background_work_guard = acquire_background_work_guard(&state).await;
    state.db.set_operation_running(&operation_id)?;
    let repo_lock = state.repo_lock(&repo.id).await;
    let _guard = repo_lock.lock().await;
    log_operation(
        &state,
        &app,
        &operation_id,
        "preflight",
        "info",
        stack_operation_summary("reordering repo overlay", &repo),
    );
    let result = async {
        let mut tracked_state = load_repo_tracked_state(&state, &installation, &repo)
            .await?
            .ok_or_else(|| AppError::InvalidInput("repo has no tracked stack".to_string()))?;
        let index = overlay_index(&tracked_state, &input.overlay_id)?;
        let target_index = match input.direction {
            OverlayMoveDirection::Up if index > 0 => index - 1,
            OverlayMoveDirection::Down if index + 1 < tracked_state.overlays.len() => index + 1,
            _ => index,
        };
        if target_index != index {
            tracked_state.overlays.swap(index, target_index);
            normalize_overlay_positions(&mut tracked_state);
        }
        let checkpoint = apply_repo_tracking_state(
            &state,
            &app,
            &operation_id,
            &installation,
            &repo,
            &tracked_state,
            &input.dirty_repo_strategy,
            input.sync_dependencies,
            true,
        )
        .await?;
        state.db.finish_operation(
            &operation_id,
            OperationStatus::Succeeded,
            None,
            Some(&checkpoint.id),
        )?;
        log_operation(
            &state,
            &app,
            &operation_id,
            "done",
            "info",
            "repo overlay order updated",
        );
        Ok::<(), AppError>(())
    }
    .await;
    if let Err(err) = result {
        state.db.finish_operation(
            &operation_id,
            OperationStatus::Failed,
            Some(&err.to_string()),
            None,
        )?;
        log_operation(
            &state,
            &app,
            &operation_id,
            "error",
            "error",
            err.to_string(),
        );
        return Err(err);
    }
    Ok(())
}

#[tauri::command]
async fn update_repo(
    app: AppHandle,
    state: State<'_, AppState>,
    input: UpdateRepoInput,
) -> Result<OperationStart, String> {
    let repo = state
        .db
        .get_repo(&input.repo_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "repo not found".to_string())?;
    let installation = state
        .db
        .get_installation(&repo.installation_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "installation not found".to_string())?;
    let background_work_lock = state.background_work_lock();
    let _background_work_accept_guard = background_work_lock.lock().await;
    let op = state
        .db
        .create_operation(
            &installation.id,
            Some(&repo.id),
            OperationKind::UpdateRepo,
            repo.tracked_state
                .as_ref()
                .map(|tracked| tracked.base.source_input.as_str())
                .or(repo.tracked_target_input.as_deref()),
        )
        .map_err(|e| e.to_string())?;
    let op_id = op.id.clone();
    let state_handle = app.state::<AppState>().inner().clone();
    tauri::async_runtime::spawn(async move {
        let _ = run_update_repo(app, state_handle, installation, repo, input, op_id).await;
    });
    Ok(OperationStart {
        operation_id: op.id,
    })
}

async fn run_update_repo(
    app: AppHandle,
    state: AppState,
    installation: Installation,
    repo: ManagedRepo,
    input: UpdateRepoInput,
    operation_id: String,
) -> AppResult<()> {
    let _background_work_guard = acquire_background_work_guard(&state).await;
    state.db.set_operation_running(&operation_id)?;
    let repo_lock = state.repo_lock(&repo.id).await;
    let _guard = repo_lock.lock().await;
    log_operation(
        &state,
        &app,
        &operation_id,
        "preflight",
        "info",
        format!("updating {}", repo.display_name),
    );
    let result = async {
        let tracked_state = load_repo_tracked_state(&state, &installation, &repo)
            .await?
            .ok_or_else(|| AppError::InvalidInput("repo has no tracked target".to_string()))?;
        let checkpoint = apply_repo_tracking_state(
            &state,
            &app,
            &operation_id,
            &installation,
            &repo,
            &tracked_state,
            &input.dirty_repo_strategy,
            input.sync_dependencies,
            true,
        )
        .await?;
        state.db.finish_operation(
            &operation_id,
            OperationStatus::Succeeded,
            None,
            Some(&checkpoint.id),
        )?;
        log_operation(
            &state,
            &app,
            &operation_id,
            "done",
            "info",
            "repo update completed",
        );
        Ok::<(), AppError>(())
    }
    .await;
    if let Err(err) = result {
        state.db.finish_operation(
            &operation_id,
            OperationStatus::Failed,
            Some(&err.to_string()),
            None,
        )?;
        log_operation(
            &state,
            &app,
            &operation_id,
            "error",
            "error",
            err.to_string(),
        );
        return Err(err);
    }
    Ok(())
}

#[tauri::command]
async fn update_all(
    app: AppHandle,
    state: State<'_, AppState>,
    input: UpdateAllInput,
) -> Result<OperationStart, String> {
    let installation = state
        .db
        .get_installation(&input.installation_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "installation not found".to_string())?;
    let background_work_lock = state.background_work_lock();
    let _background_work_accept_guard = background_work_lock.lock().await;
    let op = state
        .db
        .create_operation(&installation.id, None, OperationKind::UpdateAll, None)
        .map_err(|e| e.to_string())?;
    let op_id = op.id.clone();
    let state_handle = app.state::<AppState>().inner().clone();
    tauri::async_runtime::spawn(async move {
        let _ = run_update_all(app, state_handle, installation, input, op_id).await;
    });
    Ok(OperationStart {
        operation_id: op.id,
    })
}

async fn run_update_all(
    app: AppHandle,
    state: AppState,
    installation: Installation,
    input: UpdateAllInput,
    operation_id: String,
) -> AppResult<()> {
    let _background_work_guard = acquire_background_work_guard(&state).await;
    state.db.set_operation_running(&operation_id)?;
    log_operation(
        &state,
        &app,
        &operation_id,
        "preflight",
        "info",
        "updating all tracked repositories",
    );
    let result = async {
        let detail = state.db.get_installation_detail(&installation.id)?;
        let mut repos = Vec::new();
        if let Some(core) = detail.core_repo {
            repos.push(core);
        }
        if let Some(frontend) = detail.frontend_repo {
            repos.push(frontend);
        }
        repos.extend(detail.custom_node_repos);
        let mut checkpoints = Vec::new();
        let mut failures = Vec::new();

        for repo in repos {
            if let Some(tracked_state) = load_repo_tracked_state(&state, &installation, &repo).await?
            {
                let repo_lock = state.repo_lock(&repo.id).await;
                let _guard = repo_lock.lock().await;
                log_operation(
                    &state,
                    &app,
                    &operation_id,
                    "preflight",
                    "info",
                    format!("updating {}", repo.display_name),
                );
                match apply_repo_tracking_state(
                    &state,
                    &app,
                    &operation_id,
                    &installation,
                    &repo,
                    &tracked_state,
                    &input.dirty_repo_strategy,
                    input.sync_dependencies,
                    true,
                )
                .await
                {
                    Ok(checkpoint) => checkpoints.push(checkpoint.id),
                    Err(err) => failures.push(format!("{}: {}", repo.display_name, err)),
                }
            } else {
                log_operation(
                    &state,
                    &app,
                    &operation_id,
                    "preflight",
                    "warn",
                    format!("skipping {}: no tracked target", repo.display_name),
                );
            }
        }

        if failures.is_empty() {
            maybe_restart_installation(
                &state,
                &app,
                &operation_id,
                &installation,
                input.restart_after_success,
            )
            .await?;
            let checkpoint_ref = checkpoints.last().map(|v| v.as_str());
            state.db.finish_operation(
                &operation_id,
                OperationStatus::Succeeded,
                None,
                checkpoint_ref,
            )?;
            log_operation(
                &state,
                &app,
                &operation_id,
                "done",
                "info",
                "update all completed",
            );
            Ok::<(), AppError>(())
        } else {
            let message = failures.join("\n");
            let checkpoint_ref = checkpoints.last().map(|v| v.as_str());
            state.db.finish_operation(
                &operation_id,
                OperationStatus::Failed,
                Some(&message),
                checkpoint_ref,
            )?;
            log_operation(
                &state,
                &app,
                &operation_id,
                "error",
                "error",
                message.clone(),
            );
            Err(AppError::Git(message))
        }
    }
    .await;
    if let Err(err) = result {
        if let Some(op) = state.db.get_operation(&operation_id)? {
            if matches!(op.status, OperationStatus::Running) {
                state.db.finish_operation(
                    &operation_id,
                    OperationStatus::Failed,
                    Some(&err.to_string()),
                    op.checkpoint_id.as_deref(),
                )?;
                log_operation(
                    &state,
                    &app,
                    &operation_id,
                    "error",
                    "error",
                    err.to_string(),
                );
            }
        }
        return Err(err);
    }
    Ok(())
}

#[tauri::command]
async fn rollback_repo(
    app: AppHandle,
    state: State<'_, AppState>,
    input: RollbackRepoInput,
) -> Result<OperationStart, String> {
    let repo = state
        .db
        .get_repo(&input.repo_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "repo not found".to_string())?;
    let installation = state
        .db
        .get_installation(&repo.installation_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "installation not found".to_string())?;
    let background_work_lock = state.background_work_lock();
    let _background_work_accept_guard = background_work_lock.lock().await;
    let op = state
        .db
        .create_operation(
            &installation.id,
            Some(&repo.id),
            OperationKind::RollbackRepo,
            None,
        )
        .map_err(|e| e.to_string())?;
    let op_id = op.id.clone();
    let state_handle = app.state::<AppState>().inner().clone();
    tauri::async_runtime::spawn(async move {
        let _ = run_rollback_repo(app, state_handle, installation, repo, input, op_id).await;
    });
    Ok(OperationStart {
        operation_id: op.id,
    })
}

async fn run_rollback_repo(
    app: AppHandle,
    state: AppState,
    installation: Installation,
    repo: ManagedRepo,
    input: RollbackRepoInput,
    operation_id: String,
) -> AppResult<()> {
    let _background_work_guard = acquire_background_work_guard(&state).await;
    state.db.set_operation_running(&operation_id)?;
    let repo_lock = state.repo_lock(&repo.id).await;
    let _guard = repo_lock.lock().await;
    let result = async {
        let checkpoint = state
            .db
            .latest_checkpoint(&repo.id)?
            .ok_or_else(|| AppError::NotFound("no checkpoint available for repo".to_string()))?;
        let safety_checkpoint = create_checkpoint_if_needed(
            &state,
            &installation,
            &repo,
            None,
            &operation_id,
            &DirtyRepoStrategy::Stash,
        )
        .await?;
        log_operation(
            &state,
            &app,
            &operation_id,
            "checkpoint",
            "info",
            format!(
                "created safety checkpoint {} ({}) before restore",
                safety_checkpoint
                    .label
                    .as_deref()
                    .unwrap_or(&safety_checkpoint.id),
                safety_checkpoint.old_head_sha
            ),
        );
        log_operation(
            &state,
            &app,
            &operation_id,
            "checkpoint",
            "info",
            format!("restoring {}", checkpoint.old_head_sha),
        );
        let path = Path::new(&repo.local_path);
        restore_checkpoint_state(
            &state,
            path,
            &repo.id,
            &checkpoint,
            input.restore_stash,
        )
        .await?;
        let _ = submodule_update(path).await;
        maybe_sync_dependencies(
            &state,
            &app,
            &operation_id,
            &installation,
            &repo,
            path,
            input.sync_dependencies,
        )
        .await?;
        refresh_repo_state(&state, &repo.id).await?;
        maybe_restart_installation(
            &state,
            &app,
            &operation_id,
            &installation,
            input.restart_after_success,
        )
        .await?;
        state.db.finish_operation(
            &operation_id,
            OperationStatus::Succeeded,
            None,
            Some(&checkpoint.id),
        )?;
        log_operation(
            &state,
            &app,
            &operation_id,
            "done",
            "info",
            "rollback completed",
        );
        Ok::<(), AppError>(())
    }
    .await;
    if let Err(err) = result {
        state.db.finish_operation(
            &operation_id,
            OperationStatus::Failed,
            Some(&err.to_string()),
            None,
        )?;
        log_operation(
            &state,
            &app,
            &operation_id,
            "error",
            "error",
            err.to_string(),
        );
        return Err(err);
    }
    Ok(())
}

#[tauri::command]
async fn restore_checkpoint(
    app: AppHandle,
    state: State<'_, AppState>,
    input: RestoreCheckpointInput,
) -> Result<OperationStart, String> {
    let repo = state
        .db
        .get_repo(&input.repo_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "repo not found".to_string())?;
    let installation = state
        .db
        .get_installation(&repo.installation_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "installation not found".to_string())?;
    let background_work_lock = state.background_work_lock();
    let _background_work_accept_guard = background_work_lock.lock().await;
    let op = state
        .db
        .create_operation(
            &installation.id,
            Some(&repo.id),
            OperationKind::RestoreCheckpoint,
            Some(&input.checkpoint_id),
        )
        .map_err(|e| e.to_string())?;
    let op_id = op.id.clone();
    let state_handle = app.state::<AppState>().inner().clone();
    tauri::async_runtime::spawn(async move {
        let _ = run_restore_checkpoint(app, state_handle, installation, repo, input, op_id).await;
    });
    Ok(OperationStart {
        operation_id: op.id,
    })
}

async fn run_restore_checkpoint(
    app: AppHandle,
    state: AppState,
    installation: Installation,
    repo: ManagedRepo,
    input: RestoreCheckpointInput,
    operation_id: String,
) -> AppResult<()> {
    let _background_work_guard = acquire_background_work_guard(&state).await;
    state.db.set_operation_running(&operation_id)?;
    let repo_lock = state.repo_lock(&repo.id).await;
    let _guard = repo_lock.lock().await;
    let result = async {
        let checkpoint = state
            .db
            .get_checkpoint(&input.checkpoint_id)?
            .ok_or_else(|| AppError::NotFound("checkpoint not found".to_string()))?;
        if checkpoint.repo_id != repo.id {
            return Err(AppError::Conflict(
                "checkpoint does not belong to the selected repo".to_string(),
            ));
        }
        let path = Path::new(&repo.local_path);
        let safety_checkpoint = create_checkpoint_if_needed(
            &state,
            &installation,
            &repo,
            None,
            &operation_id,
            &DirtyRepoStrategy::Stash,
        )
        .await?;
        log_operation(
            &state,
            &app,
            &operation_id,
            "checkpoint",
            "info",
            format!(
                "created safety checkpoint {} ({}) before restore",
                safety_checkpoint
                    .label
                    .as_deref()
                    .unwrap_or(&safety_checkpoint.id),
                safety_checkpoint.old_head_sha
            ),
        );
        log_operation(
            &state,
            &app,
            &operation_id,
            "checkpoint",
            "info",
            format!(
                "restoring checkpoint {} ({})",
                checkpoint.label.as_deref().unwrap_or(&checkpoint.id),
                checkpoint.old_head_sha
            ),
        );
        restore_checkpoint_state(&state, path, &repo.id, &checkpoint, input.restore_stash).await?;
        let _ = submodule_update(path).await;
        maybe_sync_dependencies(
            &state,
            &app,
            &operation_id,
            &installation,
            &repo,
            path,
            input.sync_dependencies,
        )
        .await?;
        refresh_repo_state(&state, &repo.id).await?;
        maybe_restart_installation(
            &state,
            &app,
            &operation_id,
            &installation,
            input.restart_after_success,
        )
        .await?;
        state.db.finish_operation(
            &operation_id,
            OperationStatus::Succeeded,
            None,
            Some(&checkpoint.id),
        )?;
        log_operation(
            &state,
            &app,
            &operation_id,
            "done",
            "info",
            "checkpoint restore completed",
        );
        Ok::<(), AppError>(())
    }
    .await;
    if let Err(err) = result {
        state.db.finish_operation(
            &operation_id,
            OperationStatus::Failed,
            Some(&err.to_string()),
            None,
        )?;
        log_operation(
            &state,
            &app,
            &operation_id,
            "error",
            "error",
            err.to_string(),
        );
        return Err(err);
    }
    Ok(())
}

#[tauri::command]
async fn compare_checkpoint(
    state: State<'_, AppState>,
    repo_id: String,
    checkpoint_id: String,
) -> Result<RepoCheckpointComparison, String> {
    let repo = state
        .db
        .get_repo(&repo_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "repo not found".to_string())?;
    let installation = state
        .db
        .get_installation(&repo.installation_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "installation not found".to_string())?;
    let checkpoint = state
        .db
        .get_checkpoint(&checkpoint_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "checkpoint not found".to_string())?;
    if checkpoint.repo_id != repo.id {
        return Err("checkpoint does not belong to the selected repo".to_string());
    }
    let repo_lock = state.repo_lock(&repo.id).await;
    let _guard = repo_lock.lock().await;
    let live_repo = enrich_managed_repo(&state, &installation, &repo, None)
        .await
        .map_err(|e| e.to_string())?;
    let mut commits = Vec::new();
    let mut file_changes = Vec::new();
    let mut warnings = live_repo.live_warnings.clone();
    if matches!(live_repo.live_status, RepoLiveStatus::Missing | RepoLiveStatus::NotGit) {
        warnings.push(
            "Checkpoint comparison is unavailable because this repo is not currently a valid git worktree."
                .to_string(),
        );
    } else if let Some(current_head) = live_repo.current_head_sha.as_deref() {
        let path = Path::new(&repo.local_path);
        match commits_between(path, &checkpoint.old_head_sha, current_head, 20).await {
            Ok(next_commits) => commits = next_commits,
            Err(error) => warnings.push(format!(
                "Unable to inspect commits from this checkpoint; the saved commit may no longer exist locally: {}",
                error
            )),
        }
        match diff_name_status(path, &checkpoint.old_head_sha, current_head).await {
            Ok(next_file_changes) => file_changes = next_file_changes,
            Err(error) => warnings.push(format!(
                "Unable to inspect changed files from this checkpoint; the saved commit may no longer exist locally: {}",
                error
            )),
        }
    }
    if live_repo.current_head_sha.as_deref() == Some(checkpoint.old_head_sha.as_str()) {
        warnings.push("Current disk state already matches this checkpoint.".to_string());
    }
    Ok(RepoCheckpointComparison {
        checkpoint,
        current_head_sha: live_repo.current_head_sha.clone(),
        commits,
        file_changes,
        warnings,
        current_dependency_state: live_repo.dependency_state.clone(),
    })
}

#[tauri::command]
async fn uninstall_repo(
    app: AppHandle,
    state: State<'_, AppState>,
    input: RepoLifecycleInput,
) -> Result<OperationStart, String> {
    let repo = state
        .db
        .get_repo(&input.repo_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "repo not found".to_string())?;
    ensure_repo_lifecycle_supported(&repo, "Uninstall").map_err(|e| e.to_string())?;
    let installation = state
        .db
        .get_installation(&repo.installation_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "installation not found".to_string())?;
    let background_work_lock = state.background_work_lock();
    let _background_work_accept_guard = background_work_lock.lock().await;
    let op = state
        .db
        .create_operation(
            &installation.id,
            Some(&repo.id),
            OperationKind::UninstallRepo,
            Some(&repo.local_path),
        )
        .map_err(|e| e.to_string())?;
    let op_id = op.id.clone();
    let state_handle = app.state::<AppState>().inner().clone();
    tauri::async_runtime::spawn(async move {
        let _ = run_uninstall_repo(app, state_handle, repo, op_id).await;
    });
    Ok(OperationStart {
        operation_id: op.id,
    })
}

async fn run_uninstall_repo(
    app: AppHandle,
    state: AppState,
    repo: ManagedRepo,
    operation_id: String,
) -> AppResult<()> {
    let _background_work_guard = acquire_background_work_guard(&state).await;
    state.db.set_operation_running(&operation_id)?;
    let repo_lock = state.repo_lock(&repo.id).await;
    let _guard = repo_lock.lock().await;
    let result = async {
        let path = PathBuf::from(&repo.local_path);
        log_operation(
            &state,
            &app,
            &operation_id,
            "preflight",
            "info",
            format!("uninstalling {}", repo.display_name),
        );
        if path.exists() {
            if path.is_dir() {
                std::fs::remove_dir_all(&path)?;
            } else {
                std::fs::remove_file(&path)?;
            }
        }
        state.db.delete_repo(&repo.id)?;
        state.db.finish_operation(&operation_id, OperationStatus::Succeeded, None, None)?;
        log_operation(
            &state,
            &app,
            &operation_id,
            "done",
            "info",
            "repo uninstall completed",
        );
        Ok::<(), AppError>(())
    }
    .await;
    if let Err(err) = result {
        state.db.finish_operation(
            &operation_id,
            OperationStatus::Failed,
            Some(&err.to_string()),
            None,
        )?;
        log_operation(
            &state,
            &app,
            &operation_id,
            "error",
            "error",
            err.to_string(),
        );
        return Err(err);
    }
    Ok(())
}

#[tauri::command]
async fn disable_repo(
    app: AppHandle,
    state: State<'_, AppState>,
    input: RepoLifecycleInput,
) -> Result<OperationStart, String> {
    let repo = state
        .db
        .get_repo(&input.repo_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "repo not found".to_string())?;
    ensure_repo_lifecycle_supported(&repo, "Disable").map_err(|e| e.to_string())?;
    let installation = state
        .db
        .get_installation(&repo.installation_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "installation not found".to_string())?;
    let background_work_lock = state.background_work_lock();
    let _background_work_accept_guard = background_work_lock.lock().await;
    let op = state
        .db
        .create_operation(
            &installation.id,
            Some(&repo.id),
            OperationKind::DisableRepo,
            Some(&repo.local_path),
        )
        .map_err(|e| e.to_string())?;
    let op_id = op.id.clone();
    let state_handle = app.state::<AppState>().inner().clone();
    tauri::async_runtime::spawn(async move {
        let _ = run_disable_repo(app, state_handle, installation, repo, op_id).await;
    });
    Ok(OperationStart {
        operation_id: op.id,
    })
}

async fn run_disable_repo(
    app: AppHandle,
    state: AppState,
    installation: Installation,
    repo: ManagedRepo,
    operation_id: String,
) -> AppResult<()> {
    let _background_work_guard = acquire_background_work_guard(&state).await;
    state.db.set_operation_running(&operation_id)?;
    let repo_lock = state.repo_lock(&repo.id).await;
    let _guard = repo_lock.lock().await;
    let result = async {
        let path = PathBuf::from(&repo.local_path);
        let disabled_root = installation_retained_backup_root(&installation, "disabled");
        std::fs::create_dir_all(&disabled_root)?;
        let disabled_path = choose_tracking_backup_path(&path, &disabled_root);
        log_operation(
            &state,
            &app,
            &operation_id,
            "preflight",
            "info",
            format!(
                "moving {} to disabled location {}",
                repo.display_name,
                disabled_path.to_string_lossy()
            ),
        );
        if path.exists() {
            std::fs::rename(&path, &disabled_path)?;
        }
        state.db.delete_repo(&repo.id)?;
        state.db.finish_operation(&operation_id, OperationStatus::Succeeded, None, None)?;
        log_operation(
            &state,
            &app,
            &operation_id,
            "done",
            "info",
            "repo disable completed",
        );
        Ok::<(), AppError>(())
    }
    .await;
    if let Err(err) = result {
        state.db.finish_operation(
            &operation_id,
            OperationStatus::Failed,
            Some(&err.to_string()),
            None,
        )?;
        log_operation(
            &state,
            &app,
            &operation_id,
            "error",
            "error",
            err.to_string(),
        );
        return Err(err);
    }
    Ok(())
}

#[tauri::command]
async fn untrack_repo(
    app: AppHandle,
    state: State<'_, AppState>,
    input: RepoLifecycleInput,
) -> Result<OperationStart, String> {
    let repo = state
        .db
        .get_repo(&input.repo_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "repo not found".to_string())?;
    ensure_repo_lifecycle_supported(&repo, "Untrack").map_err(|e| e.to_string())?;
    let installation = state
        .db
        .get_installation(&repo.installation_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "installation not found".to_string())?;
    let background_work_lock = state.background_work_lock();
    let _background_work_accept_guard = background_work_lock.lock().await;
    let op = state
        .db
        .create_operation(
            &installation.id,
            Some(&repo.id),
            OperationKind::UntrackRepo,
            Some(&repo.local_path),
        )
        .map_err(|e| e.to_string())?;
    let op_id = op.id.clone();
    let state_handle = app.state::<AppState>().inner().clone();
    tauri::async_runtime::spawn(async move {
        let _ = run_untrack_repo(app, state_handle, repo, op_id).await;
    });
    Ok(OperationStart {
        operation_id: op.id,
    })
}

async fn run_untrack_repo(
    app: AppHandle,
    state: AppState,
    repo: ManagedRepo,
    operation_id: String,
) -> AppResult<()> {
    let _background_work_guard = acquire_background_work_guard(&state).await;
    state.db.set_operation_running(&operation_id)?;
    let repo_lock = state.repo_lock(&repo.id).await;
    let _guard = repo_lock.lock().await;
    let result = async {
        log_operation(
            &state,
            &app,
            &operation_id,
            "preflight",
            "info",
            format!("stopping management for {}", repo.display_name),
        );
        state
            .db
            .ignore_repo_path(&repo.installation_id, &repo.kind, &repo.local_path)?;
        state.db.delete_repo(&repo.id)?;
        state.db.finish_operation(&operation_id, OperationStatus::Succeeded, None, None)?;
        log_operation(
            &state,
            &app,
            &operation_id,
            "done",
            "info",
            "repo untrack completed",
        );
        Ok::<(), AppError>(())
    }
    .await;
    if let Err(err) = result {
        state.db.finish_operation(
            &operation_id,
            OperationStatus::Failed,
            Some(&err.to_string()),
            None,
        )?;
        log_operation(
            &state,
            &app,
            &operation_id,
            "error",
            "error",
            err.to_string(),
        );
        return Err(err);
    }
    Ok(())
}

#[tauri::command]
async fn start_installation(
    app: AppHandle,
    state: State<'_, AppState>,
    input: StartInstallationInput,
) -> Result<OperationStart, String> {
    let lifecycle_lock = state.lifecycle_lock();
    let _lifecycle_accept_guard = lifecycle_lock.lock().await;
    let installation = state
        .db
        .get_installation(&input.installation_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "installation not found".to_string())?;
    let op = state
        .db
        .create_operation(
            &installation.id,
            None,
            OperationKind::StartInstallation,
            None,
        )
        .map_err(|e| e.to_string())?;
    let op_id = op.id.clone();
    let state_handle = app.state::<AppState>().inner().clone();
    tauri::async_runtime::spawn(async move {
        let _ = run_start_installation(app, state_handle, installation, op_id).await;
    });
    Ok(OperationStart {
        operation_id: op.id,
    })
}

async fn run_start_installation(
    app: AppHandle,
    state: AppState,
    installation: Installation,
    operation_id: String,
) -> AppResult<()> {
    state.db.set_operation_running(&operation_id)?;
    let lock = state.installation_lock(&installation.id).await;
    let _guard = lock.lock().await;
    let lifecycle_lock = state.lifecycle_lock();
    let _lifecycle_guard = lifecycle_lock.lock().await;
    let result = async {
        let require_managed_frontend_dist = state
            .db
            .get_installation_detail(&installation.id)?
            .frontend_repo
            .is_some();
        let profile = effective_launch_profile(&installation, require_managed_frontend_dist)?;
        log_operation(
            &state,
            &app,
            &operation_id,
            "start",
            "info",
            "starting installation",
        );
        state.processes.start(&installation.id, &profile).await?;
        state
            .db
            .finish_operation(&operation_id, OperationStatus::Succeeded, None, None)?;
        log_operation(
            &state,
            &app,
            &operation_id,
            "done",
            "info",
            "start completed",
        );
        Ok::<(), AppError>(())
    }
    .await;
    if let Err(err) = result {
        state.db.finish_operation(
            &operation_id,
            OperationStatus::Failed,
            Some(&err.to_string()),
            None,
        )?;
        log_operation(
            &state,
            &app,
            &operation_id,
            "error",
            "error",
            err.to_string(),
        );
        return Err(err);
    }
    Ok(())
}

#[tauri::command]
async fn stop_installation(
    app: AppHandle,
    state: State<'_, AppState>,
    input: StopInstallationInput,
) -> Result<OperationStart, String> {
    let lifecycle_lock = state.lifecycle_lock();
    let _lifecycle_accept_guard = lifecycle_lock.lock().await;
    let installation = state
        .db
        .get_installation(&input.installation_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "installation not found".to_string())?;
    let op = state
        .db
        .create_operation(
            &installation.id,
            None,
            OperationKind::StopInstallation,
            None,
        )
        .map_err(|e| e.to_string())?;
    let op_id = op.id.clone();
    let state_handle = app.state::<AppState>().inner().clone();
    tauri::async_runtime::spawn(async move {
        let _ = run_stop_installation(app, state_handle, installation, op_id).await;
    });
    Ok(OperationStart {
        operation_id: op.id,
    })
}

async fn run_stop_installation(
    app: AppHandle,
    state: AppState,
    installation: Installation,
    operation_id: String,
) -> AppResult<()> {
    state.db.set_operation_running(&operation_id)?;
    let lock = state.installation_lock(&installation.id).await;
    let _guard = lock.lock().await;
    let lifecycle_lock = state.lifecycle_lock();
    let _lifecycle_guard = lifecycle_lock.lock().await;
    let result = async {
        let profile = installation
            .launch_profile
            .clone()
            .ok_or_else(|| AppError::Process("installation has no launch profile".to_string()))?;
        log_operation(
            &state,
            &app,
            &operation_id,
            "stop",
            "info",
            "stopping installation",
        );
        let stopped = state.processes.stop(&installation.id, &profile).await?;
        if !stopped {
            log_operation(
                &state,
                &app,
                &operation_id,
                "stop",
                "info",
                "installation was already stopped",
            );
        }
        state
            .db
            .finish_operation(&operation_id, OperationStatus::Succeeded, None, None)?;
        log_operation(
            &state,
            &app,
            &operation_id,
            "done",
            "info",
            "stop completed",
        );
        Ok::<(), AppError>(())
    }
    .await;
    if let Err(err) = result {
        state.db.finish_operation(
            &operation_id,
            OperationStatus::Failed,
            Some(&err.to_string()),
            None,
        )?;
        log_operation(
            &state,
            &app,
            &operation_id,
            "error",
            "error",
            err.to_string(),
        );
        return Err(err);
    }
    Ok(())
}

#[tauri::command]
async fn restart_installation(
    app: AppHandle,
    state: State<'_, AppState>,
    input: RestartInstallationInput,
) -> Result<OperationStart, String> {
    let lifecycle_lock = state.lifecycle_lock();
    let _lifecycle_accept_guard = lifecycle_lock.lock().await;
    let installation = state
        .db
        .get_installation(&input.installation_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "installation not found".to_string())?;
    let op = state
        .db
        .create_operation(
            &installation.id,
            None,
            OperationKind::RestartInstallation,
            None,
        )
        .map_err(|e| e.to_string())?;
    let op_id = op.id.clone();
    let state_handle = app.state::<AppState>().inner().clone();
    tauri::async_runtime::spawn(async move {
        let _ = run_restart_installation(app, state_handle, installation, op_id).await;
    });
    Ok(OperationStart {
        operation_id: op.id,
    })
}

async fn run_restart_installation(
    app: AppHandle,
    state: AppState,
    installation: Installation,
    operation_id: String,
) -> AppResult<()> {
    state.db.set_operation_running(&operation_id)?;
    let lock = state.installation_lock(&installation.id).await;
    let _guard = lock.lock().await;
    let lifecycle_lock = state.lifecycle_lock();
    let _lifecycle_guard = lifecycle_lock.lock().await;
    let result = async {
        let require_managed_frontend_dist = state
            .db
            .get_installation_detail(&installation.id)?
            .frontend_repo
            .is_some();
        let profile = effective_launch_profile(&installation, require_managed_frontend_dist)?;
        log_operation(
            &state,
            &app,
            &operation_id,
            "restart",
            "info",
            "restarting installation",
        );
        state.processes.restart(&installation.id, &profile).await?;
        state
            .db
            .finish_operation(&operation_id, OperationStatus::Succeeded, None, None)?;
        log_operation(
            &state,
            &app,
            &operation_id,
            "done",
            "info",
            "restart completed",
        );
        Ok::<(), AppError>(())
    }
    .await;
    if let Err(err) = result {
        state.db.finish_operation(
            &operation_id,
            OperationStatus::Failed,
            Some(&err.to_string()),
            None,
        )?;
        log_operation(
            &state,
            &app,
            &operation_id,
            "error",
            "error",
            err.to_string(),
        );
        return Err(err);
    }
    Ok(())
}

#[tauri::command]
fn list_operations(
    state: State<'_, AppState>,
    installation_id: Option<String>,
) -> Result<Vec<OperationRecord>, String> {
    state
        .db
        .list_operations(installation_id.as_deref())
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn get_operation_log(state: State<'_, AppState>, operation_id: String) -> Result<String, String> {
    state
        .db
        .get_operation_log(&operation_id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn list_checkpoints(
    state: State<'_, AppState>,
    repo_id: String,
) -> Result<Vec<RepoCheckpoint>, String> {
    state
        .db
        .list_checkpoints(&repo_id)
        .map_err(|e| e.to_string())
}

#[cfg(desktop)]
mod app_updates {
    use super::shutdown_managed_installations;
    use crate::state::AppState;
    use serde::Serialize;
    use tauri::{AppHandle, Emitter, State};
    use tauri_plugin_updater::{Update, UpdaterExt};

    const APP_UPDATE_EVENT: &str = "app-update-event";
    const UPDATE_ENDPOINT: &str =
        "https://github.com/xmarre/ComfyUI-Patcher/releases/latest/download/latest.json";

    #[derive(Default)]
    pub struct PendingUpdate(pub tokio::sync::Mutex<Option<Update>>);

    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    pub struct UpdateMetadata {
        version: String,
        current_version: String,
    }

    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    pub struct UpdateCheckResult {
        enabled: bool,
        disabled_reason: Option<String>,
        update: Option<UpdateMetadata>,
    }

    #[derive(Clone, Serialize)]
    #[serde(rename_all = "camelCase")]
    pub struct UpdateEvent {
        kind: String,
        chunk_length: Option<usize>,
        content_length: Option<u64>,
    }

    fn updater_pubkey(app: &AppHandle) -> Option<String> {
        let updater = app.config().plugins.0.get("updater")?;
        let pubkey = updater.get("pubkey")?.as_str()?.trim();
        (!pubkey.is_empty()).then(|| pubkey.to_owned())
    }

    fn emit_update_event(app: &AppHandle, event: UpdateEvent) {
        let _ = app.emit(APP_UPDATE_EVENT, event);
    }

    #[tauri::command]
    pub async fn fetch_app_update(
        app: AppHandle,
        pending_update: State<'_, PendingUpdate>,
    ) -> Result<UpdateCheckResult, String> {
        {
            let mut pending = pending_update.0.lock().await;
            pending.take();
        }

        let pubkey = updater_pubkey(&app)
            .ok_or_else(|| "updater configuration is missing plugins.updater.pubkey".to_string())?;

        let update = app
            .updater_builder()
            .pubkey(&pubkey)
            .endpoints(vec![
                UPDATE_ENDPOINT
                    .parse()
                    .map_err(|e: url::ParseError| e.to_string())?,
            ])
            .map_err(|e| e.to_string())?
            .build()
            .map_err(|e| e.to_string())?
            .check()
            .await
            .map_err(|e| e.to_string())?;

        let metadata = update.as_ref().map(|update| UpdateMetadata {
            version: update.version.clone(),
            current_version: app.package_info().version.to_string(),
        });

        let mut pending = pending_update.0.lock().await;
        *pending = update;

        Ok(UpdateCheckResult {
            enabled: true,
            disabled_reason: None,
            update: metadata,
        })
    }

    #[tauri::command]
    pub async fn install_app_update(
        app: AppHandle,
        state: State<'_, AppState>,
        pending_update: State<'_, PendingUpdate>,
    ) -> Result<(), String> {
        let lifecycle_lock = state.lifecycle_lock();
        let _lifecycle_guard = lifecycle_lock.lock().await;
        let background_work_lock = state.background_work_lock();
        let _background_work_guard = background_work_lock.try_lock().map_err(|_| {
            "cannot install update while operations are running".to_string()
        })?;
        if state
            .db
            .has_in_flight_background_operations()
            .map_err(|e| e.to_string())?
        {
            return Err("cannot install update while operations are running".to_string());
        }
        if state
            .db
            .has_in_flight_lifecycle_operations()
            .map_err(|e| e.to_string())?
        {
            return Err("cannot install update while operations are running".to_string());
        }
        let update = {
            let mut pending = pending_update.0.lock().await;
            pending
                .take()
                .ok_or_else(|| "there is no pending update; check for updates first".to_string())?
        };

        shutdown_managed_installations(state.inner().clone())
            .await
            .map_err(|e| e.to_string())?;

        let mut started = false;
        update
            .download_and_install(
                |chunk_length, content_length| {
                    if !started {
                        emit_update_event(
                            &app,
                            UpdateEvent {
                                kind: "started".to_string(),
                                chunk_length: None,
                                content_length,
                            },
                        );
                        started = true;
                    }
                    emit_update_event(
                        &app,
                        UpdateEvent {
                            kind: "progress".to_string(),
                            chunk_length: Some(chunk_length),
                            content_length: None,
                        },
                    );
                },
                || {
                    emit_update_event(
                        &app,
                        UpdateEvent {
                            kind: "finished".to_string(),
                            chunk_length: None,
                            content_length: None,
                        },
                    );
                },
            )
            .await
            .map_err(|e| e.to_string())
    }
}

#[cfg(not(desktop))]
mod app_updates {
    use serde::Serialize;
    use tauri::{AppHandle, State};

    #[derive(Default)]
    pub struct PendingUpdate;

    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    pub struct UpdateMetadata {
        version: String,
        current_version: String,
    }

    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    pub struct UpdateCheckResult {
        enabled: bool,
        disabled_reason: Option<String>,
        update: Option<UpdateMetadata>,
    }

    #[tauri::command]
    pub async fn fetch_app_update(
        _app: AppHandle,
        _pending_update: State<'_, PendingUpdate>,
    ) -> Result<UpdateCheckResult, String> {
        Ok(UpdateCheckResult {
            enabled: false,
            disabled_reason: Some("Updater is only supported on desktop builds.".to_string()),
            update: None,
        })
    }

    #[tauri::command]
    pub async fn install_app_update(
        _app: AppHandle,
        _pending_update: State<'_, PendingUpdate>,
    ) -> Result<(), String> {
        Err("Updater is only supported on desktop builds.".to_string())
    }
}

async fn shutdown_managed_installations(state: AppState) -> AppResult<()> {
    let installations = match state.db.list_installations() {
        Ok(installations) => installations,
        Err(_) => {
            state.processes.shutdown_all().await;
            return Ok(());
        }
    };

    for installation in installations {
        if let Some(profile) = installation.launch_profile.as_ref() {
            if state.processes.stop(&installation.id, profile).await.is_err() {
                if let Err(force_stop_error) = state.processes.force_stop(&installation.id).await {
                    return Err(AppError::Process(format!(
                        "failed to stop managed installation '{}' during app update handoff: {}",
                        installation.name, force_stop_error
                    )));
                }
            }
        } else {
            state.processes.force_stop(&installation.id).await.map_err(|error| {
                AppError::Process(format!(
                    "failed to force-stop managed installation '{}' during app update handoff: {}",
                    installation.name, error
                ))
            })?;
        }
    }
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_process::init())
        .setup(|app| {
            #[cfg(desktop)]
            app.handle()
                .plugin(tauri_plugin_updater::Builder::new().build())
                .map_err(|e| -> Box<dyn std::error::Error> { Box::new(e) })?;

            let state = AppState::new(app.handle())?;
            app.manage(state);
            app.manage(app_updates::PendingUpdate::default());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            register_installation,
            save_installation,
            delete_installation,
            list_installations,
            get_installation_detail,
            reconcile_installation,
            list_manager_custom_nodes,
            adopt_tracked_custom_nodes,
            resolve_target,
            preview_repo_target,
            preview_tracked_repo_update,
            patch_core,
            install_or_patch_frontend,
            install_or_patch_custom_node,
            set_repo_base_target,
            add_repo_overlay,
            set_repo_overlay_enabled,
            remove_repo_overlay,
            move_repo_overlay,
            update_repo,
            update_all,
            rollback_repo,
            restore_checkpoint,
            compare_checkpoint,
            uninstall_repo,
            disable_repo,
            untrack_repo,
            start_installation,
            stop_installation,
            restart_installation,
            list_operations,
            get_operation_log,
            list_checkpoints,
            app_updates::fetch_app_update,
            app_updates::install_app_update,
        ])
        .build(tauri::generate_context!())
        .expect("error while building ComfyUI Patcher");

    app.run(|app_handle, event| {
        if let tauri::RunEvent::ExitRequested { .. } = event {
            let state = app_handle.state::<AppState>().inner().clone();
            tauri::async_runtime::block_on(async move {
                let _ = shutdown_managed_installations(state).await;
            });
        }
    });
}
