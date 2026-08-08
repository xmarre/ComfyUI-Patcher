# Changelog

All notable changes to ComfyUI Patcher are documented here. Earlier release notes
remain available on the [GitHub Releases](https://github.com/xmarre/ComfyUI-Patcher/releases) page.

## [0.1.12] - 2026-08-08

This patch release makes UI state updates immediate after repository operations, even for large WSL installations.

### Changed

- Normal installation-detail refreshes now read durable reconciliation snapshots from SQLite instead of rescanning every repository.
- Explicit reconciliation scans custom-node repositories with bounded concurrency while keeping database writes deterministic.
- Windows CI now runs all Rust unit tests before the production backend build.

### Fixed

- Completed installs, updates, repairs, and removals no longer remain invisible while queued full-repository scans finish.
- Operation progress events no longer create a storm of redundant detail rescans.
- Reconciliation diagnostics and timestamps persist across cheap UI reads.
- Manual reconciliation no longer races active repository mutations or allows stale in-flight reads to overwrite its result.

## [0.1.11] - 2026-08-08

This patch release fixes managed-repository updates and reconciliation failures reported in v0.1.10.

### Changed

- Repository mutations launched from the UI now preserve dirty worktrees with the stash strategy, including bulk updates, tracked-target changes, overlay edits, and custom-node adoption.
- GitHub repository, branch, and commit targets now follow repository rename redirects before remote identity validation, including the API fallback path.

### Fixed

- Preserved tracked local configuration changes, such as `ffmpeg_config.ini`, across repository materialization and dependency synchronization.
- Reapplied saved worktrees by immutable stash commit SHA and retained the recovery stash when race-free deletion could not be guaranteed, preventing a shifted `stash@{n}` from deleting unrelated user work.
- Explicit **Reconcile** now removes stale managed custom-node records when their directories have been deleted, while retaining missing core/frontend records and existing non-Git paths as actionable warnings.

## [0.1.10] - 2026-08-08

This cumulative maintenance release contains every merged change since v0.1.9.

### Added

- Added installation reconciliation with live on-disk repo status, warnings, changed-file details, dependency drift, and last-scan timestamps.
- Added previews for target changes and tracked updates, including commits, file changes, dependency effects, and conservative conflict checks.
- Added checkpoint labels, dependency snapshots, comparison, and restore controls.
- Added uninstall, disable, and untrack actions for managed non-core repositories, with ignored-path persistence to prevent unwanted rediscovery.
- Added a bulk **Repair tracked repos** recovery flow that rematerializes tracked state and can optionally synchronize dependencies.
- Added a dismissible notice when dependency synchronization is deferred during stack editing.

### Changed

- Reduced startup Git work by reusing discovered repo status instead of immediately scanning every repository twice.
- Treat untracked runtime and user payload files separately from tracked modifications in managed-repo status and the UI.
- Made Abort-strategy updates collision-aware: unrelated untracked files remain untouched, while incoming paths that would overwrite them are blocked.
- Existing tracked PR overlays now use their persisted metadata instead of requiring another GitHub API lookup.
- Same-repository GitHub PR URLs can be resolved and fetched locally before falling back to the GitHub API.
- Stack-edit actions defer broad dependency synchronization, while explicit updates and required frontend rebuild paths retain targeted synchronization.
- pnpm frontend installs use frozen-lockfile mode to avoid rewriting managed lockfiles.
- Release publishing can now be requested from a reviewed `main` commit; the workflow validates every version source and publishes the matching changelog section with the signed artifacts.

### Fixed

- Ignored untracked generated Python cache artifacts (`__pycache__`, `*.pyc`, and `*.pyo`) when determining whether a repository is dirty, without hiding tracked changes.
- Improved branch and detached-HEAD detection and normalized changed paths across platforms.
- Added post-operation cleanliness checks so materialization, rollback, and checkpoint restore cannot silently persist conflicted tracked state.
- Fixed tracked and same-repository PR overlay operations failing because of avoidable GitHub API rate-limit or authorization errors.
- Fixed frontend overlay synchronization and rebuild recovery when `pnpm-lock.yaml` is stale or has lockfile-only drift.
- Fixed transient Windows directory deletion failures during uninstall with retry and safe staging behavior.
- Fixed force-pushed pull requests failing to refresh cached PR overlay and preview refs with a non-fast-forward fetch rejection. Forced updates are restricted to disposable refs owned by ComfyUI Patcher.

[0.1.12]: https://github.com/xmarre/ComfyUI-Patcher/compare/v0.1.11...v0.1.12
[0.1.11]: https://github.com/xmarre/ComfyUI-Patcher/compare/v0.1.10...v0.1.11
[0.1.10]: https://github.com/xmarre/ComfyUI-Patcher/compare/v0.1.9...v0.1.10
