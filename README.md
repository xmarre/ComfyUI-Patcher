# ComfyUI Patcher

ComfyUI Patcher is a desktop app for managing a local ComfyUI installation plus git-backed extensions around it. It can register an existing ComfyUI root, discover managed repositories, resolve GitHub URLs and raw git targets, apply them safely with checkpoints, sync dependencies, and control a saved launch profile for **Start / Stop / Restart**.

It currently manages four repository kinds:

* **core** — the main ComfyUI repository at the installation root
* **frontend** — a dedicated managed `ComfyUI_frontend` checkout outside `custom_nodes`
* **kitchen** — an opt-in managed `Comfy-Org/comfy-kitchen` source checkout, materialized into the installation's Python environment
* **custom_node** — repositories under `custom_nodes/`

The app supports both direct revision tracking and **stacked PR overlays** on managed repositories.

---

## Architecture

### Product shape

* **Desktop shell:** Tauri 2
* **Backend:** Rust
* **Frontend:** React + TypeScript + Vite
* **Persistence:** SQLite via `rusqlite`
* **Git execution:** system `git` CLI
* **Target resolution:** GitHub REST API for PR metadata, git for actual fetch / checkout / merge
* **Process control:** local child-process management through a saved launch profile

### Backend modules

* `db.rs` — SQLite schema and CRUD for installations, repos, operations, checkpoints, logs, and tracked repo state
* `github.rs` — parses GitHub URLs and resolves repo / branch / commit / PR targets
* `git.rs` — thin system-git wrapper for inspection, fetch, checkout, reset, stash, clone, merge, and submodule update
* `deps.rs` — dependency detection / planning / execution for Python and frontend package managers
* `kitchen.rs` — Comfy Kitchen runtime probing, source-wheel materialization, provenance checks, and restoration of the requirement declared by ComfyUI
* `process.rs` — starts / stops / restarts a managed child process from a launch profile
* `state.rs` — application state, GitHub client, database, process registry, and per-installation / per-repo locks
* `lib.rs` — Tauri command boundary and orchestration for registration, mutation, rollback, update, and logging

### Frontend modules

* `src/App.tsx` — primary shell with installation registration, installation settings, core/frontend/Kitchen/custom-node panels, repo cards, registry browser, event stream, and operation panel
* `src/components/RepoCard.tsx` — repo summary with tracked-base / overlay controls, update, and rollback
* `src/components/OperationPanel.tsx` — operation list and persisted log viewer
* `src/components/ManagerRegistryBrowser.tsx` — ComfyUI-Manager registry browsing and install entrypoint
* `src/api.ts` / `src/types.ts` — typed Tauri command wrappers and DTOs

---

## Repository model

Each managed repo stores:

* local path
* remote URL
* current branch / detached-HEAD state
* current HEAD SHA
* dirty state
* checkpoint history
* tracked update source

Tracked state supports:

* **base target** — branch / tag / commit / repo default branch / PR base
* **overlay list** — typically PR overlays, applied in order; an overlay may target the tracked base branch or the head branch of an earlier enabled overlay

Stack dependency ordering is enforced. A dependent PR cannot be moved ahead of, enabled without, or left enabled after removal of the PR whose head branch it targets. Same-repository stacks resolve dependency branch identity from local git refs and GitHub's pull test-merge ref when that topology is unambiguous, preserving the API/rate-limit-safe path. Older or ambiguous locally-resolved overlay records are enriched from GitHub metadata only when local git cannot prove the relationship.

This lets the app support flows like:

* track a branch directly
* track a commit directly
* track a PR as a managed stack
* keep a base branch and stack multiple PR overlays on top of it

For PR overlays, the app materializes an integration branch such as `patcher/stack` and synthesizes merge commits for the overlays.

---

## Complete file structure

