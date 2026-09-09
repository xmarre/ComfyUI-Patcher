from pathlib import Path
import re


def replace_block(path: str, old: str, new: str, label: str, expected: int = 1) -> None:
    file_path = Path(path)
    raw = file_path.read_bytes()
    parts = old.encode("utf-8").split(b"\n")
    pattern = re.compile(b"\\r?\\n".join(re.escape(part) for part in parts))
    matches = list(pattern.finditer(raw))
    if len(matches) != expected:
        raise SystemExit(f"{label}: expected {expected} matches, found {len(matches)}")
    offset = 0
    for match in matches:
        start = match.start() + offset
        end = match.end() + offset
        matched = raw[start:end]
        newline = b"\r\n" if b"\r\n" in matched else b"\n"
        replacement = new.encode("utf-8").replace(b"\n", newline)
        raw = raw[:start] + replacement + raw[end:]
        offset += len(replacement) - (end - start)
    file_path.write_bytes(raw)


def replace_bytes(path: str, old: bytes, new: bytes, label: str, expected: int = 1) -> None:
    file_path = Path(path)
    raw = file_path.read_bytes()
    count = raw.count(old)
    if count != expected:
        raise SystemExit(f"{label}: expected {expected} matches, found {count}")
    file_path.write_bytes(raw.replace(old, new))


# 1. Kitchen source materialization requires immutable Git provenance before build/install.
replace_block(
    "src-tauri/src/kitchen.rs",
    '    let head_sha = rev_parse(repo_path, "HEAD").await?;',
    '''    let head_sha = rev_parse(repo_path, "HEAD").await?.ok_or_else(|| {
        AppError::Git(
            "cannot materialize Comfy Kitchen source: repository HEAD could not be resolved"
                .to_string(),
        )
    })?;''',
    "Kitchen HEAD provenance",
)

# 2. Runtime reconciliation uses PEP 610 wheel origin/hash as additional replacement evidence.
old_eval = '''    let status = if !probe.distribution_present {
        MaterializationStatus::Missing
    } else if !probe.import_ok {
        MaterializationStatus::ImportFailed
    } else if state.materialized_head_sha.is_none()
        || repo.current_head_sha.as_deref() != state.materialized_head_sha.as_deref()
    {
        MaterializationStatus::Stale
    } else if state
        .installed_record_sha256
        .as_deref()
        .zip(probe.record_sha256.as_deref())
        .is_some_and(|(expected, actual)| !expected.eq_ignore_ascii_case(actual))
        || state
            .installed_version
            .as_deref()
            .zip(probe.installed_version.as_deref())
            .is_some_and(|(expected, actual)| expected != actual)
    {
        MaterializationStatus::Replaced
    } else if state.installed_record_sha256.is_some() && probe.record_sha256.is_none() {
        MaterializationStatus::Replaced
    } else {
        MaterializationStatus::Current
    };'''
new_eval = '''    let installed_artifact_mismatch = state
        .artifact_sha256
        .as_deref()
        .zip(probe.direct_url_sha256.as_deref())
        .is_some_and(|(expected, actual)| !expected.eq_ignore_ascii_case(actual));
    let installed_source_origin_mismatch = state
        .installed_origin
        .as_deref()
        .filter(|origin| origin.starts_with("file:"))
        .is_some_and(|expected| probe.direct_url.as_deref() != Some(expected));

    let status = if !probe.distribution_present {
        MaterializationStatus::Missing
    } else if !probe.import_ok {
        MaterializationStatus::ImportFailed
    } else if state.materialized_head_sha.is_none()
        || repo.current_head_sha.as_deref() != state.materialized_head_sha.as_deref()
    {
        MaterializationStatus::Stale
    } else if state
        .installed_record_sha256
        .as_deref()
        .zip(probe.record_sha256.as_deref())
        .is_some_and(|(expected, actual)| !expected.eq_ignore_ascii_case(actual))
        || state
            .installed_version
            .as_deref()
            .zip(probe.installed_version.as_deref())
            .is_some_and(|(expected, actual)| expected != actual)
        || installed_artifact_mismatch
        || installed_source_origin_mismatch
    {
        MaterializationStatus::Replaced
    } else if state.installed_record_sha256.is_some() && probe.record_sha256.is_none() {
        MaterializationStatus::Replaced
    } else {
        MaterializationStatus::Current
    };'''
