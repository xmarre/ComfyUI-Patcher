from pathlib import Path

path = Path("README.md")
text = path.read_text(encoding="utf-8")
old = "**Untrack** stops source management/reassertion but deliberately leaves the currently installed Kitchen package untouched. **Restore ComfyUI Kitchen**, **Disable**, and **Uninstall** return runtime ownership to the `comfy-kitchen` requirement declared by the current managed ComfyUI checkout before deactivating/removing the source override."
anchor = "- **Disable / Uninstall**: before moving or deleting an active source checkout, Patcher restores and verifies the Kitchen requirement declared by the current managed ComfyUI core checkout. A failed restore aborts the filesystem mutation."
if text.count(old) != 1:
    raise SystemExit(f"README lifecycle paragraph: expected 1 match, found {text.count(old)}")
if anchor in text:
    raise SystemExit("README lifecycle anchor already exists")
text = text.replace(old, old + "\n\n" + anchor, 1)
path.write_text(text, encoding="utf-8", newline="\n")
print("Prepared current README lifecycle anchor")