```text
comfyui-patcher/
├─ README.md
├─ index.html
├─ package.json
├─ tsconfig.json
├─ vite.config.ts
├─ src/
│  ├─ App.tsx
│  ├─ api.ts
│  ├─ main.tsx
│  ├─ styles.css
│  ├─ types.ts
│  └─ components/
│     ├─ ManagerRegistryBrowser.tsx
│     ├─ OperationPanel.tsx
│     └─ RepoCard.tsx
├─ src-tauri/
│  ├─ Cargo.toml
│  ├─ build.rs
│  ├─ tauri.conf.json
│  ├─ capabilities/
│  │  └─ default.json
│  └─ src/
│     ├─ db.rs
│     ├─ deps.rs
│     ├─ errors.rs
│     ├─ execution.rs
│     ├─ git.rs
│     ├─ github.rs
│     ├─ kitchen.rs
│     ├─ lib.rs
│     ├─ main.rs
│     ├─ models.rs
│     ├─ process.rs
│     ├─ registry.rs
│     ├─ state.rs
│     └─ util.rs
└─ tests/
   └─ README.md
```

---

## Implemented features

### Installation management

* register a local ComfyUI root
* detect `custom_nodes/`
* detect a likely Python executable if one is not provided
* store editable installation settings:

  * display name
  * Python executable
  * launch profile
  * managed frontend repo root
  * managed frontend dist path
  * managed frontend package manager
* re-register the same root and update the existing installation entry instead of creating a duplicate
* delete an installation entry
* discover git-backed repositories already present:

  * core repo at the ComfyUI root
  * frontend repo at the configured managed frontend path
  * official Comfy Kitchen source checkout at the managed sibling path when present
  * git-backed custom nodes under `custom_nodes/`

### Target resolution

Accepted inputs:

* raw branch name for an existing managed repo
* raw tag name for an existing managed repo
* raw commit SHA for an existing managed repo
* GitHub repo URL
* GitHub branch URL (`/tree/...`)
* GitHub commit URL (`/commit/...`)
* GitHub PR URL (`/pull/<id>`)

Resolution rules:

* PR URLs are resolved through the GitHub API
* repo URLs resolve to the repository default branch
* raw names are resolved against `origin` for existing managed repos
* before a Kitchen checkout exists, bare Kitchen branch/tag/commit inputs resolve against the official `Comfy-Org/comfy-kitchen` upstream
* branch names containing slashes are supported
* target resolution is repo-kind aware: `core`, `frontend`, `kitchen`, or `custom_node`

### Core ComfyUI patching

* resolve target
* checkpoint repo state before mutation
* handle dirty repo according to:

  * `abort`
  * `stash`
  * `hard_reset`
* fetch + checkout / reset target revision
* update submodules
* run dependency sync
* persist tracked target state for later update
* support base-target changes and overlay stacks on the core repo card

### Managed frontend support

* dedicated **Install or patch ComfyUI frontend** flow
* supports fresh frontend install without requiring pre-saved frontend settings
* auto-derives a default managed frontend checkout path when needed
* tracks the frontend as a first-class managed repo separate from `custom_nodes`
* supports the same tracked-base / overlay stack model as other managed repos
* supports branch / commit / PR resolution for the frontend
* supports update / rollback for the frontend repo
* updates the installation launch profile at runtime by injecting managed `--front-end-root`

Frontend dependency support includes:

* `package.json` detection
* package manager selection:

  * `auto`
  * `npm`
  * `pnpm`
  * `yarn`
* dependency install step
* frontend build step

Frontend runtime integration:

* when managed frontend settings are configured, **Start / Restart** strip any existing `--front-end-root` from stored launch args, restart args, and appended args
* then they inject the managed frontend dist path at runtime
* for WSL-backed launch commands, the injected frontend path is rewritten to the Linux path form expected inside WSL

### Managed Comfy Kitchen support

