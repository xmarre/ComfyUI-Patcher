from pathlib import Path

path = Path("src-tauri/src/lib.rs")
text = path.read_text()


def replace_once(old: str, new: str, label: str) -> None:
    global text
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{label}: expected exactly one match, found {count}")
    text = text.replace(old, new, 1)


def replace_count(old: str, new: str, expected: int, label: str) -> None:
    global text
    count = text.count(old)
    if count != expected:
        raise SystemExit(f"{label}: expected {expected} matches, found {count}")
    text = text.replace(old, new)


replace_once(
    '''            RepoKind::Frontend => {
                state
                    .db
                    .get_installation_detail(&installation.id)?
                    .frontend_repo
            }
            RepoKind::CustomNode => None,
''',
    '''            RepoKind::Frontend => {
                state
                    .db
                    .get_installation_detail(&installation.id)?
                    .frontend_repo
            }
            RepoKind::Kitchen => {
                state
                    .db
                    .get_installation_detail(&installation.id)?
                    .kitchen_repo
            }
            RepoKind::CustomNode => None,
''',
    "Kitchen target context",
)

replace_once(
    '''            RepoKind::Core => "ComfyUI".to_string(),
            RepoKind::Frontend => "ComfyUI Frontend".to_string(),
            RepoKind::CustomNode => resolved.suggested_local_dir_name.clone(),
''',
    '''            RepoKind::Core => "ComfyUI".to_string(),
            RepoKind::Frontend => "ComfyUI Frontend".to_string(),
            RepoKind::Kitchen => "Comfy Kitchen".to_string(),
            RepoKind::CustomNode => resolved.suggested_local_dir_name.clone(),
''',
    "Kitchen preview label",
)

replace_once(
    "        if repo.materialization_state.is_some() || repo.tracked_state.is_some() {\n",
    "        if repo.materialization_state.is_some() {\n",
    "Kitchen reconciliation activation semantics",
)

replace_once(
    '''    if restore_errors.is_empty() {
        Ok(())
    } else {
        Err(AppError::Git(restore_errors.join("; ")))
    }
}

fn update_overlay_from_resolved(
''',
    '''    if let Err(err) = state
        .db
        .set_repo_materialization_state(repo_id, checkpoint.materialization_state.as_ref())
    {
        restore_errors.push(format!("failed to restore project materialization metadata: {err}"));
    }

    if restore_errors.is_empty() {
        Ok(())
    } else {
        Err(AppError::Git(restore_errors.join("; ")))
    }
}

fn update_overlay_from_resolved(
''',
    "checkpoint materialization metadata restore",
)

