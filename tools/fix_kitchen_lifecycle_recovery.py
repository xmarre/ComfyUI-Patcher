from pathlib import Path


def replace_once(path, old, new, label):
    p = Path(path)
    text = p.read_text(encoding="utf-8")
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{label}: expected 1 match, found {count}")
    p.write_text(text.replace(old, new, 1), encoding="utf-8", newline="\n")


def replace_region(path, start_marker, end_marker, replacement, label):
    p = Path(path)
    text = p.read_text(encoding="utf-8")
    start_count = text.count(start_marker)
    end_count = text.count(end_marker)
    if start_count != 1 or end_count != 1:
        raise SystemExit(
            f"{label}: expected unique markers, found start={start_count}, end={end_count}"
        )
    start = text.index(start_marker)
    end = text.index(end_marker, start)
    p.write_text(text[:start] + replacement + text[end:], encoding="utf-8", newline="\n")


lifecycle_helpers = '''async fn ensure_kitchen_lifecycle_source_recoverable(repo: &ManagedRepo) -> AppResult<()> {
    let saved_materialization = repo.materialization_state.as_ref().ok_or_else(|| {
        AppError::Conflict("Kitchen source recovery requires saved materialization state".to_string())
    })?;
    let path = Path::new(&repo.local_path);
    if !path.exists() || !has_git_marker(path) || !is_git_repo(path).await {
        return Err(AppError::Conflict(
            "cannot Disable/Uninstall an active Comfy Kitchen source override because its checkout is missing or invalid; use Restore ComfyUI Kitchen after repairing the checkout, or Untrack to leave the current runtime untouched"
                .to_string(),
        ));
    }
    let status = inspect_repo(path).await?;
    if status.head_sha.as_deref() != saved_materialization.materialized_head_sha.as_deref() {
        return Err(AppError::Conflict(
            "cannot Disable/Uninstall an active Comfy Kitchen source override while the checkout HEAD differs from the deployed source revision; update/repair Kitchen or use Restore ComfyUI Kitchen first"
                .to_string(),
        ));
    }
    if status.is_dirty {
        return Err(AppError::Conflict(
            "cannot Disable/Uninstall an active Comfy Kitchen source override from a dirty checkout because a failed lifecycle transition could not safely rebuild the previous runtime; clean/stash the checkout or Untrack instead"
                .to_string(),
        ));
    }
    Ok(())
}

fn combine_kitchen_lifecycle_recovery_error(
    original: AppError,
    recovery: AppError,
) -> AppError {
    AppError::Process(format!(
        "{original}; additionally failed to restore the previous managed Comfy Kitchen source runtime after the lifecycle failure: {recovery}"
    ))
}

async fn recover_kitchen_source_after_lifecycle_failure(
    state: &AppState,
    app: &AppHandle,
    operation_id: &str,
    installation: &Installation,
    repo: &ManagedRepo,
) -> AppResult<()> {
    let saved_materialization = repo.materialization_state.clone().ok_or_else(|| {
        AppError::Conflict("Kitchen lifecycle recovery has no saved source materialization".to_string())
    })?;
    let path = Path::new(&repo.local_path);
    if !path.exists() || !has_git_marker(path) || !is_git_repo(path).await {
        return Err(AppError::Conflict(
            "Kitchen lifecycle recovery cannot find the original managed source checkout"
                .to_string(),
        ));
    }
    let status = inspect_repo(path).await?;
    if status.head_sha.as_deref() != saved_materialization.materialized_head_sha.as_deref() {
        return Err(AppError::Conflict(format!(
            "Kitchen lifecycle recovery source HEAD {} differs from the previous materialized HEAD {}",
            status.head_sha.as_deref().unwrap_or("unknown"),
            saved_materialization
                .materialized_head_sha
                .as_deref()
                .unwrap_or("unknown")
        )));
    }

    let mut recovered_repo = repo.clone();
    recovered_repo.current_head_sha = status.head_sha;
    recovered_repo.current_branch = status.branch;
    recovered_repo.is_detached = status.is_detached;
    recovered_repo.is_dirty = status.is_dirty;
    recovered_repo.materialization_state = Some(saved_materialization.clone());

    let runtime = kitchen::probe_kitchen_runtime(installation).await?;
    state.db.set_kitchen_runtime(&installation.id, &runtime)?;
    if kitchen::evaluate_materialization(&recovered_repo, &runtime).status
        == MaterializationStatus::Current
    {
        state
            .db
            .set_repo_materialization_state(&repo.id, Some(&saved_materialization))?;
        log_operation(
            state,
            app,
            operation_id,
            "rollback",
            "info",
            "Kitchen lifecycle mutation failed before changing the source runtime; restored its materialization ownership without rebuilding",
        );
        return Ok(());
    }

    if status.is_dirty {
        return Err(AppError::Conflict(
            "Kitchen lifecycle mutation changed the runtime, but the restored source checkout is dirty and cannot be rebuilt safely"
                .to_string(),
        ));
    }
    log_operation(
        state,
        app,
        operation_id,
        "rollback",
        "warn",
        "Kitchen lifecycle mutation failed after changing runtime ownership; rebuilding the previous managed source revision",
    );
    materialize_kitchen_override(
        state,
        app,
        operation_id,
        installation,
        &recovered_repo,
    )
    .await
}

fn restore_moved_kitchen_checkout(original: &Path, moved: &Path) -> AppResult<()> {
    if !moved.exists() {
        return Ok(());
    }
    if original.exists() {
        return Err(AppError::Conflict(format!(
            "cannot restore retained Kitchen checkout {} because the original path {} is already occupied",
            moved.to_string_lossy(),
            original.to_string_lossy()
        )));
    }
    std::fs::rename(moved, original)?;
    Ok(())
}

#[tauri::command]
async fn uninstall_repo('''
replace_once(
    "src-tauri/src/lib.rs",
    "#[tauri::command]\nasync fn uninstall_repo(",
    lifecycle_helpers,
    "Kitchen lifecycle recovery helpers",
)