* probe the installed `comfy-kitchen` distribution/import state independently of source-checkout management
* manage the official Comfy Kitchen checkout as a first-class repository with tracked base targets and PR overlays
* initialize required submodules and build/install a source wheel through the configured ComfyUI Python environment
* persist source HEAD plus installed artifact/RECORD provenance so source and runtime drift can be distinguished
* reassert an active source override after Patcher-controlled Python dependency installs replace it
* restore runtime ownership to the exact `comfy-kitchen` requirement declared by the current ComfyUI checkout without deleting the source checkout
* restore Kitchen runtime state together with repository checkpoints and refuse Start/Restart when an active override is incoherent

### Custom node install / patch

* install a new git-backed custom node into `custom_nodes/<name>`
* patch an existing git-backed node if the target path already points to a repo
* preserve canonical remote matching so an existing repo is reused only when it matches the resolved target
* conflict handling for occupied non-git paths:

  * `abort`
  * `replace`
  * `install_with_suffix`

### ComfyUI-Manager registry browsing

* load ComfyUI-Manager registry entries
* search / filter registry entries in the UI
* install through the app instead of leaving the managed workflow
* preserve manager-style custom node directory naming so ComfyUI-Manager compatibility is not broken
* show whether entries are already installed / managed when detectable

### Update

* update a managed repo to its tracked target
* update all tracked repos of an installation
* include frontend and actively tracked Kitchen repos in **Update all**
* reuse the same tracked-state materialization logic for branch, tag, commit, and PR tracking

### Rollback

* restore the last checkpointed repo state
* restore branch vs detached-HEAD state
* optionally restore stashed changes
* rerun dependency sync after rollback

### Process control

* save a launch profile with:

  * launch command
  * launch args
  * launch cwd
  * optional stop command / args
  * optional restart command / args
* **Start / Stop / Restart** through the saved launch profile
* runtime argument injection for the managed frontend
* process registry tied to the installation entry

### Logging and operations

Each mutation creates an operation record and persisted logs. The UI shows:

* recent operations
* live backend events
* persisted operation logs
* per-stage status like:

  * `preflight`
  * `checkpoint`
  * `fetch`
  * `checkout`
  * `dependency_plan`
  * `dependency_sync`
  * `submodules` / `materialization` for Comfy Kitchen source builds
  * `restart`
  * `rollback`
  * `done` / `error`

---

## Supported dependency sync

### Python repos (`core`, `custom_node`)

Supported manifests:

* `requirements.txt`
* `pyproject.toml` with a standalone dependency list that can be executed directly

### Frontend repos

Supported manifests / conventions:

* `package.json`
* `packageManager` field, lockfile hints, or explicit package manager selection
* build script under `scripts.build`

The app does **not** execute arbitrary install scripts beyond the supported manifest-driven flows.

### Comfy Kitchen source overrides

Comfy Kitchen is handled as a project materialization rather than generic Python dependency sync. When source management is enabled, Patcher initializes the checkout's required submodules, asks the checkout's normal PEP 517/setuptools build to produce a wheel, installs that wheel into the configured installation Python, and records source/runtime provenance separately. Upstream Comfy Kitchen remains responsible for CUDA/HIP compiler discovery and architecture policy. The wheel is built before `pip install`; if a failed first source-management attempt never changes the existing unmanaged runtime, automatic recovery verifies that identity and leaves it untouched instead of reinstalling a package unnecessarily.

A Patcher-managed Kitchen source override is reasserted after Patcher-controlled core/custom-node dependency installs if those installs replace the active `comfy-kitchen` distribution. Installation-wide **Update all** and tracked-repository rematerialization defer that override work until ordinary Python dependency mutations finish, then finalize Kitchen once at the end instead of rebuilding it after every repository.

**Untrack** stops source management/reassertion but deliberately leaves the currently installed Kitchen package untouched. **Restore ComfyUI Kitchen**, **Disable**, and **Uninstall** return runtime ownership to the `comfy-kitchen` requirement declared by the current managed ComfyUI checkout before deactivating/removing the source override.