helpers = r'''
async fn materialize_kitchen_override(
    state: &AppState,
    app: &AppHandle,
    operation_id: &str,
    installation: &Installation,
    repo: &ManagedRepo,
) -> AppResult<RepoMaterializationState> {
    if repo.kind != RepoKind::Kitchen {
        return Err(AppError::InvalidInput(
            "Kitchen materialization requires a Kitchen repository".to_string(),
        ));
    }
    let path = Path::new(&repo.local_path);
    log_operation(
        state,
        app,
        operation_id,
        "submodules",
        "info",
        "initializing required Comfy Kitchen submodules",
    );
    submodule_update(path).await.map_err(|error| {
        AppError::Dependency(format!(
            "Comfy Kitchen submodule initialization failed; source materialization was not attempted: {error}"
        ))
    })?;
    log_operation(
        state,
        app,
        operation_id,
        "materialization",
        "info",
        "building Comfy Kitchen from the managed source checkout before installation",
    );
    match kitchen::materialize_kitchen_project(installation, repo).await {
        Ok(materialization) => {
            state
                .db
                .set_repo_materialization_state(&repo.id, Some(&materialization))?;
            let runtime = kitchen::probe_kitchen_runtime(installation).await?;
            state.db.set_kitchen_runtime(&installation.id, &runtime)?;
            log_operation(
                state,
                app,
                operation_id,
                "materialization",
                "info",
                format!(
                    "Comfy Kitchen source materialized at {}",
                    materialization
                        .materialized_head_sha
                        .as_deref()
                        .unwrap_or("unknown HEAD")
                ),
            );
            Ok(materialization)
        }
        Err(error) => {
            let failed = kitchen::failed_materialization_state(
                repo.materialization_state.as_ref(),
                &error.to_string(),
            );
            let _ = state
                .db
                .set_repo_materialization_state(&repo.id, Some(&failed));
            Err(error)
        }
    }
}

async fn restore_comfy_requirement_for_repo(
    state: &AppState,
    app: &AppHandle,
    operation_id: &str,
    installation: &Installation,
    repo: &ManagedRepo,
) -> AppResult<()> {
    if repo.kind != RepoKind::Kitchen {
        return Err(AppError::InvalidInput(
            "ComfyUI-managed Kitchen restore requires a Kitchen repository".to_string(),
        ));
    }
    log_operation(
        state,
        app,
        operation_id,
        "materialization",
        "info",
        "restoring the comfy-kitchen requirement declared by the current ComfyUI checkout",
    );
    let runtime = kitchen::restore_comfy_managed_kitchen(installation).await?;
    state.db.set_kitchen_runtime(&installation.id, &runtime)?;
    state.db.set_repo_materialization_state(&repo.id, None)?;
    Ok(())
}

async fn reassert_managed_kitchen_override_if_needed(
    state: &AppState,
    app: &AppHandle,
    operation_id: &str,
    installation: &Installation,
) -> AppResult<()> {
    let detail = state.db.get_installation_detail(&installation.id)?;
    let Some(mut kitchen_repo) = detail.kitchen_repo else {
        return Ok(());
    };
    let Some(saved_materialization) = kitchen_repo.materialization_state.clone() else {
        return Ok(());
    };
    let path = Path::new(&kitchen_repo.local_path);
    if !path.exists() || !has_git_marker(path) || !is_git_repo(path).await {
        return Err(AppError::Conflict(
            "Patcher-managed Comfy Kitchen override is active, but its source checkout is missing or is no longer a git repository"
                .to_string(),
        ));
    }
    let status = inspect_repo(path).await?;
    if status.is_dirty {
        return Err(AppError::Conflict(
            "Patcher-managed Comfy Kitchen override was replaced by another dependency operation, but the Kitchen source checkout is dirty; refusing to rebuild from uncommitted source"
                .to_string(),
        ));
    }
    kitchen_repo.current_head_sha = status.head_sha.clone();
    kitchen_repo.current_branch = status.branch.clone();
    kitchen_repo.is_detached = status.is_detached;
    kitchen_repo.is_dirty = false;
    let runtime = kitchen::probe_kitchen_runtime(installation).await?;
    state.db.set_kitchen_runtime(&installation.id, &runtime)?;
    let evaluated = kitchen::evaluate_materialization(&kitchen_repo, &runtime);
    match evaluated.status {
        MaterializationStatus::Current => Ok(()),
        MaterializationStatus::Replaced
        | MaterializationStatus::Missing
        | MaterializationStatus::ImportFailed => {
            if status.head_sha.as_deref()
                != saved_materialization.materialized_head_sha.as_deref()
            {
                return Err(AppError::Conflict(
                    "Comfy Kitchen runtime was replaced, but the Kitchen source checkout has also moved since the last successful materialization; update or repair Kitchen explicitly before continuing"
                        .to_string(),
                ));
            }
            log_operation(
                state,
                app,
                operation_id,
                "materialization",
                "warn",
                "another Patcher-controlled dependency install replaced the active Comfy Kitchen source build; reasserting the managed source override",
            );
            materialize_kitchen_override(
                state,
                app,
                operation_id,
                installation,
                &kitchen_repo,
            )
            .await?;
            Ok(())
        }
        MaterializationStatus::Stale | MaterializationStatus::Failed => Err(AppError::Conflict(
            "Patcher-managed Comfy Kitchen runtime is not coherent with its source checkout; repair or update Kitchen explicitly before continuing"
                .to_string(),
        )),
    }
}

async fn ensure_kitchen_runtime_ready_for_launch(
    state: &AppState,
    installation: &Installation,
) -> AppResult<()> {
    let detail = state.db.get_installation_detail(&installation.id)?;
    let Some(mut kitchen_repo) = detail.kitchen_repo else {
        return Ok(());
    };
    if kitchen_repo.materialization_state.is_none() {
        return Ok(());
    }
    let path = Path::new(&kitchen_repo.local_path);
    if !path.exists() || !has_git_marker(path) || !is_git_repo(path).await {
        return Err(AppError::Process(
            "cannot start ComfyUI: the active Patcher-managed Comfy Kitchen source checkout is missing or invalid"
                .to_string(),
        ));
    }
    let status = inspect_repo(path).await?;
    kitchen_repo.current_head_sha = status.head_sha;
    kitchen_repo.current_branch = status.branch;
    kitchen_repo.is_detached = status.is_detached;
    kitchen_repo.is_dirty = !status.tracked_changed_files.is_empty();
    let runtime = kitchen::probe_kitchen_runtime(installation).await.map_err(|error| {
        AppError::Process(format!(
            "cannot start ComfyUI: failed to inspect active Patcher-managed Comfy Kitchen runtime: {error}"
        ))
    })?;
    state.db.set_kitchen_runtime(&installation.id, &runtime)?;
    let evaluated = kitchen::evaluate_materialization(&kitchen_repo, &runtime);
    if evaluated.status != MaterializationStatus::Current {
        return Err(AppError::Process(format!(
            "cannot start ComfyUI: active Patcher-managed Comfy Kitchen runtime is {}",
            serde_json::to_string(&evaluated.status)
                .unwrap_or_else(|_| "inconsistent".to_string())
                .trim_matches('"')
        )));
    }
    Ok(())
}

async fn restore_checkpoint_with_kitchen_runtime(
    state: &AppState,
    app: &AppHandle,
    operation_id: &str,
    installation: &Installation,
    repo: &ManagedRepo,
    checkpoint: &RepoCheckpoint,
    restore_stash: bool,
) -> AppResult<()> {
    let path = Path::new(&repo.local_path);
    if repo.kind != RepoKind::Kitchen {
        return restore_checkpoint_state(
            state,
            path,
            &repo.id,
            checkpoint,
            restore_stash,
        )
        .await;
    }

    restore_checkpoint_state(state, path, &repo.id, checkpoint, false).await?;
    if let Some(saved_materialization) = checkpoint.materialization_state.as_ref() {
        let status = inspect_repo(path).await?;
        if status.head_sha.as_deref()
            != saved_materialization.materialized_head_sha.as_deref()
        {
            return Err(AppError::Conflict(format!(
                "cannot restore Comfy Kitchen runtime provenance exactly: checkpoint source HEAD {} differs from saved materialized HEAD {}",
                status.head_sha.as_deref().unwrap_or("unknown"),
                saved_materialization
                    .materialized_head_sha
                    .as_deref()
                    .unwrap_or("none")
            )));
        }
        if status.is_dirty {
            return Err(AppError::Conflict(
                "cannot restore Comfy Kitchen runtime from a dirty checkpoint worktree before stash reapplication"
                    .to_string(),
            ));
        }
        let mut restored_repo = repo.clone();
        restored_repo.current_head_sha = status.head_sha;
        restored_repo.current_branch = status.branch;
        restored_repo.is_detached = status.is_detached;
        restored_repo.is_dirty = false;
        restored_repo.materialization_state = Some(saved_materialization.clone());
        materialize_kitchen_override(
            state,
            app,
            operation_id,
            installation,
            &restored_repo,
        )
        .await?;
    } else {
        restore_comfy_requirement_for_repo(
            state,
            app,
            operation_id,
            installation,
            repo,
        )
        .await?;
    }

    if restore_stash && checkpoint.stash_created {
        let stash_ref = checkpoint.stash_ref.as_deref().ok_or_else(|| {
            AppError::Git(
                "checkpoint indicates a stash was created but no stash reference was stored"
                    .to_string(),
            )
        })?;
        apply_stash(path, stash_ref).await?;
    }
    Ok(())
}

'''
replace_once(
    "async fn maybe_sync_dependencies(\n",
    helpers + "async fn maybe_sync_dependencies(\n",
    "Kitchen orchestration helpers",
)