replace_block("src-tauri/src/kitchen.rs", old_eval, new_eval, "Kitchen runtime provenance evaluation")

old_test = '''    #[test]
    fn reconciliation_distinguishes_checkout_staleness_and_external_replacement() {
        let mut repo = repo();
        let probe = KitchenRuntimeProbe {
            distribution_present: true,
            installed_version: Some("0.2.33".to_string()),
            record_sha256: Some("record-a".to_string()),
            import_ok: true,
            ..Default::default()
        };
        assert_eq!(
            evaluate_materialization(&repo, &probe).status,
            MaterializationStatus::Current
        );

        repo.current_head_sha = Some("def".to_string());
        assert_eq!(
            evaluate_materialization(&repo, &probe).status,
            MaterializationStatus::Stale
        );

        repo.current_head_sha = Some("abc".to_string());
        let replaced = KitchenRuntimeProbe {
            record_sha256: Some("record-b".to_string()),
            ..probe.clone()
        };
        assert_eq!(
            evaluate_materialization(&repo, &replaced).status,
            MaterializationStatus::Replaced
        );
    }'''
new_test = '''    #[test]
    fn reconciliation_distinguishes_checkout_staleness_and_external_replacement() {
        let mut repo = repo();
        let probe = KitchenRuntimeProbe {
            distribution_present: true,
            installed_version: Some("0.2.33".to_string()),
            direct_url: Some("file:///wheel.whl".to_string()),
            direct_url_sha256: Some("a".repeat(64)),
            record_sha256: Some("record-a".to_string()),
            import_ok: true,
            ..Default::default()
        };
        assert_eq!(
            evaluate_materialization(&repo, &probe).status,
            MaterializationStatus::Current
        );

        repo.current_head_sha = Some("def".to_string());
        assert_eq!(
            evaluate_materialization(&repo, &probe).status,
            MaterializationStatus::Stale
        );

        repo.current_head_sha = Some("abc".to_string());
        let replaced_record = KitchenRuntimeProbe {
            record_sha256: Some("record-b".to_string()),
            ..probe.clone()
        };
        assert_eq!(
            evaluate_materialization(&repo, &replaced_record).status,
            MaterializationStatus::Replaced
        );

        let replaced_artifact = KitchenRuntimeProbe {
            direct_url_sha256: Some("b".repeat(64)),
            ..probe.clone()
        };
        assert_eq!(
            evaluate_materialization(&repo, &replaced_artifact).status,
            MaterializationStatus::Replaced
        );

        let replaced_origin = KitchenRuntimeProbe {
            direct_url: None,
            direct_url_sha256: None,
            ..probe.clone()
        };
        assert_eq!(
            evaluate_materialization(&repo, &replaced_origin).status,
            MaterializationStatus::Replaced
        );

        let mut fallback_repo = repo();
        fallback_repo
            .materialization_state
            .as_mut()
            .unwrap()
            .installed_origin = Some("/venv/site-packages".to_string());
        let fallback_probe = KitchenRuntimeProbe {
            direct_url: None,
            direct_url_sha256: None,
            ..probe
        };
        assert_eq!(
            evaluate_materialization(&fallback_repo, &fallback_probe).status,
            MaterializationStatus::Current
        );
    }'''
replace_block("src-tauri/src/kitchen.rs", old_test, new_test, "Kitchen provenance regression test")

# 3. The additive migration fixture must use the JSON representation actually persisted by old builds.
replace_bytes(
    "src-tauri/src/db.rs",
    b"'\\\"custom_node\\\"'",
    b"'\"custom_node\"'",
    "legacy RepoKind fixture",
)

