from pathlib import Path


def replace_once(path: Path, old: str, new: str, label: str) -> None:
    text = path.read_text()
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{label}: expected exactly one match, found {count}")
    path.write_text(text.replace(old, new, 1))


lib = Path("src-tauri/src/lib.rs")
text = lib.read_text()

old = '''    let installation_lock = state.installation_lock(&installation.id).await;\n    let _installation_guard = installation_lock.lock().await;\n    let result = async {\n'''
# This shape appears in frontend and Kitchen runners; scope the replacement to the Kitchen function only.
start = text.index("async fn run_install_or_patch_kitchen(")
end = text.index("\n#[tauri::command]\nasync fn restore_comfy_managed_kitchen", start)
segment = text[start:end]
if segment.count(old) != 1:
    raise SystemExit(f"Kitchen transaction state anchor: expected one match, found {segment.count(old)}")
segment = segment.replace(
    old,
    '''    let installation_lock = state.installation_lock(&installation.id).await;\n    let _installation_guard = installation_lock.lock().await;\n    let mut replaced_backup_path: Option<PathBuf> = None;\n    let mut created_repo_id: Option<String> = None;\n    let mut created_target_path = false;\n    let mut source_apply_completed = false;\n    let result = async {\n''',
    1,
)
old_local = "        let mut replaced_backup_path: Option<PathBuf> = None;\n\n"
if segment.count(old_local) != 1:
    raise SystemExit("Kitchen local backup state anchor mismatch")
segment = segment.replace(old_local, "", 1)
old_clone = '''            log_operation(\n                &state,\n                &app,\n                &operation_id,\n                "clone",\n                "info",\n                format!("cloning {}", resolved.canonical_repo_url),\n            );\n            clone_repo(&resolved.fetch_url, &target_path).await?;\n            let status = inspect_repo(&target_path).await?;\n'''
new_clone = '''            log_operation(\n                &state,\n                &app,\n                &operation_id,\n                "clone",\n                "info",\n                format!("cloning {}", resolved.canonical_repo_url),\n            );\n            // The target did not exist when this operation reached clone (or was\n            // moved to the retained backup above), so cleanup may safely remove\n            // anything clone creates here if source application fails.\n            created_target_path = true;\n            clone_repo(&resolved.fetch_url, &target_path).await?;\n            let status = inspect_repo(&target_path).await?;\n'''
if segment.count(old_clone) != 1:
    raise SystemExit("Kitchen clone ownership anchor mismatch")
segment = segment.replace(old_clone, new_clone, 1)
old_repo = '''            let repo = state.db.upsert_repo(\n                &installation.id,\n                RepoKind::Kitchen,\n                "Comfy Kitchen",\n                &target_path.to_string_lossy(),\n                status.origin_url.as_deref(),\n                status.head_sha.as_deref(),\n                status.branch.as_deref(),\n                status.is_detached,\n                repo_has_tracked_local_changes(&status),\n            )?;\n            let repo_lock = state.repo_lock(&repo.id).await;\n'''
new_repo = '''            let repo = state.db.upsert_repo(\n                &installation.id,\n                RepoKind::Kitchen,\n                "Comfy Kitchen",\n                &target_path.to_string_lossy(),\n                status.origin_url.as_deref(),\n                status.head_sha.as_deref(),\n                status.branch.as_deref(),\n                status.is_detached,\n                repo_has_tracked_local_changes(&status),\n            )?;\n            created_repo_id = Some(repo.id.clone());\n            let repo_lock = state.repo_lock(&repo.id).await;\n'''
# There are two Kitchen upserts in the function; only the fresh-clone one is after created_target_path.
pos = segment.index("            created_target_path = true;")
tail = segment[pos:]
if tail.count(old_repo) != 1:
    raise SystemExit(f"fresh Kitchen repo ownership anchor: expected one match after clone, found {tail.count(old_repo)}")
tail = tail.replace(old_repo, new_repo, 1)
segment = segment[:pos] + tail
old_apply = '''            apply_repo_tracking_state(\n                &state,\n                &app,\n                &operation_id,\n                &installation,\n                &repo,\n                &tracked_state,\n                &DirtyRepoStrategy::Abort,\n                false,\n                input.set_tracked_target,\n            )\n            .await?\n'''
new_apply = '''            let checkpoint = apply_repo_tracking_state(\n                &state,\n                &app,\n                &operation_id,\n                &installation,\n                &repo,\n                &tracked_state,\n                &DirtyRepoStrategy::Abort,\n                false,\n                input.set_tracked_target,\n            )\n            .await?;\n            source_apply_completed = true;\n            checkpoint\n'''
if segment.count(old_apply) != 1:
    raise SystemExit("fresh Kitchen apply completion anchor mismatch")
