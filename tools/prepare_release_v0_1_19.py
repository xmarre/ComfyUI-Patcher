from pathlib import Path


def replace_bytes(path: str, old: str, new: str, expected: int = 1) -> None:
    target = Path(path)
    data = target.read_bytes()
    newline = "\r\n" if b"\r\n" in data else "\n"

    def encode_pattern(value: str) -> bytes:
        normalized = value.replace("\r\n", "\n")
        return normalized.replace("\n", newline).encode("utf-8")

    old_bytes = encode_pattern(old)
    new_bytes = encode_pattern(new)
    count = data.count(old_bytes)
    if count != expected:
        raise SystemExit(
            f"{path}: expected {expected} occurrence(s) of {old!r}, found {count}"
        )
    target.write_bytes(data.replace(old_bytes, new_bytes))


replace_bytes(
    "package.json",
    '"name": "comfyui-patcher",\n  "private": true,\n  "version": "0.1.18"',
    '"name": "comfyui-patcher",\n  "private": true,\n  "version": "0.1.19"',
)
replace_bytes(
    "package-lock.json",
    '"name": "comfyui-patcher",\n  "version": "0.1.18",\n  "lockfileVersion"',
    '"name": "comfyui-patcher",\n  "version": "0.1.19",\n  "lockfileVersion"',
)
replace_bytes(
    "package-lock.json",
    '"": {\n      "name": "comfyui-patcher",\n      "version": "0.1.18"',
    '"": {\n      "name": "comfyui-patcher",\n      "version": "0.1.19"',
)
replace_bytes(
    "src-tauri/Cargo.toml",
    'name = "comfyui-patcher"\nversion = "0.1.18"',
    'name = "comfyui-patcher"\nversion = "0.1.19"',
)
replace_bytes(
    "src-tauri/Cargo.lock",
    'name = "comfyui-patcher"\nversion = "0.1.18"\n',
    'name = "comfyui-patcher"\nversion = "0.1.19"\n',
)
replace_bytes(
    "src-tauri/tauri.conf.json",
    '"productName": "ComfyUI Patcher",\n  "version": "0.1.18"',
    '"productName": "ComfyUI Patcher",\n  "version": "0.1.19"',
)
replace_bytes(
    ".github/release-request.json",
    '"version": "0.1.18"',
    '"version": "0.1.19"',
)

changelog_path = Path("CHANGELOG.md")
changelog = changelog_path.read_bytes().decode("utf-8")
newline = "\r\n" if "\r\n" in changelog else "\n"
start = changelog.index(f"## Unreleased{newline}")
end = changelog.index(f"## [0.1.18] - 2026-08-30{newline}")
release_section = """## [0.1.19] - 2026-09-10

This release adds first-class management for the official `Comfy-Org/comfy-kitchen` source project, keeping its Git checkout and the compiled Python runtime coherent under the same patch/update/rollback lifecycle as the rest of ComfyUI Patcher.

### Added

- Added first-class Comfy Kitchen management as a separate repository/runtime layer, including official-source target resolution, stacked PR support, recursive submodule initialization, source-wheel materialization, and runtime provenance tracking.
- Added an installation-level Kitchen runtime probe and UI that reports installed/import state independently of whether a source checkout is managed.
- Added durable **Restore ComfyUI Kitchen** behavior that returns runtime ownership to the requirement declared by the current ComfyUI checkout without deleting the source checkout.

### Fixed

- Patcher-owned root `.patcher-build` output is excluded from meaningful Git dirtiness without hiding similarly named or nested user paths, preventing interrupted/interleaved Kitchen builds from triggering false dirty-worktree strategies.
- Pending Kitchen preview requests are invalidated after installation registration/update and successful source installation so stale asynchronous previews cannot repopulate cleared UI state.

### Safety

- Patcher reasserts an active managed Kitchen source build after Patcher-controlled Python dependency installs replace it, defers that reassertion to one final Kitchen pass during installation-wide operations, and blocks Start/Restart when active Kitchen runtime provenance is incoherent.
- Kitchen Untrack preserves the currently installed runtime while stopping future source reassertion; Disable/Uninstall restore the requirement owned by the current ComfyUI checkout first and recover the previous source runtime if a later lifecycle mutation fails.
- Kitchen rollback/checkpoint restore includes runtime materialization state; Restore ComfyUI Kitchen retains dirty-worktree recovery data without leaving the user's checkout stashed, and inactive source checkouts no longer expose tracked Update actions. Failed fresh source installs clean up only operation-owned state and preserve/restore retained pre-existing paths.
- A failed first Kitchen source build preserves an unchanged pre-existing unmanaged runtime; ComfyUI requirement restoration is reserved for failed attempts that changed or could not verify that runtime.

""".replace("\n", newline)
changelog = changelog[:start] + release_section + changelog[end:]
compare_anchor = (
    "[0.1.18]: https://github.com/xmarre/ComfyUI-Patcher/compare/"
    "v0.1.17...v0.1.18"
)
if compare_anchor not in changelog:
    raise SystemExit("CHANGELOG.md: v0.1.18 compare anchor not found")
changelog = changelog.replace(
    compare_anchor,
    "[0.1.19]: https://github.com/xmarre/ComfyUI-Patcher/compare/"
    f"v0.1.18...v0.1.19{newline}"
    + compare_anchor,
    1,
)
changelog_path.write_bytes(changelog.encode("utf-8"))