---

## Setup

### Windows: use the prebuilt release unless you want to develop the app

If you are on Windows and just want to use ComfyUI Patcher, the normal path is to download the prebuilt executable / installer from the project's GitHub releases and run that.

That path does **not** require a local Rust toolchain, Node.js, or a Tauri build setup.

Build from source only if you want to modify the app, work on the codebase, or produce your own local builds.

### Build from source

#### Prerequisites

* Node.js
* Rust toolchain
* system `git`
* Tauri prerequisites for your platform
* a local ComfyUI installation
* for managed frontend builds:

  * a working Node toolchain in the environment where the frontend repo lives
  * for WSL-managed frontend repos, Linux `node` and Linux `npm` / `pnpm` / `yarn` must be available inside WSL

#### Install dependencies

```bash
npm install
```

#### Run in development

```bash
npm run tauri dev
```

#### Build

```bash
npm install
npm run build
npm run tauri build
```

If you are recovering from a previously broken Rust dependency graph, it can still be useful to clear old lock / target state once before rebuilding:

```bash
rm -f src-tauri/Cargo.lock
rm -rf src-tauri/target
npm install
npm run build
npm run tauri build
```

---

## Usage

### 1. Register a ComfyUI installation

Fill in:

* display name
* local ComfyUI root directory
* optional explicit Python executable
* launch command and args for process control
* optional managed frontend settings

Example simple launch profile:

* command: `python`
* args: `main.py --listen 0.0.0.0 --port 8188`

Example WSL-backed launch profile:

* command: `wsl.exe`
* args: `-d Ubuntu-22.04 -- /home/toor/start_comfyui.sh`

If your launch command calls a shell script, that script should:

* activate the environment
* `exec` the final ComfyUI process
* forward `"$@"`

Example:

```bash
#!/usr/bin/env bash
set -e

source ~/miniconda3/etc/profile.d/conda.sh
conda activate comfy312

cd ~/ComfyUI
exec python main.py --listen 0.0.0.0 --port 8188 "$@"
```

### 2. Patch core ComfyUI

Paste one of:

* `master`
* `some-feature-branch`
* commit SHA
* `https://github.com/Comfy-Org/ComfyUI/tree/feature/branch`
* `https://github.com/Comfy-Org/ComfyUI/pull/12936`

Click **Resolve**, inspect the preview, then **Apply**.

If the repo already has overlays, prefer changing the tracked base on the repo card instead of using the one-shot apply box.

### 3. Install or patch the managed frontend

Paste one of:

* `https://github.com/Comfy-Org/ComfyUI_frontend`
* `https://github.com/Comfy-Org/ComfyUI_frontend/tree/main`
* `https://github.com/Comfy-Org/ComfyUI_frontend/pull/10367`

Behavior:

* on first install, the app can auto-derive a default managed frontend checkout path
* it clones / reuses the frontend repo
* materializes the tracked stack
* installs frontend dependencies
* builds the frontend
* then **Start / Restart** inject the managed `--front-end-root` automatically at runtime

The managed frontend is intended for a **single canonical remote per checkout**. Stacking overlays works within that managed frontend repo model, but switching between unrelated remotes at the same fixed repo root is treated as a repo replacement problem rather than as a same-stack overlay.

### 4. Manage Comfy Kitchen source

The **Kitchen** panel probes the `comfy-kitchen` distribution through the installation's configured Python even when no source checkout is managed. Source management is opt-in.

When enabled, Patcher accepts targets from the official `https://github.com/Comfy-Org/comfy-kitchen` repository, uses a fixed sibling checkout named `comfy-kitchen` beside the ComfyUI root, materializes the selected base/PR stack into a wheel, and installs that wheel into the managed Python environment. Checkout state and installed-runtime provenance are tracked independently so stale, missing, replaced, or import-failing runtimes can be detected.