# 4. Make installation-wide repository ordering explicit and testable: Kitchen is last.
old_collect = '''fn collect_installation_repos(detail: InstallationDetail) -> Vec<ManagedRepo> {
    let mut repos = Vec::new();
    if let Some(core) = detail.core_repo {
        repos.push(core);
    }
    if let Some(frontend) = detail.frontend_repo {
        repos.push(frontend);
    }
    repos.extend(detail.custom_node_repos);
    if let Some(kitchen) = detail.kitchen_repo {
        repos.push(kitchen);
    }
    repos
}'''
new_collect = '''fn installation_repo_priority(kind: &RepoKind) -> u8 {
    match kind {
        RepoKind::Core => 0,
        RepoKind::Frontend => 1,
        RepoKind::CustomNode => 2,
        RepoKind::Kitchen => 3,
    }
}

fn collect_installation_repos(detail: InstallationDetail) -> Vec<ManagedRepo> {
    let mut repos = Vec::new();
    if let Some(core) = detail.core_repo {
        repos.push(core);
    }
    if let Some(frontend) = detail.frontend_repo {
        repos.push(frontend);
    }
    repos.extend(detail.custom_node_repos);
    if let Some(kitchen) = detail.kitchen_repo {
        repos.push(kitchen);
    }
    repos.sort_by_key(|repo| installation_repo_priority(&repo.kind));
    repos
}'''
replace_block("src-tauri/src/lib.rs", old_collect, new_collect, "installation repository priority")

# 5. Before a Kitchen checkout exists, bare refs resolve against the known official upstream.
resolve_anchor = '''async fn resolve_target_for_context(
    state: &AppState,'''
resolve_new = '''fn default_remote_for_repo_kind(kind: &RepoKind) -> Option<&'static str> {
    match kind {
        RepoKind::Kitchen => Some(kitchen::DEFAULT_KITCHEN_REPO_URL),
        _ => None,
    }
}

async fn resolve_target_for_context(
    state: &AppState,'''
replace_block("src-tauri/src/lib.rs", resolve_anchor, resolve_new, "Kitchen implicit upstream resolver")
replace_block(
    "src-tauri/src/lib.rs",
    '''    let current_remote = repo.as_ref().and_then(|r| r.canonical_remote.as_deref());''',
    '''    let current_remote = repo
        .as_ref()
        .and_then(|r| r.canonical_remote.as_deref())
        .or_else(|| {
            if repo.is_none() {
                default_remote_for_repo_kind(kind)
            } else {
                None
            }
        });''',
    "resolver current remote",
)

# 6. Single-repo dependency operations immediately restore managed Kitchen even when pip fails;
# installation-wide operations can defer that work to one finalization point.
old_maybe_sync = '''async fn maybe_sync_dependencies(
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
}'''
new_maybe_sync = '''#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ManagedPythonOverrideMode {
    Immediate,
    Deferred,
}

fn should_reassert_managed_kitchen_override(
    repo_kind: &RepoKind,
    mode: ManagedPythonOverrideMode,
) -> bool {
    mode == ManagedPythonOverrideMode::Immediate
        && matches!(repo_kind, RepoKind::Core | RepoKind::CustomNode)
}

fn combine_dependency_and_override_errors(
    dependency_error: AppError,
    override_error: AppError,
) -> AppError {
    AppError::Dependency(format!(
        "{dependency_error}; additionally failed to reassert the managed Comfy Kitchen source override: {override_error}"
    ))
}

async fn maybe_sync_dependencies_with_override_mode(
    state: &AppState,
    app: &AppHandle,
    operation_id: &str,
    installation: &Installation,
    repo: &ManagedRepo,
    repo_path: &Path,
    enabled: bool,
    override_mode: ManagedPythonOverrideMode,
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

    let dependency_result = if plan.steps.is_empty() {
        Ok(())
    } else {
        execute_dependency_sync(&plan).await
    };
    let override_result = if should_reassert_managed_kitchen_override(&repo.kind, override_mode) {
        reassert_managed_kitchen_override_if_needed(state, app, operation_id, installation).await
    } else {
        Ok(())
    };

    match (dependency_result, override_result) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) => Err(error),
        (Ok(()), Err(error)) => Err(error),
        (Err(dependency_error), Err(override_error)) => Err(
            combine_dependency_and_override_errors(dependency_error, override_error),
        ),
    }
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
    maybe_sync_dependencies_with_override_mode(
        state,
        app,
        operation_id,
        installation,
        repo,
        repo_path,
        enabled,
        ManagedPythonOverrideMode::Immediate,
    )
    .await
}'''
replace_block("src-tauri/src/lib.rs", old_maybe_sync, new_maybe_sync, "managed Python override orchestration")