segment = segment.replace(old_apply, new_apply, 1)
old_error = '''    if let Err(err) = result {\n        state.db.finish_operation(\n            &operation_id,\n            OperationStatus::Failed,\n            Some(&err.to_string()),\n            None,\n        )?;\n        log_operation(\n            &state,\n            &app,\n            &operation_id,\n            "error",\n            "error",\n            err.to_string(),\n        );\n        return Err(err);\n    }\n'''
new_error = '''    if let Err(err) = result {\n        let mut cleanup_errors = Vec::new();\n        if created_target_path && !source_apply_completed {\n            if let Some(repo_id) = created_repo_id.as_deref() {\n                match state.db.list_checkpoints(repo_id) {\n                    Ok(checkpoints) => {\n                        for checkpoint in checkpoints\n                            .into_iter()\n                            .filter(|checkpoint| checkpoint.operation_id == operation_id)\n                        {\n                            if let Err(cleanup_error) = state.db.delete_checkpoint(&checkpoint.id) {\n                                cleanup_errors.push(format!(\n                                    "failed to delete failed Kitchen checkpoint {}: {}",\n                                    checkpoint.id, cleanup_error\n                                ));\n                            }\n                        }\n                    }\n                    Err(cleanup_error) => cleanup_errors.push(format!(\n                        "failed to enumerate failed Kitchen checkpoints: {}",\n                        cleanup_error\n                    )),\n                }\n                if let Err(cleanup_error) = state.db.delete_repo(repo_id) {\n                    cleanup_errors.push(format!(\n                        "failed to delete failed Kitchen repo state {}: {}",\n                        repo_id, cleanup_error\n                    ));\n                }\n            }\n            let target_path = kitchen::default_kitchen_path(&installation);\n            if target_path.exists() {\n                if let Err(cleanup_error) = remove_path_with_retries(&target_path).await {\n                    cleanup_errors.push(format!(\n                        "failed to remove Kitchen checkout created by the failed operation: {}",\n                        cleanup_error\n                    ));\n                }\n            }\n            if let Some(backup_path) = replaced_backup_path.as_ref() {\n                if !target_path.exists() {\n                    if let Err(cleanup_error) = std::fs::rename(backup_path, &target_path) {\n                        cleanup_errors.push(format!(\n                            "failed to restore retained Kitchen path {}: {}",\n                            backup_path.to_string_lossy(),\n                            cleanup_error\n                        ));\n                    } else {\n                        log_operation(\n                            &state,\n                            &app,\n                            &operation_id,\n                            "rollback",\n                            "warn",\n                            format!(\n                                "restored retained Kitchen path {} after failed source installation",\n                                backup_path.to_string_lossy()\n                            ),\n                        );\n                    }\n                }\n            }\n        }\n\n        let final_error = if cleanup_errors.is_empty() {\n            err\n        } else {\n            AppError::Io(format!(\n                "{}; additionally failed to clean up the failed Kitchen installation: {}",\n                err,\n                cleanup_errors.join("; ")\n            ))\n        };\n        state.db.finish_operation(\n            &operation_id,\n            OperationStatus::Failed,\n            Some(&final_error.to_string()),\n            None,\n        )?;\n        log_operation(\n            &state,\n            &app,\n            &operation_id,\n            "error",\n            "error",\n            final_error.to_string(),\n        );\n        return Err(final_error);\n    }\n'''
if segment.count(old_error) != 1:
    raise SystemExit(f"Kitchen error cleanup anchor: expected one match, found {segment.count(old_error)}")
segment = segment.replace(old_error, new_error, 1)
text = text[:start] + segment + text[end:]
lib.write_text(text)