**Restore ComfyUI Kitchen** reinstalls the exact `comfy-kitchen` requirement declared by the current ComfyUI checkout, clears the tracked Kitchen source target, and leaves the source checkout on disk. A later **Update all** therefore does not silently reactivate the source override. Rollback/checkpoint restore can return to the prior source-managed runtime.

**Untrack** leaves the currently installed Kitchen runtime unchanged and only stops Patcher source reassertion. **Disable** and **Uninstall** restore the ComfyUI-declared runtime requirement before moving/removing the source checkout. Start/Restart also refuse to launch when an active managed source override is no longer coherent with its recorded source revision/runtime provenance.

### 5. Install or patch a custom node manually

Paste one of:

* `https://github.com/owner/repo`
* `https://github.com/owner/repo/tree/branch-name`
* `https://github.com/owner/repo/pull/123`

The app clones into `custom_nodes/` if the repo is new, or patches the existing repo if it already exists at the target path.

### 6. Use the ComfyUI-Manager registry browser

* search registry entries
* inspect installable items
* install through the app
* manage installed repos through the same tracked repo UI afterward

### 7. Update and rollback

* **Update** on a repo card re-applies its tracked state
* **Update all** runs update for every tracked repo in the installation
* **Rollback** restores the most recent checkpoint for that repo

### 8. Start / Stop / Restart

Use the saved launch profile to control the managed ComfyUI process.

When a managed frontend is configured, **Start / Restart** inject the frontend dist path automatically. You should not need to hardcode `--front-end-root` in your saved launch args or shell script.

---

## Assumptions

1. **Core ComfyUI must be git-backed for patch / update / rollback.**
   A non-git core install can still be registered, but git-based core mutation is unavailable until the install is git-backed.

2. **Raw branch / tag / SHA inputs normally require an existing managed repo.**
   A brand-new install flow generally needs a repository URL or PR URL so the app knows what to clone. Comfy Kitchen is the explicit exception: before its checkout exists, bare refs resolve against the fixed official upstream.

3. **Tracked updates preserve the user’s chosen target model.**
   Direct targets and stacked overlays are both valid tracked states.

4. **Dependency sync is manifest-driven.**
   The app does not try to discover arbitrary project-specific install scripts beyond the supported Python / frontend flows.

5. **Managed frontend runtime injection assumes the saved launch profile forwards extra args correctly.**
   If your launcher script drops `"$@"`, the injected `--front-end-root` will never reach ComfyUI.

---

## Limitations and known constraints

### Implemented, but narrower than the broad product vision

* one-window desktop app with a single primary shell
* persisted operations and logs, but not a background job daemon
* local git + GitHub-only workflow rather than multi-forge support

### Not implemented in this version

* GitHub Enterprise / GitLab / Bitbucket support
* ZIP / manual custom node installs
* per-repo arbitrary dependency commands
* automatic conflict resolution for content-level merge conflicts
* filesystem watchers for out-of-band repo changes
* secure OS keychain storage for GitHub tokens
* advanced authenticated private-repo UX beyond `GITHUB_TOKEN`
* cross-remote overlay stacking in a single managed checkout
* permanent rewrite of stored launch args when managed frontend runtime injection strips / replaces `--front-end-root`

### Operational caveats

* stacked PR overlays rely on Git merge commits; the environment used for git execution must allow synthetic commits
* a WSL-managed frontend repo must be built with a Linux Node toolchain inside WSL, not with Windows `pnpm` / `npm` shims
* if a managed frontend repo and ComfyUI install live on different filesystems, replacement / backup handling is designed to avoid cross-device rename failures by backing up beside the target path
* Comfy Kitchen source builds require the native compiler/toolchain expected by the selected upstream Kitchen backend; Patcher does not replace upstream CUDA/HIP architecture selection
* managed Kitchen source targets are restricted to the official Comfy-Org/comfy-kitchen remote; unrelated remotes are rejected rather than being treated as interchangeable runtime providers