# 7. Thread the override mode through repo materialization while preserving the existing public helper.
old_apply_header = '''async fn apply_repo_tracking_state(
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
    validate_overlay_stack(tracked_state)?;'''
new_apply_header = '''async fn apply_repo_tracking_state(
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
    apply_repo_tracking_state_with_override_mode(
        state,
        app,
        operation_id,
        installation,
        repo,
        tracked_state,
        dirty_repo_strategy,
        sync_dependencies,
        write_tracked_target,
        ManagedPythonOverrideMode::Immediate,
    )
    .await
}

async fn apply_repo_tracking_state_with_override_mode(
    state: &AppState,
    app: &AppHandle,
    operation_id: &str,
    installation: &Installation,
    repo: &ManagedRepo,
    tracked_state: &TrackedRepoState,
    dirty_repo_strategy: &DirtyRepoStrategy,
    sync_dependencies: bool,
    write_tracked_target: bool,
    override_mode: ManagedPythonOverrideMode,
) -> AppResult<RepoCheckpoint> {
    validate_overlay_stack(tracked_state)?;'''
replace_block("src-tauri/src/lib.rs", old_apply_header, new_apply_header, "repo apply override mode")
replace_block(
    "src-tauri/src/lib.rs",
    '''            maybe_sync_dependencies(
                state,
                app,
                operation_id,
                installation,
                repo,
                path,
                sync_dependencies,
            )
            .await?;''',
    '''            maybe_sync_dependencies_with_override_mode(
                state,
                app,
                operation_id,
                installation,
                repo,
                path,
                sync_dependencies,
                override_mode,
            )
            .await?;''',
    "repo apply deferred dependency sync",
)

# 8. Preserve previous known-good runtime when forward materialization fails before install;
# explicit rollback/checkpoint restore still forces a rebuild of the restored revision.
replace_block(
    "src-tauri/src/lib.rs",
    '''    checkpoint: &RepoCheckpoint,
    restore_stash: bool,
) -> AppResult<()> {''',
    '''    checkpoint: &RepoCheckpoint,
    restore_stash: bool,
    force_kitchen_runtime_rebuild: bool,
) -> AppResult<()> {''',
    "Kitchen checkpoint recovery signature",
)
old_restore_build = '''        restored_repo.materialization_state = Some(saved_materialization.clone());
        materialize_kitchen_override(
            state,
            app,
            operation_id,
            installation,
            &restored_repo,
        )
        .await?;'''
new_restore_build = '''        restored_repo.materialization_state = Some(saved_materialization.clone());
        let runtime_is_current = if force_kitchen_runtime_rebuild {
            false
        } else {
            match kitchen::probe_kitchen_runtime(installation).await {
                Ok(runtime) => {
                    state.db.set_kitchen_runtime(&installation.id, &runtime)?;
                    kitchen::evaluate_materialization(&restored_repo, &runtime).status
                        == MaterializationStatus::Current
                }
                Err(error) => {
                    log_operation(
                        state,
                        app,
                        operation_id,
                        "rollback",
                        "warn",
                        format!(
                            "failed to verify whether the previous Kitchen runtime survived unchanged; rebuilding checkpointed source: {error}"
                        ),
                    );
                    false
                }
            }
        };
        if runtime_is_current {
            log_operation(
                state,
                app,
                operation_id,
                "rollback",
                "info",
                "previous Comfy Kitchen runtime still matches the restored checkpoint; skipping an unnecessary rebuild",
            );
        } else {
            materialize_kitchen_override(
                state,
                app,
                operation_id,
                installation,
                &restored_repo,
            )
            .await?;
        }'''
replace_block("src-tauri/src/lib.rs", old_restore_build, new_restore_build, "Kitchen checkpoint runtime preservation")

