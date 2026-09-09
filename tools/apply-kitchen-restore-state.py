from pathlib import Path


def replace_once(path: Path, old: str, new: str, label: str) -> None:
    text = path.read_text()
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{label}: expected exactly one match, found {count}")
    path.write_text(text.replace(old, new, 1))


lib = Path("src-tauri/src/lib.rs")
text = lib.read_text()

marker = '''#[tauri::command]\nasync fn restore_comfy_managed_kitchen(\n'''
helper = '''async fn create_kitchen_runtime_checkpoint(\n    state: &AppState,\n    installation: &Installation,\n    repo: &ManagedRepo,\n    operation_id: &str,\n) -> AppResult<RepoCheckpoint> {\n    if repo.kind != RepoKind::Kitchen {\n        return Err(AppError::InvalidInput(\n            "Kitchen runtime checkpoint requires a Kitchen repository".to_string(),\n        ));\n    }\n    let path = Path::new(&repo.local_path);\n    let status = inspect_repo(path).await?;\n    let head = status\n        .head_sha\n        .clone()\n        .ok_or_else(|| AppError::Git("Kitchen repository has no HEAD".to_string()))?;\n    let operation = state\n        .db\n        .get_operation(operation_id)?\n        .ok_or_else(|| AppError::NotFound("operation not found".to_string()))?;\n    let (label, reason) = checkpoint_label_and_reason(&operation, repo);\n    let dependency_state =\n        build_repo_dependency_state(installation, repo, path, &status.changed_files);\n    state.db.create_checkpoint(\n        &repo.id,\n        operation_id,\n        &head,\n        status.branch.as_deref(),\n        status.is_detached,\n        true,\n        repo.tracked_target_kind.as_ref(),\n        repo.tracked_target_input.as_deref(),\n        repo.tracked_target_resolved_sha.as_deref(),\n        false,\n        None,\n        Some(&label),\n        Some(&reason),\n        dependency_state.as_ref(),\n    )\n}\n\n'''
if text.count(marker) != 1:
    raise SystemExit(f"Kitchen restore command marker: expected one match, found {text.count(marker)}")
text = text.replace(marker, helper + marker, 1)

start = text.index("async fn run_restore_comfy_managed_kitchen(")
end = text.index("\nstruct CustomNodeInstallRunResult", start)
segment = text[start:end]
old = '''    let result = async {\n        restore_comfy_requirement_for_repo(\n            &state,\n            &app,\n            &operation_id,\n            &installation,\n            &repo,\n        )\n        .await?;\n        maybe_restart_installation(\n            &state,\n            &app,\n            &operation_id,\n            &installation,\n            input.restart_after_success,\n        )\n        .await?;\n        state\n            .db\n            .finish_operation(&operation_id, OperationStatus::Succeeded, None, None)?;\n        log_operation(\n            &state,\n            &app,\n            &operation_id,\n            "done",\n            "info",\n            "restored the ComfyUI-managed comfy-kitchen requirement",\n        );\n        Ok::<(), AppError>(())\n    }\n    .await;\n'''
new = '''    let result = async {\n        let checkpoint = create_kitchen_runtime_checkpoint(\n            &state,\n            &installation,\n            &repo,\n            &operation_id,\n        )\n        .await?;\n        log_operation(\n            &state,\n            &app,\n            &operation_id,\n            "checkpoint",\n            "info",\n            format!(\n                "checkpoint {} created before returning Kitchen runtime ownership to ComfyUI",\n                checkpoint.id\n            ),\n        );\n\n        let mutation = async {\n            restore_comfy_requirement_for_repo(\n                &state,\n                &app,\n                &operation_id,\n                &installation,\n                &repo,\n            )\n            .await?;\n            // Returning runtime ownership to ComfyUI is a durable state transition.\n            // Clear the tracked source target as well as materialization provenance so\n            // Update all cannot silently reactivate the source override later. The\n            // checkout remains on disk and can be explicitly adopted again through\n            // Install / Patch source.\n            state.db.set_repo_tracked_state(&repo.id, None, None)?;\n            refresh_repo_state(&state, &repo.id).await?;\n            Ok::<(), AppError>(())\n        }\n        .await;\n\n        if let Err(error) = mutation {\n            let restore_result = restore_checkpoint_with_kitchen_runtime(\n                &state,\n                &app,\n                &operation_id,\n                &installation,\n                &repo,\n                &checkpoint,\n                false,\n            )\n            .await;\n            return match restore_result {\n                Ok(()) => Err(error),\n                Err(restore_error) => Err(restore_checkpoint_error(error, restore_error)),\n            };\n        }\n\n        maybe_restart_installation(\n            &state,\n            &app,\n            &operation_id,\n            &installation,\n            input.restart_after_success,\n        )\n        .await?;\n        state.db.finish_operation(\n            &operation_id,\n            OperationStatus::Succeeded,\n            None,\n            Some(&checkpoint.id),\n        )?;\n        log_operation(\n            &state,\n            &app,\n            &operation_id,\n            "done",\n            "info",\n            "restored the ComfyUI-managed comfy-kitchen requirement and deactivated tracked source override management",\n        );\n        Ok::<(), AppError>(())\n    }\n    .await;\n'''
if segment.count(old) != 1:
    raise SystemExit(f"Kitchen restore implementation: expected one match, found {segment.count(old)}")
segment = segment.replace(old, new, 1)
text = text[:start] + segment + text[end:]
lib.write_text(text)

card = Path("src/components/RepoCard.tsx")
replace_once(
    card,
    '''                    "Restore the comfy-kitchen requirement declared by the current ComfyUI checkout? The source checkout stays on disk, but its built runtime override is deactivated."\n''',
    '''                    "Restore the comfy-kitchen requirement declared by the current ComfyUI checkout? The source checkout stays on disk, but its tracked source target and built runtime override are deactivated. Use Install / Patch source to enable source management again."\n''',
    "Kitchen restore confirmation semantics",
)

readme = Path("README.md")
replace_once(
    readme,
    '''* **Restore ComfyUI Kitchen** installs the `comfy-kitchen` requirement declared by the current ComfyUI checkout and deactivates the source-built runtime override without fabricating or deleting Git history\n''',
    '''* **Restore ComfyUI Kitchen** checkpoints the active source state, installs the `comfy-kitchen` requirement declared by the current ComfyUI checkout, clears the tracked source target, and deactivates the source-built runtime override without deleting the source checkout; later **Update all** runs therefore cannot silently reactivate it\n''',
    "README durable Kitchen restore semantics",
)
replace_once(
    readme,
    '''Use **Restore ComfyUI Kitchen** on the Kitchen repo card to return the runtime to the requirement declared by the currently checked-out ComfyUI core.\n''',
    '''Use **Restore ComfyUI Kitchen** on the Kitchen repo card to return the runtime to the requirement declared by the currently checked-out ComfyUI core. The source checkout remains on disk, but its tracked source target is cleared so this state survives later **Update all** operations. A checkpoint is recorded first, so **Rollback latest** can restore the prior source-managed state.\n''',
    "README durable Kitchen restore usage",
)
