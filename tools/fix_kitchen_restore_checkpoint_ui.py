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
    if text.count(start_marker) != 1 or text.count(end_marker) != 1:
        raise SystemExit(
            f"{label}: expected unique markers, found start={text.count(start_marker)}, end={text.count(end_marker)}"
        )
    start = text.index(start_marker)
    end = text.index(end_marker, start)
    p.write_text(text[:start] + replacement + text[end:], encoding="utf-8", newline="\n")


checkpoint_fn = '''async fn create_kitchen_runtime_checkpoint(
    state: &AppState,
    installation: &Installation,
    repo: &ManagedRepo,
    operation_id: &str,
) -> AppResult<RepoCheckpoint> {
    if repo.kind != RepoKind::Kitchen {
        return Err(AppError::InvalidInput(
            "Kitchen runtime checkpoint requires a Kitchen repository".to_string(),
        ));
    }
    let path = Path::new(&repo.local_path);
    let checkpoint = create_checkpoint_if_needed(
        state,
        installation,
        repo,
        None,
        operation_id,
        &DirtyRepoStrategy::Stash,
    )
    .await?;

    // Restore ComfyUI Kitchen does not mutate the source checkout. If the user
    // has local work, retain an immutable recovery stash in the checkpoint but
    // immediately put the worktree back exactly where the user left it.
    if checkpoint.stash_created {
        let stash_ref = checkpoint.stash_ref.as_deref().ok_or_else(|| {
            AppError::Git(
                "Kitchen runtime checkpoint recorded a stash without its recovery reference"
                    .to_string(),
            )
        })?;
        if let Err(apply_error) = apply_stash_keep(path, stash_ref).await {
            let compensation = async {
                reset_hard(path, &checkpoint.old_head_sha).await?;
                let status = inspect_repo(path).await?;
                if !status.untracked_files.is_empty() {
                    clean_untracked_paths(path, &status.untracked_files).await?;
                }
                apply_stash_keep(path, stash_ref).await?;
                Ok::<(), AppError>(())
            }
            .await;
            return match compensation {
                Ok(()) => Err(apply_error),
                Err(compensation_error) => Err(AppError::Git(format!(
                    "{apply_error}; additionally failed to restore the Kitchen worktree from its retained checkpoint stash: {compensation_error}"
                ))),
            };
        }
    }
    Ok(checkpoint)
}

'''
replace_region(
    "src-tauri/src/lib.rs",
    "async fn create_kitchen_runtime_checkpoint(\n",
    "#[tauri::command]\nasync fn restore_comfy_managed_kitchen(",
    checkpoint_fn,
    "stash-backed Kitchen runtime checkpoint",
)

replace_once(
    "src-tauri/src/lib.rs",
    '''    restore_checkpoint_state(state, path, &repo.id, checkpoint, false).await?;
    if let Some(saved_materialization) = checkpoint.materialization_state.as_ref() {''',
    '''    // A Restore ComfyUI Kitchen checkpoint can intentionally retain a stash
    // while leaving the user's worktree dirty. When that checkpoint is restored,
    // its stash is the recovery copy for those untracked paths, so remove only
    // those currently visible untracked files before resetting/rebuilding source.
    if restore_stash && checkpoint.stash_created {
        let current_status = inspect_repo(path).await?;
        if !current_status.untracked_files.is_empty() {
            clean_untracked_paths(path, &current_status.untracked_files).await?;
        }
    }

    restore_checkpoint_state(state, path, &repo.id, checkpoint, false).await?;
    if let Some(saved_materialization) = checkpoint.materialization_state.as_ref() {''',
    "prepare stash-backed Kitchen checkpoint restore",
)

restore_start = "async fn run_restore_comfy_managed_kitchen(\n"
restore_end = "struct CustomNodeInstallRunResult"
lib_path = Path("src-tauri/src/lib.rs")
lib_text = lib_path.read_text(encoding="utf-8")
start = lib_text.index(restore_start)
end = lib_text.index(restore_end, start)
region = lib_text[start:end]
old = '''                &checkpoint,
                false,
                true,
                None,
            )'''
new = '''                &checkpoint,
                true,
                true,
                None,
            )'''
if region.count(old) != 1:
    raise SystemExit(f"Restore ComfyUI recovery stash: expected 1 regional match, found {region.count(old)}")
lib_path.write_text(lib_text[:start] + region.replace(old, new, 1) + lib_text[end:], encoding="utf-8", newline="\n")