readme = Path("README.md")
replace_once(
    readme,
    '''It currently manages three repository kinds:\n\n* **core** — the main ComfyUI repository at the installation root\n* **frontend** — a dedicated managed `ComfyUI_frontend` checkout outside `custom_nodes`\n* **custom_node** — repositories under `custom_nodes/`\n''',
    '''It currently manages four repository kinds:\n\n* **core** — the main ComfyUI repository at the installation root\n* **frontend** — a dedicated managed `ComfyUI_frontend` checkout outside `custom_nodes`\n* **kitchen** — an optional dedicated `Comfy-Org/comfy-kitchen` source checkout whose built runtime artifact is tracked separately from Git state\n* **custom_node** — repositories under `custom_nodes/`\n''',
    "README repository kinds",
)
replace_once(
    readme,
    '''* `deps.rs` — dependency detection / planning / execution for Python and frontend package managers\n* `process.rs` — starts / stops / restarts a managed child process from a launch profile\n''',
    '''* `deps.rs` — dependency detection / planning / execution for Python and frontend package managers\n* `kitchen.rs` — Comfy Kitchen runtime probing, current-ComfyUI requirement parsing, source-wheel build/install, and runtime provenance evaluation\n* `process.rs` — starts / stops / restarts a managed child process from a launch profile\n''',
    "README Kitchen backend module",
)
replace_once(
    readme,
    '''* `src/App.tsx` — primary shell with installation registration, installation settings, core/frontend/custom-node panels, repo cards, registry browser, event stream, and operation panel\n''',
    '''* `src/App.tsx` — primary shell with installation registration, installation settings, core/frontend/Kitchen/custom-node panels, repo cards, registry browser, event stream, and operation panel\n''',
    "README frontend module description",
)
replace_once(
    readme,
    '''│     ├─ github.rs\n│     ├─ lib.rs\n''',
    '''│     ├─ github.rs\n│     ├─ kitchen.rs\n│     ├─ lib.rs\n''',
    "README file tree Kitchen module",
)
replace_once(
    readme,
    '''  * core repo at the ComfyUI root\n  * frontend repo at the configured managed frontend path\n  * git-backed custom nodes under `custom_nodes/`\n''',
    '''  * core repo at the ComfyUI root\n  * frontend repo at the configured managed frontend path\n  * the official Comfy Kitchen repo at the dedicated sibling source path, when that checkout exists\n  * git-backed custom nodes under `custom_nodes/`\n''',
    "README Kitchen discovery",
)
replace_once(
    readme,
    '''* target resolution is repo-kind aware: `core`, `frontend`, or `custom_node`\n''',
    '''* target resolution is repo-kind aware: `core`, `frontend`, `kitchen`, or `custom_node`\n''',
    "README target kinds",
)

