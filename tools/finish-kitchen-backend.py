from pathlib import Path

path = Path("src-tauri/src/lib.rs")
text = path.read_text()
start = text.index("async fn maybe_restart_installation(")
end = text.index("\nasync fn acquire_background_work_guard(", start)
segment = text[start:end]
old = '''    let require_managed_frontend_dist = state
        .db
        .get_installation_detail(&installation.id)?
        .frontend_repo
        .is_some();
'''
if segment.count(old) != 1:
    raise SystemExit(f"maybe_restart_installation: expected one profile preflight, found {segment.count(old)}")
segment = segment.replace(
    old,
    '''    ensure_kitchen_runtime_ready_for_launch(state, installation).await?;
    let require_managed_frontend_dist = state
        .db
        .get_installation_detail(&installation.id)?
        .frontend_repo
        .is_some();
''',
    1,
)
text = text[:start] + segment + text[end:]
path.write_text(text)