old_maybe_sync_start = text.index("async fn maybe_sync_dependencies(\n")
old_maybe_sync_end = text.index("\nasync fn maybe_restart_installation(\n", old_maybe_sync_start)
maybe_sync_replacement = r'''async fn maybe_sync_dependencies(
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
    if !plan.steps.is_empty() {
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
    }
    if matches!(repo.kind, RepoKind::Core | RepoKind::CustomNode) {
        reassert_managed_kitchen_override_if_needed(
            state,
            app,
            operation_id,
            installation,
        )
        .await?;
    }
    Ok(())
}
'''
text = text[:old_maybe_sync_start] + maybe_sync_replacement + text[old_maybe_sync_end:]

replace_once(
    '''        maybe_sync_dependencies(
            state,
            app,
            operation_id,
            installation,
            repo,
            path,
            sync_dependencies,
        )
        .await?;
        cleanup_frontend_dependency_artifacts(
            state,
            app,
            operation_id,
            repo,
            path,
            sync_dependencies,
        )
        .await?;
        ensure_repo_clean_after_patcher_mutation(path, "patcher-controlled checkout materialization")
            .await?;
''',
    '''        if repo.kind == RepoKind::Kitchen {
            materialize_kitchen_override(
                state,
                app,
                operation_id,
                installation,
                repo,
            )
            .await?;
        } else {
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
            cleanup_frontend_dependency_artifacts(
                state,
                app,
                operation_id,
                repo,
                path,
                sync_dependencies,
            )
            .await?;
        }
        ensure_repo_clean_after_patcher_mutation(path, "patcher-controlled checkout materialization")
            .await?;
''',
    "mandatory Kitchen project materialization",
)

