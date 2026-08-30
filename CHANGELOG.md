# Changelog

All notable changes to ComfyUI Patcher are documented here. Earlier release notes
remain available on the [GitHub Releases](https://github.com/xmarre/ComfyUI-Patcher/releases) page.

## [0.1.16] - 2026-08-30

This hotfix makes pull-request resolution resilient to transient GitHub/network failures while keeping same-repository overlays on the local git topology path whenever possible.

### Changed

- Same-repository PR resolution now refreshes only the exact base, pull-head, and pull-merge refs required for topology; a broad `git fetch origin` is now best-effort branch-name enrichment rather than a prerequisite.
- Stacked child PR bases can now be identified directly from earlier tracked-overlay head SHAs before remote branch-name lookup.
- GitHub REST metadata requests retry bounded transient connection/request/timeout failures, HTTP 429, and 5xx responses.

### Fixed

- Fixed valid PR overlays, including Spectrum PR #91, failing immediately when a transient local git refresh pushed resolution onto a one-shot GitHub API request.
- Same-repo pull/base ref fetches now retry before falling back to REST.
- If both local git resolution and the GitHub API fallback fail, the operation now reports both causes instead of hiding the local failure behind a generic API transport error.
- Legacy overlay dependency-metadata hydration now uses the same resilient local-first resolution behavior.

## [0.1.15] - 2026-08-29

This hotfix makes WSL-backed command execution argv-safe so Git arguments are no longer reinterpreted by a Linux shell.

### Fixed

- Internally translated WSL commands now use `wsl.exe --exec`, which executes the target Linux binary without routing the command through the default shell.
- Git arguments containing shell metacharacters are now passed intact, including the stacked-PR branch lookup format `--format=%(refname:short)`.
- Stacked-PR preflight on WSL-backed installations no longer fails with `/bin/bash: syntax error near unexpected token '('` while resolving remote branches.

### Tests

- Added a regression test using the exact failing `for-each-ref --format=%(refname:short)` argument and SHA, asserting the constructed WSL command uses `--exec` and preserves every Git argument separately.

## [0.1.14] - 2026-08-29

This patch release adds dependency-aware support for pull requests stacked on other pull-request branches.

### Added

- Added proper stacked-PR overlay support: a PR may target the tracked repository base or the head branch of an earlier enabled PR overlay.
- Added dependency validation for stacked overlays, including parent-before-child ordering and repository-aware branch identity.
- Added local git topology resolution for same-repository stacked PRs using GitHub pull head/test-merge refs, with GitHub API metadata as a fail-safe fallback.
- Added focused regression coverage for multi-level stacks, disabled prerequisites, invalid ordering, ambiguous intermediate bases, repository collisions, and legacy metadata enrichment.

### Changed

- Dirty-worktree collision preflight now computes each stacked PR's incoming changes from its declared dependency base.
- Conflict preview now probes each PR against its actual declared dependency instead of treating every overlay as independent from the repository base.
- Older locally-resolved overlays are enriched from unambiguous local git refs first, preserving the existing API/rate-limit-safe resolution path.

### Fixed

- Fixed valid stacked PRs being rejected with errors such as `PR overlays for this repo must target base branch 'main'` when a child PR correctly targets its parent PR branch.
- Fixed stack edit operations allowing dependency-invalid states to reach checkpoint or checkout mutation.
- Fixed dependency matching from guessing when branch or repository identity is ambiguous; ambiguous relationships now fail closed.

## [0.1.13] - 2026-08-15

This patch release makes tracked-repository recovery precise so reconciliation repairs only the repositories that actually need it.

### Changed

- Reconciliation warnings now expose a **Repair this repo** action for each tracked repository currently flagged as dirty or drifted.
- The bulk recovery action now hard-resets only the currently flagged repair set and explicitly states how many other tracked repositories will remain untouched.

### Fixed

- Repairing reconciliation drift no longer hard-resets every tracked repository in an installation when only one or a few repositories need recovery.
- Unrelated managed repositories no longer receive recovery operations or checkpoints during normal reconciliation repair.

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

[0.1.16]: https://github.com/xmarre/ComfyUI-Patcher/compare/v0.1.15...v0.1.16
[0.1.15]: https://github.com/xmarre/ComfyUI-Patcher/compare/v0.1.14...v0.1.15
[0.1.14]: https://github.com/xmarre/ComfyUI-Patcher/compare/v0.1.13...v0.1.14
[0.1.13]: https://github.com/xmarre/ComfyUI-Patcher/compare/v0.1.12...v0.1.13
[0.1.12]: https://github.com/xmarre/ComfyUI-Patcher/compare/v0.1.11...v0.1.12
[0.1.11]: https://github.com/xmarre/ComfyUI-Patcher/compare/v0.1.10...v0.1.11
[0.1.10]: https://github.com/xmarre/ComfyUI-Patcher/compare/v0.1.9...v0.1.10