---

## Validation checklist

### Installation registration

* register a git-backed ComfyUI root
* confirm the core repo is discovered
* confirm git-backed repos under `custom_nodes/` are discovered
* confirm a configured frontend repo is discovered when present
* re-register the same root and confirm the existing entry is updated instead of duplicated

### Core patch

* patch to a branch URL
* patch to a commit URL
* patch to a PR URL
* verify `currentBranch`, `currentHeadSha`, and tracked state update correctly

### Comfy Kitchen

* register/reconcile an installation with and without an installed `comfy-kitchen` distribution and verify the runtime probe remains independent of source-checkout discovery
* install an official Kitchen branch/commit/PR target and verify required submodules initialize before the source wheel is built and installed
* verify the recorded materialized source HEAD and installed-runtime provenance match the active source build
* run a Patcher-controlled core/custom-node dependency sync that replaces `comfy-kitchen` and verify the active Kitchen source override is reasserted
* use **Restore ComfyUI Kitchen** and verify the current ComfyUI requirement is installed, source tracking is cleared, the checkout remains on disk, and **Update all** does not reactivate it
* use **Untrack** and verify the currently installed Kitchen runtime is left unchanged while future Patcher source reassertion stops
* rollback/restore a Kitchen checkpoint and verify both checkout/tracked state and runtime ownership are restored coherently
* replace or remove the installed source runtime out of band and verify Start/Restart refuses an incoherent active override
* force a fresh Kitchen source install/materialization failure and verify only operation-owned checkout/DB state is cleaned up while any retained pre-existing path is restored

### Frontend patch

* install the frontend from a repo URL or PR URL
* confirm dependency install + build run successfully
* confirm the managed frontend repo appears in the UI
* confirm **Start / Restart** inject `--front-end-root` from the managed dist path
* on WSL, confirm the injected runtime path uses Linux path form inside WSL

### Custom node install

* install from a repo URL
* install from a branch URL
* install from a PR URL

### Dirty repo handling

* modify a file in a managed repo
* confirm `abort` blocks mutation
* confirm `stash` allows mutation and leaves a checkpoint
* confirm `hard_reset` discards changes

### Overlay stack behavior

* set a base target
* add one or more PR overlays
* toggle overlay enabled / disabled state
* reorder overlays
* update the repo and confirm the stack is re-materialized correctly

### Rollback

* apply a patch
* rollback
* verify the previous HEAD / branch state is restored

### Process control

* save a valid launch profile
* **Start** the installation
* **Stop** the installation
* **Restart** the installation
* confirm managed frontend injection still works through **Start / Restart**

---

## In-app updater

The app now supports a native Tauri updater flow backed by GitHub Releases. The intended stable endpoint is:

```text
https://github.com/xmarre/ComfyUI-Patcher/releases/latest/download/latest.json
```

### Required release setup

1. Generate a Tauri updater signing key pair:

```bash
npm run tauri signer generate -- -w ~/.tauri/comfyui-patcher.key
```

2. Add these GitHub Actions repository secrets:

* `TAURI_SIGNING_PRIVATE_KEY`
* `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`
* `COMFYUI_PATCHER_UPDATER_PUBKEY`

`COMFYUI_PATCHER_UPDATER_PUBKEY` must contain the public key text that should be embedded into release builds. If it is missing at build time, the app still builds, but the in-app updater is disabled and the UI explains why.

### Release flow

* tag a release as `vX.Y.Z`
* GitHub Actions builds the NSIS bundle
* Tauri signs the updater artifacts
* the workflow publishes the release assets and `latest.json`

### Runtime behavior

* the app checks for a newer stable release on startup
* users can trigger a manual check from the sidebar
* install first shuts down managed ComfyUI child processes
* Windows installer handoff is delegated to the native Tauri updater