replace_once(
    '''            let restore_result =
                restore_checkpoint_state(state, path, &repo.id, &checkpoint, true).await;
''',
    '''            let restore_result = restore_checkpoint_with_kitchen_runtime(
                state,
                app,
                operation_id,
                installation,
                repo,
                &checkpoint,
                true,
            )
            .await;
''',
    "failed forward Kitchen recovery",
)

replace_once(
    '''        restore_checkpoint_state(
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
        cleanup_frontend_dependency_artifacts(
            &state,
            &app,
            &operation_id,
            &repo,
            path,
            input.sync_dependencies,
        )
        .await?;
''',
    '''        restore_checkpoint_with_kitchen_runtime(
            &state,
            &app,
            &operation_id,
            &installation,
            &repo,
            &checkpoint,
            input.restore_stash,
        )
        .await?;
        if repo.kind != RepoKind::Kitchen {
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
            cleanup_frontend_dependency_artifacts(
                &state,
                &app,
                &operation_id,
                &repo,
                path,
                input.sync_dependencies,
            )
            .await?;
        }
''',
    "Kitchen rollback runtime restore",
)

replace_once(
    '''        restore_checkpoint_state(&state, path, &repo.id, &checkpoint, input.restore_stash).await?;
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
        cleanup_frontend_dependency_artifacts(
            &state,
            &app,
            &operation_id,
            &repo,
            path,
            input.sync_dependencies,
        )
        .await?;
''',
    '''        restore_checkpoint_with_kitchen_runtime(
            &state,
            &app,
            &operation_id,
            &installation,
            &repo,
            &checkpoint,
            input.restore_stash,
        )
        .await?;
        if repo.kind != RepoKind::Kitchen {
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
            cleanup_frontend_dependency_artifacts(
                &state,
                &app,
                &operation_id,
                &repo,
                path,
                input.sync_dependencies,
            )
            .await?;
        }
''',
    "Kitchen checkpoint runtime restore",
)

replace_count(
    '''    let result = async {
        let require_managed_frontend_dist = state
''',
    '''    let result = async {
        ensure_kitchen_runtime_ready_for_launch(&state, &installation).await?;
        let require_managed_frontend_dist = state
''',
    2,
    "Start and Restart Kitchen fail-fast",
)

replace_once(
    '''async fn run_start_installation(
    app: AppHandle,
    state: AppState,
    installation: Installation,
    operation_id: String,
) -> AppResult<()> {
    state.db.set_operation_running(&operation_id)?;
''',
    '''async fn run_start_installation(
    app: AppHandle,
    state: AppState,
    installation: Installation,
    operation_id: String,
) -> AppResult<()> {
    let _background_work_guard = acquire_background_work_guard(&state).await;
    state.db.set_operation_running(&operation_id)?;
''',
    "serialize Start with project materialization",
)

replace_once(
    '''async fn run_restart_installation(
    app: AppHandle,
    state: AppState,
    installation: Installation,
    operation_id: String,
) -> AppResult<()> {
    state.db.set_operation_running(&operation_id)?;
''',
    '''async fn run_restart_installation(
    app: AppHandle,
    state: AppState,
    installation: Installation,
    operation_id: String,
) -> AppResult<()> {
    let _background_work_guard = acquire_background_work_guard(&state).await;
    state.db.set_operation_running(&operation_id)?;
''',
    "serialize Restart with project materialization",
)