# Known call sites: forward apply failure=false; restore-ComfyUI mutation recovery=true;
# explicit rollback and explicit checkpoint restore=true.
replace_block(
    "src-tauri/src/lib.rs",
    '''                &checkpoint,
                true,
            )
            .await;''',
    '''                &checkpoint,
                true,
                false,
            )
            .await;''',
    "forward apply recovery mode",
)
replace_block(
    "src-tauri/src/lib.rs",
    '''                &checkpoint,
                false,
            )
            .await;''',
    '''                &checkpoint,
                false,
                true,
            )
            .await;''',
    "restore ComfyUI mutation recovery mode",
)
replace_block(
    "src-tauri/src/lib.rs",
    '''            &checkpoint,
            input.restore_stash,
        )
        .await?;''',
    '''            &checkpoint,
            input.restore_stash,
            true,
        )
        .await?;''',
    "explicit Kitchen rollback recovery mode",
    expected=2,
)

# 9. Batch operations defer override reassertion and perform one final verification/reassertion.
replace_block(
    "src-tauri/src/lib.rs",
    '''                match apply_repo_tracking_state(
                    &state,
                    &app,
                    &operation_id,
                    &installation,
                    &repo,
                    &tracked_state,
                    &input.dirty_repo_strategy,
                    input.sync_dependencies,
                    true,
                )''',
    '''                match apply_repo_tracking_state_with_override_mode(
                    &state,
                    &app,
                    &operation_id,
                    &installation,
                    &repo,
                    &tracked_state,
                    &input.dirty_repo_strategy,
                    input.sync_dependencies,
                    true,
                    ManagedPythonOverrideMode::Deferred,
                )''',
    "Update all deferred override",
)
replace_block(
    "src-tauri/src/lib.rs",
    '''            match apply_repo_tracking_state(
                &state,
                &app,
                &operation_id,
                &installation,
                &repo,
                &tracked_state,
                &DirtyRepoStrategy::HardReset,
                input.sync_dependencies,
                true,
            )''',
    '''            match apply_repo_tracking_state_with_override_mode(
                &state,
                &app,
                &operation_id,
                &installation,
                &repo,
                &tracked_state,
                &DirtyRepoStrategy::HardReset,
                input.sync_dependencies,
                true,
                ManagedPythonOverrideMode::Deferred,
            )''',
    "Rematerialize deferred override",
)
replace_block(
    "src-tauri/src/lib.rs",
    '''        }

        if failures.is_empty() {
            maybe_restart_installation(''',
    '''        }

        if input.sync_dependencies {
            if let Err(error) = reassert_managed_kitchen_override_if_needed(
                &state,
                &app,
                &operation_id,
                &installation,
            )
            .await
            {
                failures.push(format!("Comfy Kitchen finalization: {error}"));
            }
        }

        if failures.is_empty() {
            maybe_restart_installation(''',
    "batch Kitchen finalization",
    expected=2,
)

