from pathlib import Path

path = Path("tools/apply-kitchen-backend.py")
text = path.read_text()

bad_restart = '''replace_once(
    \'\'\'    let require_managed_frontend_dist = state
        .db
        .get_installation_detail(&installation.id)?
        .frontend_repo
        .is_some();
    let profile = effective_launch_profile(installation, require_managed_frontend_dist)?;
\'\'\',
    \'\'\'    ensure_kitchen_runtime_ready_for_launch(state, installation).await?;
    let require_managed_frontend_dist = state
        .db
        .get_installation_detail(&installation.id)?
        .frontend_repo
        .is_some();
    let profile = effective_launch_profile(installation, require_managed_frontend_dist)?;
\'\'\',
    "restart-after-success Kitchen fail-fast",
)

'''
if text.count(bad_restart) != 1:
    raise SystemExit("could not remove ambiguous maybe-restart transformation")
text = text.replace(bad_restart, "", 1)

for old in [
    "        let mut created_repo_id: Option<String> = None;\n",
    "        let mut created_new_checkout = false;\n",
    "            created_new_checkout = true;\n",
    "            created_repo_id = Some(repo.id.clone());\n",
]:
    if text.count(old) != 1:
        raise SystemExit(f"expected one temporary cleanup marker: {old!r}")
    text = text.replace(old, "", 1)

old_compare = "repo.local_path == target_path.to_string_lossy())"
if text.count(old_compare) != 1:
    raise SystemExit("Kitchen path comparison marker mismatch")
text = text.replace(
    old_compare,
    "repo.local_path == target_path.to_string_lossy().as_ref())",
    1,
)

start_marker = '''    if let Err(err) = result {
        let target_path = kitchen::default_kitchen_path(&installation);
'''
end_marker = '''    Ok(())
}

#[tauri::command]
async fn restore_comfy_managed_kitchen'''
start = text.find(start_marker)
end = text.find(end_marker, start)
if start < 0 or end < 0:
    raise SystemExit("Kitchen install error-cleanup markers were not found")
replacement = '''    if let Err(err) = result {
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
'''
text = text[:start] + replacement + text[end:]

path.write_text(text)