replace_once(
    '''    let require_managed_frontend_dist = state
        .db
        .get_installation_detail(&installation.id)?
        .frontend_repo
        .is_some();
    let profile = effective_launch_profile(installation, require_managed_frontend_dist)?;
''',
    '''    ensure_kitchen_runtime_ready_for_launch(state, installation).await?;
    let require_managed_frontend_dist = state
        .db
        .get_installation_detail(&installation.id)?
        .frontend_repo
        .is_some();
    let profile = effective_launch_profile(installation, require_managed_frontend_dist)?;
''',
    "restart-after-success Kitchen fail-fast",
)

kitchen_commands = r'''
#[tauri::command]
async fn install_or_patch_kitchen(
    app: AppHandle,
    state: State<'_, AppState>,
    input: PatchKitchenInput,
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
    let operation_kind = if detail.kitchen_repo.is_some() {
        OperationKind::PatchKitchen
    } else {
        OperationKind::InstallKitchen
    };
    let background_work_lock = state.background_work_lock();
    let _background_work_accept_guard = background_work_lock.lock().await;
    let op = state
        .db
        .create_operation(
            &installation.id,
            detail.kitchen_repo.as_ref().map(|repo| repo.id.as_str()),
            operation_kind,
            Some(&input.input),
        )
        .map_err(|e| e.to_string())?;
    let op_id = op.id.clone();
    let state_handle = app.state::<AppState>().inner().clone();
    tauri::async_runtime::spawn(async move {
        let _ = run_install_or_patch_kitchen(app, state_handle, installation, input, op_id).await;
    });
    Ok(OperationStart {
        operation_id: op.id,
    })
}

async fn run_install_or_patch_kitchen(
    app: AppHandle,
    state: AppState,
    installation: Installation,
    input: PatchKitchenInput,
    operation_id: String,
) -> AppResult<()> {
    let _background_work_guard = acquire_background_work_guard(&state).await;
    state.db.set_operation_running(&operation_id)?;
    let installation_lock = state.installation_lock(&installation.id).await;
    let _installation_guard = installation_lock.lock().await;
    let result = async {
        let target_path = kitchen::default_kitchen_path(&installation);
        let detail = state.db.get_installation_detail(&installation.id)?;
        let resolved = resolve_target_for_context(
            &state,
            &installation,
            &RepoKind::Kitchen,
            detail.kitchen_repo.as_ref().map(|repo| repo.id.as_str()),
            &input.input,
        )
        .await?;
        let resolved_remote = canonical_target_remote(&resolved)?;
        let expected_remote = canonicalize_remote(kitchen::DEFAULT_KITCHEN_REPO_URL)
            .ok_or_else(|| AppError::InvalidInput("invalid built-in Comfy Kitchen repository URL".to_string()))?;
        if resolved_remote != expected_remote {
            return Err(AppError::Conflict(format!(
                "Comfy Kitchen source management only accepts targets from {}; resolved target belongs to {}",
                expected_remote, resolved_remote
            )));
        }

        let mut replaced_backup_path: Option<PathBuf> = None;
        let mut created_repo_id: Option<String> = None;
        let mut created_new_checkout = false;

        let apply_existing = async {
            if !target_path.exists() || !is_git_repo(&target_path).await {
                return Ok::<Option<RepoCheckpoint>, AppError>(None);
            }
            let status = inspect_repo(&target_path).await?;
            let existing_remote = status.origin_url.as_deref().and_then(canonicalize_remote);
            if existing_remote.as_deref() != Some(expected_remote.as_str()) {
                return Ok(None);
            }
            state
                .db
                .unignore_repo_path(&installation.id, &target_path.to_string_lossy())?;
            let repo = state.db.upsert_repo(
                &installation.id,
                RepoKind::Kitchen,
                "Comfy Kitchen",
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
                false,
                input.set_tracked_target,
            )
            .await?;
            Ok(Some(checkpoint))
        }
        .await?;

        let checkpoint = if let Some(checkpoint) = apply_existing {
            checkpoint
        } else {
            if target_path.exists() {
                if detail
                    .kitchen_repo
                    .as_ref()
                    .is_some_and(|repo| repo.local_path == target_path.to_string_lossy())
                {
                    return Err(AppError::Conflict(
                        "the configured Kitchen source path is already tracked but does not match the official Comfy Kitchen remote; repair or untrack it before replacement"
                            .to_string(),
                    ));
                }
                match input.existing_repo_conflict_strategy {
                    ExistingRepoConflictStrategy::Abort => {
                        return Err(AppError::Conflict(
                            "the managed Comfy Kitchen source path already exists and is not the expected repository"
                                .to_string(),
                        ));
                    }
                    ExistingRepoConflictStrategy::InstallWithSuffix => {
                        return Err(AppError::Conflict(
                            "the managed Comfy Kitchen source path is fixed; Install with suffix is not supported"
                                .to_string(),
                        ));
                    }
                    ExistingRepoConflictStrategy::Replace => {
                        let backup_root = installation_retained_backup_root(&installation, "kitchen");
                        std::fs::create_dir_all(&backup_root)?;
                        let backup_path = choose_tracking_backup_path(&target_path, &backup_root);
                        std::fs::rename(&target_path, &backup_path)?;
                        replaced_backup_path = Some(backup_path);
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
            created_new_checkout = true;
            let status = inspect_repo(&target_path).await?;
            state
                .db
                .unignore_repo_path(&installation.id, &target_path.to_string_lossy())?;
            let repo = state.db.upsert_repo(
                &installation.id,
                RepoKind::Kitchen,
                "Comfy Kitchen",
                &target_path.to_string_lossy(),
                status.origin_url.as_deref(),
                status.head_sha.as_deref(),
                status.branch.as_deref(),
                status.is_detached,
                repo_has_tracked_local_changes(&status),
            )?;
            created_repo_id = Some(repo.id.clone());
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
            apply_repo_tracking_state(
                &state,
                &app,
                &operation_id,
                &installation,
                &repo,
                &tracked_state,
                &DirtyRepoStrategy::Abort,
                false,
                input.set_tracked_target,
            )
            .await?
        };

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
        if let Some(backup) = replaced_backup_path.as_ref() {
            log_operation(
                &state,
                &app,
                &operation_id,
                "done",
                "info",
                format!(
                    "replaced path was preserved at {}; review it before deleting the backup",
                    backup.to_string_lossy()
                ),
            );
        }
        log_operation(
            &state,
            &app,
            &operation_id,
            "done",
            "info",
            "Comfy Kitchen source install/patch completed",
        );
        Ok::<(), AppError>(())
    }
    .await;

    if let Err(err) = result {
        let target_path = kitchen::default_kitchen_path(&installation);
        let detail = state.db.get_installation_detail(&installation.id).ok();
        if detail
            .as_ref()
            .and_then(|detail| detail.kitchen_repo.as_ref())
            .is_some_and(|repo| repo.materialization_state.is_none())
        {
            // A failed first source install must not leave a partially installed wheel.
            if let Some(repo) = detail.and_then(|detail| detail.kitchen_repo) {
                let _ = restore_comfy_requirement_for_repo(
                    &state,
                    &app,
                    &operation_id,
                    &installation,
                    &repo,
                )
                .await;
            }
        }
        // A failed fresh clone is not a managed source checkout. Keep retained
        // replacement backups, but remove only the checkout created by this operation.
        if target_path.exists() {
            let status = inspect_repo(&target_path).await.ok();
            let official = canonicalize_remote(kitchen::DEFAULT_KITCHEN_REPO_URL);
            if status
                .as_ref()
                .and_then(|status| status.origin_url.as_deref())
                .and_then(canonicalize_remote)
                == official
            {
                let _ = remove_path_with_retries(&target_path).await;
            }
        }
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
async fn restore_comfy_managed_kitchen(
    app: AppHandle,
    state: State<'_, AppState>,
    input: RestoreComfyManagedKitchenInput,
) -> Result<OperationStart, String> {
    let repo = state
        .db
        .get_repo(&input.repo_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "repo not found".to_string())?;
    if repo.kind != RepoKind::Kitchen {
        return Err("selected repo is not Comfy Kitchen".to_string());
    }
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
            OperationKind::RestoreComfyManagedKitchen,
            None,
        )
        .map_err(|e| e.to_string())?;
    let op_id = op.id.clone();
    let state_handle = app.state::<AppState>().inner().clone();
    tauri::async_runtime::spawn(async move {
        let _ = run_restore_comfy_managed_kitchen(
            app,
            state_handle,
            installation,
            repo,
            input,
            op_id,
        )
        .await;
    });
    Ok(OperationStart {
        operation_id: op.id,
    })
}

async fn run_restore_comfy_managed_kitchen(
    app: AppHandle,
    state: AppState,
    installation: Installation,
    repo: ManagedRepo,
    input: RestoreComfyManagedKitchenInput,
    operation_id: String,
) -> AppResult<()> {
    let _background_work_guard = acquire_background_work_guard(&state).await;
    state.db.set_operation_running(&operation_id)?;
    let repo_lock = state.repo_lock(&repo.id).await;
    let _guard = repo_lock.lock().await;
    let result = async {
        restore_comfy_requirement_for_repo(
            &state,
            &app,
            &operation_id,
            &installation,
            &repo,
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
        state
            .db
            .finish_operation(&operation_id, OperationStatus::Succeeded, None, None)?;
        log_operation(
            &state,
            &app,
            &operation_id,
            "done",
            "info",
            "restored the ComfyUI-managed comfy-kitchen requirement",
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

'''
replace_once(
    "struct CustomNodeInstallRunResult {\n",
    kitchen_commands + "struct CustomNodeInstallRunResult {\n",
    "Kitchen install and restore commands",
)