# 10. Kitchen lifecycle policy: untrack leaves the currently installed runtime untouched.
insert_lifecycle = '''fn kitchen_lifecycle_requires_comfy_restore(
    kind: &RepoKind,
    has_materialization: bool,
    operation_kind: &OperationKind,
) -> bool {
    *kind == RepoKind::Kitchen
        && has_materialization
        && matches!(
            operation_kind,
            OperationKind::UninstallRepo | OperationKind::DisableRepo
        )
}

#[tauri::command]
async fn uninstall_repo('''
replace_block(
    "src-tauri/src/lib.rs",
    '''#[tauri::command]
async fn uninstall_repo(''',
    insert_lifecycle,
    "Kitchen lifecycle policy helper",
)
replace_block(
    "src-tauri/src/lib.rs",
    '''        if repo.kind == RepoKind::Kitchen && repo.materialization_state.is_some() {''',
    '''        if kitchen_lifecycle_requires_comfy_restore(
            &repo.kind,
            repo.materialization_state.is_some(),
            &OperationKind::UninstallRepo,
        ) {''',
    "Kitchen uninstall restore policy",
)
# The second identical old condition is Disable; after replacing first only one remains.
replace_block(
    "src-tauri/src/lib.rs",
    '''        if repo.kind == RepoKind::Kitchen && repo.materialization_state.is_some() {''',
    '''        if kitchen_lifecycle_requires_comfy_restore(
            &repo.kind,
            repo.materialization_state.is_some(),
            &OperationKind::DisableRepo,
        ) {''',
    "Kitchen disable restore policy",
)
replace_block(
    "src-tauri/src/lib.rs",
    '''        let _ = run_untrack_repo(app, state_handle, installation, repo, op_id).await;''',
    '''        let _ = run_untrack_repo(app, state_handle, repo, op_id).await;''',
    "Kitchen untrack spawn",
)
replace_block(
    "src-tauri/src/lib.rs",
    '''async fn run_untrack_repo(
    app: AppHandle,
    state: AppState,
    installation: Installation,
    repo: ManagedRepo,
    operation_id: String,
) -> AppResult<()> {''',
    '''async fn run_untrack_repo(
    app: AppHandle,
    state: AppState,
    repo: ManagedRepo,
    operation_id: String,
) -> AppResult<()> {''',
    "Kitchen untrack signature",
)
old_untrack_restore = '''        if repo.kind == RepoKind::Kitchen && repo.materialization_state.is_some() {
            restore_comfy_requirement_for_repo(
                &state,
                &app,
                &operation_id,
                &installation,
                &repo,
            )
            .await?;
        }'''
new_untrack_restore = '''        if repo.kind == RepoKind::Kitchen && repo.materialization_state.is_some() {
            log_operation(
                &state,
                &app,
                &operation_id,
                "materialization",
                "warn",
                "stopping Kitchen source management without changing the currently installed comfy-kitchen runtime; Patcher will no longer reassert this source build",
            );
        }'''
replace_block("src-tauri/src/lib.rs", old_untrack_restore, new_untrack_restore, "Kitchen untrack runtime preservation")

# 11. Pure regression tests pin the orchestration/lifecycle invariants.
test_anchor = '''#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {'''
test_module = '''#[cfg(test)]
mod kitchen_management_tests {
    use super::*;

    #[test]
    fn kitchen_uses_official_upstream_before_a_checkout_exists() {
        assert_eq!(
            default_remote_for_repo_kind(&RepoKind::Kitchen),
            Some(kitchen::DEFAULT_KITCHEN_REPO_URL)
        );
        assert_eq!(default_remote_for_repo_kind(&RepoKind::Core), None);
        assert_eq!(default_remote_for_repo_kind(&RepoKind::Frontend), None);
        assert_eq!(default_remote_for_repo_kind(&RepoKind::CustomNode), None);
    }

    #[test]
    fn installation_wide_order_keeps_kitchen_after_python_dependency_repos() {
        assert!(installation_repo_priority(&RepoKind::Kitchen)
            > installation_repo_priority(&RepoKind::CustomNode));
        assert!(installation_repo_priority(&RepoKind::Kitchen)
            > installation_repo_priority(&RepoKind::Core));
    }

    #[test]
    fn managed_python_override_policy_is_immediate_only_for_single_python_repo_ops() {
        assert!(should_reassert_managed_kitchen_override(
            &RepoKind::Core,
            ManagedPythonOverrideMode::Immediate
        ));
        assert!(should_reassert_managed_kitchen_override(
            &RepoKind::CustomNode,
            ManagedPythonOverrideMode::Immediate
        ));
        assert!(!should_reassert_managed_kitchen_override(
            &RepoKind::Core,
            ManagedPythonOverrideMode::Deferred
        ));
        assert!(!should_reassert_managed_kitchen_override(
            &RepoKind::Kitchen,
            ManagedPythonOverrideMode::Immediate
        ));
    }

    #[test]
    fn kitchen_untrack_preserves_runtime_but_remove_and_disable_restore_comfyui_requirement() {
        assert!(kitchen_lifecycle_requires_comfy_restore(
            &RepoKind::Kitchen,
            true,
            &OperationKind::UninstallRepo
        ));
        assert!(kitchen_lifecycle_requires_comfy_restore(
            &RepoKind::Kitchen,
            true,
            &OperationKind::DisableRepo
        ));
        assert!(!kitchen_lifecycle_requires_comfy_restore(
            &RepoKind::Kitchen,
            true,
            &OperationKind::UntrackRepo
        ));
        assert!(!kitchen_lifecycle_requires_comfy_restore(
            &RepoKind::Kitchen,
            false,
            &OperationKind::UninstallRepo
        ));
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {'''
replace_block("src-tauri/src/lib.rs", test_anchor, test_module, "Kitchen orchestration regression tests")

