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
    apply_stash, canonicalize_remote, clone_repo, ensure_clean_or_apply_strategy, fetch_origin,
    fetch_refspec, inspect_repo, is_git_repo, join_custom_node_path, reset_hard, submodule_update,
    switch_branch, switch_detached, validate_custom_node_dir_name,
};
use crate::models::*;
use crate::state::AppState;
use crate::util::{detect_env_kind, infer_python};
use std::collections::{hash_map::Entry, HashMap, HashSet};
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Emitter, Manager, State};

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

async fn discover_repositories_for_installation(
    state: &AppState,
    installation: &Installation,
) -> AppResult<(Option<ManagedRepo>, Vec<ManagedRepo>)> {
    let root = PathBuf::from(&installation.comfy_root);
    let mut core_repo = None;
    let mut custom_node_repos = Vec::new();

    if is_git_repo(&root).await {
        let status = inspect_repo(&root).await?;
        core_repo = Some(state.db.upsert_repo(
            &installation.id,
            RepoKind::Core,
            "ComfyUI",
            &root.to_string_lossy(),
            status.origin_url.as_deref(),
            status.head_sha.as_deref(),
            status.branch.as_deref(),
            status.is_detached,
            status.is_dirty,
        )?);
    }

    let custom_nodes_dir = PathBuf::from(&installation.custom_nodes_dir);
    if custom_nodes_dir.exists() {
        for entry in std::fs::read_dir(&custom_nodes_dir)? {
            let entry = entry?;
            let path = entry.path();
            if !path.is_dir() || !is_git_repo(&path).await {
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
                &path.to_string_lossy(),
                status.origin_url.as_deref(),
                status.head_sha.as_deref(),
                status.branch.as_deref(),
                status.is_detached,
                status.is_dirty,
            )?;
            custom_node_repos.push(repo);
        }
    }

    let mut discovered_paths: HashSet<String> = HashSet::new();
    if let Some(repo) = core_repo.as_ref() {
        discovered_paths.insert(repo.local_path.clone());
    }
    for repo in &custom_node_repos {
        discovered_paths.insert(repo.local_path.clone());
    }

    for repo in state.db.list_repos_by_installation(&installation.id)? {
        if !discovered_paths.contains(&repo.local_path) {
            state.db.delete_repo(&repo.id)?;
        }
    }

    Ok((core_repo, custom_node_repos))
}

async fn discover_custom_node_repos_best_effort(
    state: &AppState,
    installation: &Installation,
) -> AppResult<Vec<ManagedRepo>> {
    let mut custom_node_repos = Vec::new();
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
        if !path.is_dir() || !is_git_repo(&path).await {
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
            &path.to_string_lossy(),
            status.origin_url.as_deref(),
            status.head_sha.as_deref(),
            status.branch.as_deref(),
            status.is_detached,
            status.is_dirty,
        )?;
        custom_node_repos.push(repo);
    }

    Ok(custom_node_repos)
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
        status.is_dirty,
    )?;
    Ok(())
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