replace_once(
    "        let _ = run_uninstall_repo(app, state_handle, repo, op_id).await;\n",
    "        let _ = run_uninstall_repo(app, state_handle, installation, repo, op_id).await;\n",
    "pass installation to uninstall",
)
replace_once(
    '''async fn run_uninstall_repo(
    app: AppHandle,
    state: AppState,
    repo: ManagedRepo,
    operation_id: String,
) -> AppResult<()> {
''',
    '''async fn run_uninstall_repo(
    app: AppHandle,
    state: AppState,
    installation: Installation,
    repo: ManagedRepo,
    operation_id: String,
) -> AppResult<()> {
''',
    "uninstall signature",
)
replace_once(
    '''        if path.exists() {
            remove_path_with_retries(&path).await?;
        }
        state.db.delete_repo(&repo.id)?;
''',
    '''        if repo.kind == RepoKind::Kitchen && repo.materialization_state.is_some() {
            restore_comfy_requirement_for_repo(
                &state,
                &app,
                &operation_id,
                &installation,
                &repo,
            )
            .await?;
        }
        if path.exists() {
            remove_path_with_retries(&path).await?;
        }
        state.db.delete_repo(&repo.id)?;
''',
    "Kitchen uninstall restores core requirement",
)

replace_once(
    '''        if path.exists() {
            std::fs::rename(&path, &disabled_path)?;
        }
        state.db.delete_repo(&repo.id)?;
''',
    '''        if repo.kind == RepoKind::Kitchen && repo.materialization_state.is_some() {
            restore_comfy_requirement_for_repo(
                &state,
                &app,
                &operation_id,
                &installation,
                &repo,
            )
            .await?;
        }
        if path.exists() {
            std::fs::rename(&path, &disabled_path)?;
        }
        state.db.delete_repo(&repo.id)?;
''',
    "Kitchen disable restores core requirement",
)