new_uninstall = '''async fn run_uninstall_repo(
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
        let source_active = kitchen_lifecycle_requires_comfy_restore(
            &repo.kind,
            repo.materialization_state.is_some(),
            &OperationKind::UninstallRepo,
        );
        log_operation(
            &state,
            &app,
            &operation_id,
            "preflight",
            "info",
            format!("uninstalling {}", repo.display_name),
        );

        if source_active {
            ensure_kitchen_lifecycle_source_recoverable(&repo).await?;
            let retained_root = installation_retained_backup_root(&installation, "uninstalling");
            std::fs::create_dir_all(&retained_root)?;
            let retained_path = choose_tracking_backup_path(&path, &retained_root);
            let mutation_result = async {
                restore_comfy_requirement_for_repo(
                    &state,
                    &app,
                    &operation_id,
                    &installation,
                    &repo,
                )
                .await?;
                if path.exists() {
                    std::fs::rename(&path, &retained_path)?;
                }
                state.db.delete_repo(&repo.id)?;
                Ok::<(), AppError>(())
            }
            .await;

            if let Err(original_error) = mutation_result {
                let recovery_result = async {
                    restore_moved_kitchen_checkout(&path, &retained_path)?;
                    recover_kitchen_source_after_lifecycle_failure(
                        &state,
                        &app,
                        &operation_id,
                        &installation,
                        &repo,
                    )
                    .await
                }
                .await;
                return Err(match recovery_result {
                    Ok(()) => original_error,
                    Err(recovery_error) => combine_kitchen_lifecycle_recovery_error(
                        original_error,
                        recovery_error,
                    ),
                });
            }

            if retained_path.exists() {
                if let Err(cleanup_error) = remove_path_with_retries(&retained_path).await {
                    return Err(AppError::Io(format!(
                        "Kitchen runtime was restored to ComfyUI ownership and Patcher source management was removed, but the retained source checkout could not be deleted from {}: {cleanup_error}",
                        retained_path.to_string_lossy()
                    )));
                }
            }
        } else {
            if path.exists() {
                remove_path_with_retries(&path).await?;
            }
            state.db.delete_repo(&repo.id)?;
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

'''
replace_region(
    "src-tauri/src/lib.rs",
    "async fn run_uninstall_repo(\n",
    "#[tauri::command]\nasync fn disable_repo(",
    new_uninstall,
    "Kitchen uninstall transactional lifecycle",
)

new_disable = '''async fn run_disable_repo(
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
        let source_active = kitchen_lifecycle_requires_comfy_restore(
            &repo.kind,
            repo.materialization_state.is_some(),
            &OperationKind::DisableRepo,
        );
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

        if source_active {
            ensure_kitchen_lifecycle_source_recoverable(&repo).await?;
            let mutation_result = async {
                restore_comfy_requirement_for_repo(
                    &state,
                    &app,
                    &operation_id,
                    &installation,
                    &repo,
                )
                .await?;
                if path.exists() {
                    std::fs::rename(&path, &disabled_path)?;
                }
                state.db.delete_repo(&repo.id)?;
                Ok::<(), AppError>(())
            }
            .await;

            if let Err(original_error) = mutation_result {
                let recovery_result = async {
                    restore_moved_kitchen_checkout(&path, &disabled_path)?;
                    recover_kitchen_source_after_lifecycle_failure(
                        &state,
                        &app,
                        &operation_id,
                        &installation,
                        &repo,
                    )
                    .await
                }
                .await;
                return Err(match recovery_result {
                    Ok(()) => original_error,
                    Err(recovery_error) => combine_kitchen_lifecycle_recovery_error(
                        original_error,
                        recovery_error,
                    ),
                });
            }
        } else {
            if path.exists() {
                std::fs::rename(&path, &disabled_path)?;
            }
            state.db.delete_repo(&repo.id)?;
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

'''
replace_region(
    "src-tauri/src/lib.rs",
    "async fn run_disable_repo(\n",
    "#[tauri::command]\nasync fn untrack_repo(",
    new_disable,
    "Kitchen disable transactional lifecycle",
)

replace_once(
    "README.md",
    '''- **Disable / Uninstall**: before moving or deleting an active source checkout, Patcher restores and verifies the Kitchen requirement declared by the current managed ComfyUI core checkout. A failed restore aborts the filesystem mutation.''',
    '''- **Disable / Uninstall**: before moving or deleting an active source checkout, Patcher requires the deployed source checkout to be recoverable, then restores and verifies the Kitchen requirement declared by the current managed ComfyUI core checkout. If the later filesystem/DB transition fails, Patcher restores the checkout path and reasserts the previous source runtime before reporting failure. Uninstall stages the checkout outside its managed sibling path until DB removal succeeds, so a cleanup failure cannot silently recreate source management.''',
    "README lifecycle recovery semantics",
)
replace_once(
    "CHANGELOG.md",
    '- Kitchen Untrack preserves the currently installed runtime while stopping future source reassertion; Disable/Uninstall restore the requirement owned by the current ComfyUI checkout first.\n',
    '- Kitchen Untrack preserves the currently installed runtime while stopping future source reassertion; Disable/Uninstall restore the requirement owned by the current ComfyUI checkout first and recover the previous source runtime if a later lifecycle mutation fails.\n',
    "CHANGELOG lifecycle recovery",
)

print("Kitchen lifecycle recovery transformations applied")