async fn find_existing_custom_node_repo_by_remote(
    state: &AppState,
    installation: &Installation,
    resolved: &ResolvedTarget,
) -> AppResult<Option<ManagedRepo>> {
    let target_remote = canonical_target_remote(resolved)?;
    let mut matches: Vec<ManagedRepo> = Vec::new();

    let custom_nodes_dir = PathBuf::from(&installation.custom_nodes_dir);
    if custom_nodes_dir.exists() {
        for entry in std::fs::read_dir(&custom_nodes_dir)? {
            let entry = entry?;
            let path = entry.path();
            if !path.is_dir() || !is_git_repo(&path).await {
                continue;
            }
            let status = inspect_repo(&path).await?;
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
                status.is_dirty,
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
    repo: &ManagedRepo,
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
        return Err(AppError::Conflict(
            "repository has local changes".to_string(),
        ));
    }

    let checkpoint = state.db.create_checkpoint(
        &repo.id,
        operation_id,
        &head,
        status.branch.as_deref(),
        status.is_detached,
        false,
        None,
    )?;

    let stash_ref = ensure_clean_or_apply_strategy(path, strategy).await?;
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
    repo_path: &Path,
    enabled: bool,
) -> AppResult<()> {
    if !enabled {
        return Ok(());
    }
    let plan = plan_dependency_sync(installation, repo_path)?;
    log_operation(
        state,
        app,
        operation_id,
        "dependency_plan",
        "info",
        format!("dependency plan: {} ({})", plan.strategy, plan.reason),
    );
    if plan.strategy != "none" {
        log_operation(
            state,
            app,
            operation_id,
            "dependency_sync",
            "info",
            "running dependency sync",
        );
        execute_dependency_sync(&plan).await?;
    }
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
    let profile = installation
        .launch_profile
        .as_ref()
        .ok_or_else(|| AppError::Process("installation has no launch profile".to_string()))?;
    log_operation(
        state,
        app,
        operation_id,
        "restart",
        "info",
        "restarting installation",
    );
    state.processes.restart(&installation.id, profile).await?;
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
    let resolved = resolve_target_for_context(
        state,
        installation,
        &repo.kind,
        Some(&repo.id),
        target_input,
    )
    .await?;
    ensure_remote_matches(repo.canonical_remote.as_deref(), &resolved)?;
    let checkpoint =
        create_checkpoint_if_needed(state, repo, operation_id, dirty_repo_strategy).await?;
    log_operation(
        state,
        app,
        operation_id,
        "checkpoint",
        "info",
        format!("checkpoint {} created", checkpoint.id),
    );
    apply_resolved_target(state, app, operation_id, repo, &resolved).await?;
    maybe_sync_dependencies(
        state,
        app,
        operation_id,
        installation,
        Path::new(&repo.local_path),
        sync_dependencies,
    )
    .await?;
    refresh_repo_state(state, &repo.id).await?;
    if write_tracked_target {
        state.db.set_repo_tracked_target(
            &repo.id,
            Some(resolved.target_kind.clone()),
            Some(target_input),
            resolved.resolved_sha.as_deref(),
        )?;
    }
    Ok(checkpoint)
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
        detected_env_kind.as_deref(),
        is_root_git_repo,
    )?;
    let _repo_guards = if existing_installation.is_some() {
        acquire_installation_repo_locks(state, &installation.id).await?
    } else {
        Vec::new()
    };
    let (core_repo, discovered_custom_nodes) =
        discover_repositories_for_installation(state, &installation).await?;
    Ok(RegisterInstallationResult {
        installation,
        core_repo,
        discovered_custom_nodes,
        warnings: Vec::new(),
    })
}

#[tauri::command]
fn list_installations(state: State<'_, AppState>) -> Result<Vec<Installation>, String> {
    state.db.list_installations().map_err(|e| e.to_string())
}