replace_once(
    "        let _ = run_untrack_repo(app, state_handle, repo, op_id).await;\n",
    "        let _ = run_untrack_repo(app, state_handle, installation, repo, op_id).await;\n",
    "pass installation to untrack",
)
replace_once(
    '''async fn run_untrack_repo(
    app: AppHandle,
    state: AppState,
    repo: ManagedRepo,
    operation_id: String,
) -> AppResult<()> {
''',
    '''async fn run_untrack_repo(
    app: AppHandle,
    state: AppState,
    installation: Installation,
    repo: ManagedRepo,
    operation_id: String,
) -> AppResult<()> {
''',
    "untrack signature",
)
replace_once(
    '''        state
            .db
            .ignore_repo_path(&repo.installation_id, &repo.kind, &repo.local_path)?;
''',
    '''        if repo.kind == RepoKind::Kitchen && repo.materialization_state.is_some() {
            restore_comfy_requirement_for_repo(
                &state,
                &app,
                &operation_id,
                &installation,
                &repo,
            )
            .await?;
        }
        state
            .db
            .ignore_repo_path(&repo.installation_id, &repo.kind, &repo.local_path)?;
''',
    "Kitchen untrack restores core requirement",
)

replace_once(
    '''            patch_core,
            install_or_patch_frontend,
            install_or_patch_custom_node,
''',
    '''            patch_core,
            install_or_patch_frontend,
            install_or_patch_kitchen,
            restore_comfy_managed_kitchen,
            install_or_patch_custom_node,
''',
    "register Kitchen commands",
)

path.write_text(text)