replace_once(
    "src/components/RepoCard.tsx",
    '''  const integrationBranch = trackedState?.materializedBranch ?? repo.currentBranch ?? "detached";
  const hasOverlays = overlays.length > 0;
  const lifecycleSupported = repo.kind !== "core";''',
    '''  const integrationBranch = trackedState?.materializedBranch ?? repo.currentBranch ?? "detached";
  const hasOverlays = overlays.length > 0;
  const hasTrackedUpdate =
    trackedState !== null || (repo.trackedTargetKind !== null && repo.trackedTargetInput !== null);
  const lifecycleSupported = repo.kind !== "core";''',
    "tracked update availability",
)

replace_once(
    "src/components/RepoCard.tsx",
    '''      <div className="row gap repo-action-wrap">
        <button
          className="secondary"
          disabled={isSubmitting}
          onClick={() =>
            void runLocalAction(async () => {
              const preview = await api.previewTrackedRepoUpdate(repo.id);
              setUpdatePreview(preview);
            })
          }
        >
          Preview update
        </button>
        {onUpdate ? (
          <button disabled={isSubmitting} onClick={() => void runLocalAction(onUpdate)}>
            Update
          </button>
        ) : null}''',
    '''      <div className="row gap repo-action-wrap">
        {hasTrackedUpdate ? (
          <button
            className="secondary"
            disabled={isSubmitting}
            onClick={() =>
              void runLocalAction(async () => {
                const preview = await api.previewTrackedRepoUpdate(repo.id);
                setUpdatePreview(preview);
              })
            }
          >
            Preview update
          </button>
        ) : null}
        {hasTrackedUpdate && onUpdate ? (
          <button disabled={isSubmitting} onClick={() => void runLocalAction(onUpdate)}>
            Update
          </button>
        ) : null}''',
    "hide inactive tracked update actions",
)

replace_once(
    "src/components/RepoCard.tsx",
    '''                    "Restore the comfy-kitchen requirement declared by the current ComfyUI checkout? The source checkout stays on disk, but its tracked source target and built runtime override are deactivated. Use Install / Patch source to enable source management again."''',
    '''                    "Restore the comfy-kitchen requirement declared by the current ComfyUI checkout? The source checkout stays on disk, but its tracked source target and built runtime override are deactivated. Local source changes stay in place and are retained in the rollback checkpoint. Use Install / Patch source or set a tracked base target to enable source management again."''',
    "Kitchen restore confirmation",
)

replace_once(
    "src/components/RepoCard.tsx",
    '''      {renderPreview(updatePreview, "Preview the tracked update plan to inspect incoming commits and files before mutating the checkout.")}''',
    '''      {hasTrackedUpdate
        ? renderPreview(updatePreview, "Preview the tracked update plan to inspect incoming commits and files before mutating the checkout.")
        : null}''',
    "hide stale tracked update preview",
)

replace_once(
    "README.md",
    '''**Untrack** stops source management/reassertion but deliberately leaves the currently installed Kitchen package untouched. **Restore ComfyUI Kitchen**, **Disable**, and **Uninstall** return runtime ownership to the `comfy-kitchen` requirement declared by the current managed ComfyUI checkout before deactivating/removing the source override.''',
    '''**Untrack** stops source management/reassertion but deliberately leaves the currently installed Kitchen package untouched. **Restore ComfyUI Kitchen**, **Disable**, and **Uninstall** return runtime ownership to the `comfy-kitchen` requirement declared by the current managed ComfyUI checkout before deactivating/removing the source override. Restore checkpoints preserve an existing dirty Kitchen worktree through Patcher's stash-backed checkpoint mechanism while immediately reapplying the user's local work; tracked Update actions are disabled once Restore has deliberately cleared the source target.''',
    "README Restore checkpoint semantics",
)

replace_once(
    "CHANGELOG.md",
    '''- Kitchen rollback/checkpoint restore includes runtime materialization state; failed fresh source installs clean up only operation-owned state and preserve/restore retained pre-existing paths.''',
    '''- Kitchen rollback/checkpoint restore includes runtime materialization state; Restore ComfyUI Kitchen retains dirty-worktree recovery data without leaving the user's checkout stashed, and inactive source checkouts no longer expose tracked Update actions. Failed fresh source installs clean up only operation-owned state and preserve/restore retained pre-existing paths.''',
    "CHANGELOG Restore checkpoint safety",
)

print("Kitchen Restore checkpoint/UI transformations applied")
