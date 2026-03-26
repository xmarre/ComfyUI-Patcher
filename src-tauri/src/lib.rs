mod db;
mod deps;
mod errors;
mod git;
mod github;
mod models;
mod process;
mod state;
mod util;

use crate::deps::{execute_dependency_sync, plan_dependency_sync};
use crate::errors::{AppError, AppResult};
use crate::git::{
    apply_stash, clone_repo, ensure_clean_or_apply_strategy, fetch_origin, fetch_refspec,
    inspect_repo, is_git_repo, join_custom_node_path, reset_hard, submodule_update, switch_branch,
    switch_detached, validate_custom_node_dir_name,
};
use crate::models::*;
use crate::state::AppState;
use crate::util::{detect_env_kind, infer_python};
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

    Ok((core_repo, custom_node_repos))
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
    if let Some(current) = current {
        if current != target.canonical_repo_url {
            return Err(AppError::Conflict(format!(
                "resolved target repo {} does not match managed repo remote {}",
                target.canonical_repo_url, current
            )));
        }
    }
    Ok(())
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
    let plan = plan_dependency_sync(installation, repo_path);
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
    let root = PathBuf::from(&input.comfy_root);
    if !root.exists() || !root.is_dir() {
        return Err(AppError::InvalidInput(
            "ComfyUI root does not exist or is not a directory".to_string(),
        ));
    }

    let python = match input.python_exe {
        Some(value) if !value.trim().is_empty() => PathBuf::from(value),
        _ => infer_python(&root).unwrap_or_else(|| PathBuf::from("python")),
    };

    let custom_nodes_dir = root.join("custom_nodes");
    std::fs::create_dir_all(&custom_nodes_dir)?;

    let installation = state.db.create_installation(
        &input.name,
        &root.to_string_lossy(),
        &python.to_string_lossy(),
        &custom_nodes_dir.to_string_lossy(),
        input.launch_profile.as_ref(),
        &detect_env_kind(&python),
        is_git_repo(&root).await,
    )?;
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
fn get_installation_detail(
    state: State<'_, AppState>,
    installation_id: String,
) -> Result<InstallationDetail, String> {
    state
        .db
        .get_installation_detail(&installation_id)
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
    log_operation(
        &state,
        &app,
        &operation_id,
        "preflight",
        "info",
        "installing or patching custom node",
    );
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
        let requested_dir_name = input
            .target_local_dir_name
            .clone()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| resolved.suggested_local_dir_name.clone());
        let base_dir_name = validate_custom_node_dir_name(&requested_dir_name)?;
        let mut target_path = join_custom_node_path(&custom_nodes_dir, &base_dir_name);

        if target_path.exists() {
            if is_git_repo(&target_path).await {
                let status = inspect_repo(&target_path).await?;
                if let Some(origin) = status.origin_url.as_deref() {
                    ensure_remote_matches(Some(origin), &resolved)?;
                }
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
    tauri::Builder::default()
        .setup(|app| {
            let state = AppState::new(app.handle())?;
            app.manage(state);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            register_installation,
            list_installations,
            get_installation_detail,
            resolve_target,
            patch_core,
            install_or_patch_custom_node,
            update_repo,
            update_all,
            rollback_repo,
            restart_installation,
            list_operations,
            get_operation_log,
            list_checkpoints,
        ])
        .run(tauri::generate_context!())
        .expect("error while running ComfyUI Patcher");
}