#[tauri::command]
async fn get_installation_detail(
    state: State<'_, AppState>,
    installation_id: String,
) -> Result<InstallationDetail, String> {
    let mut detail = state
        .db
        .get_installation_detail(&installation_id)
        .map_err(|e| e.to_string())?;
    detail.is_running = state
        .processes
        .is_running(&installation_id)
        .await
        .map_err(|e| e.to_string())?;
    Ok(detail)
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
    let installation = state
        .db
        .update_installation(
            &existing.id,
            &input.name,
            &python_string,
            launch_profile.as_ref(),
            &detected_env_kind,
            is_git_repo(&root).await,
        )
        .map_err(|e| e.to_string())?;
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

    let discovered_custom_nodes = discover_custom_node_repos_best_effort(&state, &installation)
        .await
        .map_err(|e| e.to_string())?;

    let mut tracking_managed_dirs: HashMap<String, String> = HashMap::new();
    let mut present_non_git_dirs: HashMap<String, String> = HashMap::new();
    let custom_nodes_dir = PathBuf::from(&installation.custom_nodes_dir);
    if custom_nodes_dir.exists() {
        let entries = std::fs::read_dir(&custom_nodes_dir).map_err(|e| e.to_string())?;
        for entry in entries {
            let entry = entry.map_err(|e| e.to_string())?;
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let Some(dir_name) = path.file_name().and_then(|value| value.to_str()) else {
                continue;
            };
            let key = dir_name.to_ascii_lowercase();
            if is_git_repo(&path).await {
                continue;
            }
            if path.join(".tracking").is_file() {
                tracking_managed_dirs.insert(key, path.to_string_lossy().to_string());
            } else {
                present_non_git_dirs.insert(key, path.to_string_lossy().to_string());
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

    let limit = input.limit.unwrap_or(1000).clamp(1, 10000);
    let entries = state
        .manager_registry
        .search_entries(input.query.as_deref(), usize::MAX)
        .await
        .map_err(|e| e.to_string())?;
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
        let tracking_local_path = expected_dir_names
            .iter()
            .find_map(|value| tracking_managed_dirs.get(&value.to_ascii_lowercase()))
            .cloned();
        let present_local_path = expected_dir_names
            .iter()
            .find_map(|value| present_non_git_dirs.get(&value.to_ascii_lowercase()))
            .cloned();
        let installed = installed_state.and_then(|state| state.as_ref());
        let has_ambiguous_installation = matches!(installed_state, Some(None));
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
    let result = async {
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
                &state,
                &installation,
                &RepoKind::CustomNode,
                None,
                &input.input,
            )
            .await?;
            let custom_nodes_dir = PathBuf::from(&installation.custom_nodes_dir);
            let existing_repo_match =
                find_existing_custom_node_repo_by_remote(&state, &installation, &resolved).await?;
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
                    None => preferred_custom_node_dir_name(&state, &resolved).await,
                },
            };
            let base_dir_name = validate_custom_node_dir_name(&requested_dir_name)?;
            let mut target_path = existing_repo_match
                .as_ref()
                .map(|repo| PathBuf::from(&repo.local_path))
                .unwrap_or_else(|| join_custom_node_path(&custom_nodes_dir, &base_dir_name));

        if let Some(repo) = existing_repo_match.as_ref() {
            log_operation(
                &state,
                &app,
                &operation_id,
                "preflight",
                "info",
                format!("reusing existing custom node directory {}", repo.local_path),
            );
        } else {
            log_operation(
                &state,
                &app,
                &operation_id,
                "preflight",
                "info",
                format!("selected custom node directory {}", target_path.to_string_lossy()),
            );
        }

        if target_path.exists() {
            if is_git_repo(&target_path).await {
                let status = inspect_repo(&target_path).await?;
                ensure_remote_matches(status.origin_url.as_deref(), &resolved)?;
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
                    status.is_dirty,
                )?;
                let repo_lock = state.repo_lock(&repo.id).await;
                let _guard = repo_lock.lock().await;
                let checkpoint = create_checkpoint_if_needed(
                    &state,
                    &repo,
                    &operation_id,
                    &input.dirty_repo_strategy,
                )
                .await?;
                log_operation(
                    &state,
                    &app,
                    &operation_id,
                    "checkpoint",
                    "info",
                    format!("checkpoint {} created", checkpoint.id),
                );
                apply_resolved_target(&state, &app, &operation_id, &repo, &resolved).await?;
                maybe_sync_dependencies(
                    &state,
                    &app,
                    &operation_id,
                    &installation,
                    Path::new(&repo.local_path),
                    input.sync_dependencies,
                )
                .await?;
                refresh_repo_state(&state, &repo.id).await?;
                if input.set_tracked_target {
                    state.db.set_repo_tracked_target(
                        &repo.id,
                        Some(resolved.target_kind.clone()),
                        Some(&input.input),
                        resolved.resolved_sha.as_deref(),
                    )?;
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
                    "custom node patch completed",
                );
                return Ok::<(), AppError>(());
            }

            if is_tracking_managed_dir(&target_path).await {
                if !input.adopt_tracking_install {
                    return Err(AppError::Conflict(
                        "target custom node directory is a tracking-managed install; enable tracking adoption to replace it with a managed git clone"
                            .to_string(),
                    ));
                }
                let backup_root = PathBuf::from(&installation.comfy_root)
                    .join(".comfyui-patcher-backups")
                    .join("custom_nodes");
                std::fs::create_dir_all(&backup_root)?;
                let backup_path = choose_tracking_backup_path(&target_path, &backup_root);
                log_operation(
                    &state,
                    &app,
                    &operation_id,
                    "preflight",
                    "info",
                    format!(
                        "backing up tracking-managed install {} to {} before adopting it as a git repo",
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
            &state,
            &app,
            &operation_id,
            "clone",
            "info",
            format!("cloning {}", resolved.canonical_repo_url),
        );
        clone_repo(&resolved.fetch_url, &target_path).await?;
        let status = inspect_repo(&target_path).await?;
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
            status.is_dirty,
        )?;
        if tracking_backup_path.is_some() {
            rollback_repo_id = Some(repo.id.clone());
        }
        let repo_lock = state.repo_lock(&repo.id).await;
        let _guard = repo_lock.lock().await;
        let checkpoint =
            create_checkpoint_if_needed(&state, &repo, &operation_id, &DirtyRepoStrategy::Abort)
                .await?;
        log_operation(
            &state,
            &app,
            &operation_id,
            "checkpoint",
            "info",
            format!("checkpoint {} created", checkpoint.id),
        );
        if tracking_backup_path.is_some() {
            rollback_checkpoint_id = Some(checkpoint.id.clone());
        }
        apply_resolved_target(&state, &app, &operation_id, &repo, &resolved).await?;
        maybe_sync_dependencies(
            &state,
            &app,
            &operation_id,
            &installation,
            &target_path,
            input.sync_dependencies,
        )
        .await?;
        refresh_repo_state(&state, &repo.id).await?;
        if input.set_tracked_target {
            state.db.set_repo_tracked_target(
                &repo.id,
                Some(resolved.target_kind.clone()),
                Some(&input.input),
                resolved.resolved_sha.as_deref(),
            )?;
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
            "custom node install completed",
        );
        if let Some(backup_path) = tracking_backup_path.as_ref() {
            if let Err(remove_err) = std::fs::remove_dir_all(backup_path) {
                log_operation(
                    &state,
                    &app,
                    &operation_id,
                    "done",
                    "warn",
                    format!(
                        "managed install succeeded, but failed to remove tracking backup {}: {}",
                        backup_path.to_string_lossy(),
                        remove_err
                    ),
                );
            }
        }
        Ok::<(), AppError>(())
        }
        .await;

        if let Err(err) = result {
            if let (Some(backup_path), Some(target_path)) =
                (tracking_backup_path.as_ref(), tracking_target_path.as_ref())
            {
                if let Err(restore_err) = restore_tracking_backup(backup_path, target_path) {
                    log_operation(
                        &state,
                        &app,
                        &operation_id,
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
                        &state,
                        &app,
                        &operation_id,
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
    let op = state
        .db
        .create_operation(
            &installation.id,
            Some(&repo.id),
            OperationKind::UpdateRepo,
            repo.tracked_target_input.as_deref(),
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
        let tracked = repo
            .tracked_target_input
            .clone()
            .ok_or_else(|| AppError::InvalidInput("repo has no tracked target".to_string()))?;
        let checkpoint = apply_repo_target_update(
            &state,
            &app,
            &operation_id,
            &installation,
            &repo,
            &tracked,
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
        repos.extend(detail.custom_node_repos);
        let mut checkpoints = Vec::new();
        let mut failures = Vec::new();

        for repo in repos {
            if let Some(tracked) = repo.tracked_target_input.clone() {
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
                match apply_repo_target_update(
                    &state,
                    &app,
                    &operation_id,
                    &installation,
                    &repo,
                    &tracked,
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
    state.db.set_operation_running(&operation_id)?;
    let repo_lock = state.repo_lock(&repo.id).await;
    let _guard = repo_lock.lock().await;
    let result = async {
        let checkpoint = state
            .db
            .latest_checkpoint(&repo.id)?
            .ok_or_else(|| AppError::NotFound("no checkpoint available for repo".to_string()))?;
        log_operation(
            &state,
            &app,
            &operation_id,
            "checkpoint",
            "info",
            format!("restoring {}", checkpoint.old_head_sha),
        );
        let path = Path::new(&repo.local_path);
        if checkpoint.old_is_detached {
            switch_detached(path, &checkpoint.old_head_sha).await?;
        } else if let Some(branch) = checkpoint.old_branch.as_deref() {
            switch_branch(path, branch, Some(&checkpoint.old_head_sha)).await?;
        } else {
            switch_detached(path, &checkpoint.old_head_sha).await?;
        }
        reset_hard(path, &checkpoint.old_head_sha).await?;
        let _ = submodule_update(path).await;
        if input.restore_stash && checkpoint.stash_created {
            let stash_ref = checkpoint.stash_ref.as_deref().ok_or_else(|| {
                AppError::Git(
                    "checkpoint indicates a stash was created but no stash reference was stored"
                        .to_string(),
                )
            })?;
            if let Err(err) = apply_stash(path, stash_ref).await {
                log_operation(
                    &state,
                    &app,
                    &operation_id,
                    "stash",
                    "error",
                    format!("failed to restore stash: {err}"),
                );
                return Err(AppError::Git(format!("failed to restore stash: {err}")));
            }
        }
        maybe_sync_dependencies(
            &state,
            &app,
            &operation_id,
            &installation,
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
async fn start_installation(
    app: AppHandle,
    state: State<'_, AppState>,
    input: StartInstallationInput,
) -> Result<OperationStart, String> {
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
    let result = async {
        let profile = installation
            .launch_profile
            .as_ref()
            .ok_or_else(|| AppError::Process("installation has no launch profile".to_string()))?;
        log_operation(
            &state,
            &app,
            &operation_id,
            "start",
            "info",
            "starting installation",
        );
        state.processes.start(&installation.id, profile).await?;
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
    let result = async {
        let profile = installation
            .launch_profile
            .as_ref()
            .ok_or_else(|| AppError::Process("installation has no launch profile".to_string()))?;
        log_operation(
            &state,
            &app,
            &operation_id,
            "stop",
            "info",
            "stopping installation",
        );
        let stopped = state.processes.stop(&installation.id, profile).await?;
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
    let result = async {
        let profile = installation
            .launch_profile
            .as_ref()
            .ok_or_else(|| AppError::Process("installation has no launch profile".to_string()))?;
        log_operation(
            &state,
            &app,
            &operation_id,
            "restart",
            "info",
            "restarting installation",
        );
        state.processes.restart(&installation.id, profile).await?;
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        .setup(|app| {
            let state = AppState::new(app.handle())?;
            app.manage(state);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            register_installation,
            save_installation,
            delete_installation,
            list_installations,
            get_installation_detail,
            list_manager_custom_nodes,
            resolve_target,
            patch_core,
            install_or_patch_custom_node,
            update_repo,
            update_all,
            rollback_repo,
            start_installation,
            stop_installation,
            restart_installation,
            list_operations,
            get_operation_log,
            list_checkpoints,
        ])
        .build(tauri::generate_context!())
        .expect("error while building ComfyUI Patcher");

    app.run(|app_handle, event| {
        if let tauri::RunEvent::ExitRequested { .. } = event {
            let state = app_handle.state::<AppState>().inner().clone();
            tauri::async_runtime::block_on(async move {
                state.processes.shutdown_all().await;
            });
        }
    });
}