# 12. Frontend lifecycle copy must match the runtime-preserving Untrack behavior.
replace_block(
    "src/components/RepoCard.tsx",
    '''            {repo.kind === "kitchen" && repo.materializationState
              ? "Before Kitchen is uninstalled, disabled, or untracked, Patcher first restores the comfy-kitchen requirement declared by the current ComfyUI checkout. Lifecycle actions do not create new checkpoints."
              : "Lifecycle actions do not create new checkpoints. They remove or hide the repo directly, and untrack also suppresses future reconcile rediscovery for this path."}''',
    '''            {repo.kind === "kitchen" && repo.materializationState
              ? "Uninstall and Disable first restore the comfy-kitchen requirement declared by the current ComfyUI checkout. Untrack instead leaves the currently installed runtime unchanged and only stops future Patcher source reassertion. Lifecycle actions do not create new checkpoints."
              : "Lifecycle actions do not create new checkpoints. They remove or hide the repo directly, and untrack also suppresses future reconcile rediscovery for this path."}''',
    "Kitchen lifecycle UI copy",
)

# 13. Persistent docs describe implicit upstream resolution, batch finalization, and lifecycle semantics.
replace_block(
    "README.md",
    '''* raw names are resolved against `origin` for existing managed repos
* branch names containing slashes are supported
* target resolution is repo-kind aware: `core`, `frontend`, `kitchen`, or `custom_node`''',
    '''* raw names are resolved against `origin` for existing managed repos
* before a Kitchen checkout exists, bare Kitchen branch/tag/commit inputs resolve against the official `Comfy-Org/comfy-kitchen` upstream
* branch names containing slashes are supported
* target resolution is repo-kind aware: `core`, `frontend`, `kitchen`, or `custom_node`''',
    "README Kitchen implicit upstream",
)
replace_block(
    "README.md",
    '''A Patcher-managed Kitchen source override is reasserted after Patcher-controlled core/custom-node dependency installs if those installs replace the active `comfy-kitchen` distribution.''',
    '''A Patcher-managed Kitchen source override is reasserted after Patcher-controlled core/custom-node dependency installs if those installs replace the active `comfy-kitchen` distribution. Installation-wide **Update all** and tracked-repository rematerialization defer that override work until ordinary Python dependency mutations finish, then finalize Kitchen once at the end instead of rebuilding it after every repository.

**Untrack** stops source management/reassertion but deliberately leaves the currently installed Kitchen package untouched. **Restore ComfyUI Kitchen**, **Disable**, and **Uninstall** return runtime ownership to the `comfy-kitchen` requirement declared by the current managed ComfyUI checkout before deactivating/removing the source override.''',
    "README Kitchen ordering and lifecycle",
)

# Changelog is intentionally part of the implementation PR; clarify the final orchestration invariant.
replace_block(
    "CHANGELOG.md",
    '''- Patcher reasserts an active managed Kitchen source build after Patcher-controlled Python dependency installs replace it, and blocks Start/Restart when active Kitchen runtime provenance is incoherent.''',
    '''- Patcher reasserts an active managed Kitchen source build after Patcher-controlled Python dependency installs replace it, defers that reassertion to one final Kitchen pass during installation-wide operations, and blocks Start/Restart when active Kitchen runtime provenance is incoherent.
- Kitchen Untrack preserves the currently installed runtime while stopping future source reassertion; Disable/Uninstall restore the requirement owned by the current ComfyUI checkout first.''',
    "CHANGELOG Kitchen safety semantics",
)

print("Kitchen hostile-review transformations applied successfully")