kitchen_doc = '''\n### Managed Comfy Kitchen source support\n\nComfy Kitchen is treated as a first-class project materialization, not as another generic Python dependency list. An ordinary `comfy-kitchen` package installed by ComfyUI remains runtime state only; Patcher does not invent a managed Git repository for it. Source management begins only when the official `Comfy-Org/comfy-kitchen` checkout exists or the user explicitly installs/patches it through Patcher.\n\nSource-managed Kitchen behavior:\n\n* the source checkout uses a dedicated sibling `comfy-kitchen` path outside the ComfyUI repository and `custom_nodes/`\n* source targets must resolve to the official `Comfy-Org/comfy-kitchen` repository\n* recursive Git submodule initialization/update is mandatory; a submodule failure aborts materialization\n* the build runs with the installation's exact configured Python executable\n* Patcher builds a wheel with `pip wheel --no-deps` before changing the environment, then installs that completed wheel with `pip install --no-deps --force-reinstall`\n* Kitchen's own setuptools/CMake configuration remains authoritative for CUDA/HIP/native architecture policy\n* source HEAD, built-wheel SHA-256, installed distribution version, installed `RECORD` SHA-256, import result, and timestamps are tracked independently from Git target state\n* after Patcher-controlled core/custom-node Python dependency installs, an active source override is probed and reasserted if that install replaced the Kitchen artifact while the saved source checkout is still coherent\n* **Update all** and tracked-repo rematerialization process Kitchen after core, frontend, and custom nodes so a source override is the final Python artifact state\n* rollback/checkpoint restore rebuilds the saved Kitchen source revision (or restores the current ComfyUI requirement when the checkpoint had no source override) before reporting success\n* **Restore ComfyUI Kitchen** installs the `comfy-kitchen` requirement declared by the current ComfyUI checkout and deactivates the source-built runtime override without fabricating or deleting Git history\n* uninstall/disable/untrack restore the current ComfyUI requirement before removing active source management\n* Start/Restart fail before launching if an active source-managed Kitchen runtime is missing, import-broken, externally replaced, or stale relative to the managed source checkout\n\nA Kitchen rollback rebuilds the saved source revision; it does not claim byte-for-byte reproduction of a previous native wheel. If a checkpoint was captured while its saved source/runtime provenance was already inconsistent, restore fails rather than reporting a Git-only success.\n\n'''
replace_once(
    readme,
    '''### Custom node install / patch\n''',
    kitchen_doc + '''### Custom node install / patch\n''',
    "README Kitchen feature section",
)
replace_once(
    readme,
    '''* update all tracked repos of an installation\n* include frontend repos in **Update all**\n''',
    '''* update all tracked repos of an installation\n* include frontend repos in **Update all**\n* materialize a tracked Kitchen source checkout last so later Python dependency installs cannot silently win over the source override\n''',
    "README Kitchen update ordering",
)
replace_once(
    readme,
    '''* rerun dependency sync after rollback\n''',
    '''* rerun dependency sync after rollback\n* for source-managed Kitchen, restore both the Git revision and the corresponding runtime materialization before success is reported\n''',
    "README Kitchen rollback",
)
replace_once(
    readme,
    '''### Frontend repos\n''',
    '''### Comfy Kitchen project materialization\n\nKitchen does not use the generic Python dependency-plan executor. Patcher initializes its recursive submodules, builds a wheel from the selected checkout, and installs that completed wheel into the configured installation Python environment. Build/native policy remains owned by the Kitchen project itself.\n\n### Frontend repos\n''',
    "README Kitchen dependency section",
)
replace_once(
    readme,
    '''### 4. Install or patch a custom node manually\n''',
    '''### 4. Install or patch Comfy Kitchen from source\n\nThe Kitchen source panel defaults to the official repository URL:\n\n* `https://github.com/Comfy-Org/comfy-kitchen`\n\nFor an existing managed Kitchen checkout, branch names, commits, tree URLs, and PR URLs for that same official repository are also valid. Preview the target before applying it. Patcher initializes the repository's recursive submodules, builds the source wheel using the installation Python environment, installs it, and verifies `import comfy_kitchen`.\n\nUse **Restore ComfyUI Kitchen** on the Kitchen repo card to return the runtime to the requirement declared by the currently checked-out ComfyUI core.\n\n### 5. Install or patch a custom node manually\n''',
    "README Kitchen usage step",
)
replace_once(readme, "### 5. Use the ComfyUI-Manager registry browser\n", "### 6. Use the ComfyUI-Manager registry browser\n", "README renumber manager")
replace_once(readme, "### 6. Update and rollback\n", "### 7. Update and rollback\n", "README renumber update")
replace_once(readme, "### 7. Start / Stop / Restart\n", "### 8. Start / Stop / Restart\n", "README renumber lifecycle")
replace_once(
    readme,
    '''4. **Dependency sync is manifest-driven.**\n   The app does not try to discover arbitrary project-specific install scripts beyond the supported Python / frontend flows.\n''',
    '''4. **Generic dependency sync is manifest-driven.**\n   The app does not try to discover arbitrary project-specific install scripts beyond the supported Python / frontend flows. Comfy Kitchen is an explicit project-materialization exception: Patcher invokes the project's normal wheel build after mandatory submodule initialization instead of interpreting Kitchen's native build policy itself.\n''',
    "README Kitchen dependency assumption",
)
replace_once(
    readme,
    '''* if a managed frontend repo and ComfyUI install live on different filesystems, replacement / backup handling is designed to avoid cross-device rename failures by backing up beside the target path\n''',
    '''* if a managed frontend repo and ComfyUI install live on different filesystems, replacement / backup handling is designed to avoid cross-device rename failures by backing up beside the target path\n* a Kitchen rollback rebuilds the saved source revision; native build output is verified for runtime coherence and provenance, but byte-identical reproduction of the prior wheel is not assumed\n* a failed Kitchen restore is surfaced when saved Git/runtime provenance is internally inconsistent rather than silently accepting a Git-only rollback\n''',
    "README Kitchen caveats",
)
replace_once(
    readme,
    '''* confirm a configured frontend repo is discovered when present\n* re-register the same root and confirm the existing entry is updated instead of duplicated\n''',
    '''* confirm a configured frontend repo is discovered when present\n* confirm an ordinary installed `comfy-kitchen` distribution does not create a managed Kitchen repo\n* confirm the official sibling Kitchen checkout is discovered when present\n* re-register the same root and confirm the existing entry is updated instead of duplicated\n''',
    "README Kitchen registration validation",
)

changelog = Path("CHANGELOG.md")
replace_once(
    changelog,
    '''remain available on the [GitHub Releases](https://github.com/xmarre/ComfyUI-Patcher/releases) page.\n\n## [0.1.18] - 2026-08-30\n''',
    '''remain available on the [GitHub Releases](https://github.com/xmarre/ComfyUI-Patcher/releases) page.\n\n## [Unreleased]\n\n### Added\n\n- Added first-class Comfy Kitchen source management using the official sibling checkout, the installation's exact Python environment, mandatory recursive submodules, build-before-install wheel materialization, and persisted runtime provenance.\n- Added Kitchen runtime reconciliation, source-override reassertion after Patcher-controlled Python dependency installs, Kitchen-last installation-wide updates, and launch-time fail-fast validation.\n- Added Kitchen-aware rollback/checkpoint restore and an explicit action to restore the `comfy-kitchen` requirement declared by the current ComfyUI checkout.\n\n### Safety\n\n- Ordinary environment-installed `comfy-kitchen` packages remain runtime state and do not fabricate managed Git repositories.\n- Kitchen native CUDA/HIP/build policy remains owned by the Kitchen project; Patcher does not duplicate architecture selection logic.\n- Kitchen rollback rebuilds the saved source revision and verifies runtime provenance rather than claiming byte-identical native wheel reproduction.\n\n## [0.1.18] - 2026-08-30\n''',
    "CHANGELOG Kitchen unreleased section",
)
