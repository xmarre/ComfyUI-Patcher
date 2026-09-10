from pathlib import Path

path = Path("src-tauri/src/lib.rs")
text = path.read_text(encoding="utf-8")
old = "    materialize_kitchen_override(state, app, operation_id, installation, &recovered_repo).await\n"
new = "    materialize_kitchen_override(state, app, operation_id, installation, &recovered_repo)\n        .await\n        .map(|_| ())\n"
count = text.count(old)
if count != 1:
    raise SystemExit(f"Kitchen lifecycle recovery result type: expected 1 match, found {count}")
path.write_text(text.replace(old, new, 1), encoding="utf-8", newline="\n")
print("Corrected Kitchen lifecycle recovery result type")
