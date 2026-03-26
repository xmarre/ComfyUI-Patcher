# ComfyUI Patcher

ComfyUI Patcher is a desktop app for managing a local ComfyUI installation and git-backed custom nodes. It resolves GitHub repository URLs, branch URLs, commit URLs, and PR URLs, then applies the target safely to the core ComfyUI repo or to a repository inside `custom_nodes/`. It tracks managed repositories, records checkpoints before mutation, syncs dependencies after revision changes, and can restart the installation through a saved launch profile.

## Architecture plan

### Product shape

- **Desktop shell:** Tauri 2
- **Backend:** Rust
- **Frontend:** React + TypeScript + Vite
- **Persistence:** SQLite via `rusqlite`
- **Git execution:** system `git` CLI
- **Target resolution:** GitHub REST API for PR metadata, git for actual fetch/checkout
- **Process control:** local child-process management through a saved launch profile

### Backend modules

- `db.rs` — SQLite schema and CRUD for installations, repos, operations, checkpoints, and logs.
- `github.rs` — parses GitHub URLs and resolves repo/branch/commit/PR targets.
- `git.rs` — thin system-git wrapper for inspection, fetch, checkout, reset, stash, clone, and submodule update.
- `deps.rs` — dependency detection and execution.
- `process.rs` — starts/stops/restarts a managed child process from a launch profile.
- `state.rs` — application state, GitHub client, database, process registry, and per-repo/per-installation locks.
- `lib.rs` — Tauri command boundary and operation orchestration.

### Frontend modules

- `src/App.tsx` — primary shell with installation registration, core patch panel, custom-node panel, repo cards, event stream, and operation panel.
- `src/components/RepoCard.tsx` — repo summary with update and rollback controls.
- `src/components/OperationPanel.tsx` — operation list and persisted log viewer.
- `src/api.ts` / `src/types.ts` — typed Tauri command wrappers and DTOs.

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
│     ├─ git.rs
│     ├─ github.rs
│     ├─ lib.rs
│     ├─ main.rs
│     ├─ models.rs
│     ├─ process.rs
│     ├─ state.rs
│     └─ util.rs
└─ tests/
   └─ README.md
```

## Implemented features

### Installation management

- register a local ComfyUI root
- detect `custom_nodes/`
- detect a likely Python executable if one is not provided
- register a launch profile for restart support
- discover the core repo and git-backed custom nodes already present under `custom_nodes/`

### Target resolution

Accepted inputs:

- raw branch name for an existing managed repo
- raw tag name for an existing managed repo
- raw commit SHA for an existing managed repo
- GitHub repo URL
- GitHub branch URL (`/tree/...`)
- GitHub commit URL (`/commit/...`)
- GitHub PR URL (`/pull/<id>`)

Resolution rules:

- PR URLs are resolved through the GitHub API
- repo URLs resolve to the repository default branch
- raw names are resolved against `origin` for existing local repos
- branch names containing slashes are supported

### Core patching

- resolve target
- checkpoint the repo state
- handle dirty repo according to `abort`, `stash`, or `hard_reset`
- fetch + checkout + reset target revision
- update submodules
- run dependency sync
- persist the target as the tracked update source

### Custom node install / patch

- install new repo into `custom_nodes/<name>`
- patch an existing git-backed node if the target path already points to a repo
- conflict handling for non-git paths:
  - abort
  - replace
  - install with suffixed directory name

### Update

- update a managed repo to its tracked target
- update all tracked repos of an installation
- reuse the same patch logic for branch, tag, commit, and PR tracking

### Rollback

- restore the last checkpointed repo state
- restore branch vs detached-HEAD state
- optionally re-apply stashed changes
- rerun dependency sync

### Restart

- restart the managed ComfyUI child process from a saved launch profile

### Logging and operations

Each mutation creates an operation record and a log file. The UI shows:

- recent operations
- live backend event messages
- persisted operation logs

## Setup

### Prerequisites

- Node.js
- Rust toolchain
- system `git`
- Tauri prerequisites for your platform
- a local ComfyUI installation that is git-backed for full core patch support

### Install dependencies

```bash
npm install
```

### Run in development

```bash
npm run tauri dev
```

### Build

```bash
npm run build
npm run tauri build
```

## Usage

### 1. Register a ComfyUI installation

Fill in:

- a display name
- the local ComfyUI root directory
- optional explicit Python executable
- launch command and args for restart control

Example launch profile:

- command: `python`
- args: `main.py --listen 0.0.0.0 --port 8188`

### 2. Patch core ComfyUI

Paste one of:

- `master`
- `some-feature-branch`
- commit SHA
- `https://github.com/Comfy-Org/ComfyUI/tree/feature/branch`
- `https://github.com/Comfy-Org/ComfyUI/pull/12936`

Click **Resolve**, inspect the preview, then **Apply**.

### 3. Install or patch a custom node

Paste one of:

- `https://github.com/owner/repo`
- `https://github.com/owner/repo/tree/branch-name`
- `https://github.com/owner/repo/pull/123`

The app clones into `custom_nodes/` if the repo is new, or patches the existing repo if it already exists at the chosen path.

### 4. Update and rollback

- **Update** on a repo card re-applies its tracked target
- **Update all** runs update for every tracked repo in the installation
- **Rollback** restores the most recent checkpoint for that repo

### 5. Restart

Use **Restart** to stop and start the managed ComfyUI process via the saved launch profile.

## Assumptions

1. **Core ComfyUI must be a git repo for patch/update/rollback.** A non-git core install can be registered, but git-based core mutation is not available until the install is git-backed.
2. **Raw branch/tag/SHA inputs only make sense for existing managed repos.** A brand-new custom node install needs a repository URL or PR URL so the app knows what to clone.
3. **Tracked updates reuse the original input string.** This keeps update behavior faithful to what the user chose instead of inventing a derived policy.
4. **Dependency sync does not auto-discover arbitrary install scripts.** Only the supported manifests are executed automatically.
5. **Restart is reliable only when the app started the child or when the launch profile is valid.** It does not try to discover unrelated external ComfyUI processes and kill them.

## Limitations and deviations

### Implemented, but narrower than the broad product vision

- One-window desktop app with a single primary shell instead of a more elaborate multi-page UI.
- Persisted operation records and log files, but not a background job daemon.
- Managed-child restart mode is implemented; richer custom command lifecycle handling is only modeled, not fully executed.

### Not implemented in this version

- GitHub Enterprise / GitLab / Bitbucket support
- ZIP/manual custom node installs
- per-repo custom dependency commands
- auto-rollback on mid-operation failures
- filesystem watchers for out-of-band repo changes
- secure OS keychain storage for GitHub tokens
- authenticated private-repo UX beyond reading `GITHUB_TOKEN` from the environment
- advanced merge/cherry-pick patch semantics; this app checks out a target revision cleanly instead of synthesizing patch files

## Validation checklist

### Installation registration

- register a git-backed ComfyUI root
- confirm the core repo is discovered
- confirm git-backed repos under `custom_nodes/` are discovered

### Core patch

- patch to a branch URL
- patch to a commit URL
- patch to a PR URL
- verify `currentBranch`, `currentHeadSha`, and `trackedTargetInput` update

### Custom node install

- install from a repo URL
- install from a branch URL
- install from a PR URL

### Dirty repo handling

- modify a file in a managed repo
- confirm `abort` blocks mutation
- confirm `stash` allows mutation and leaves a checkpoint
- confirm `hard_reset` discards changes

### Rollback

- apply a patch
- rollback
- verify old HEAD is restored

### Restart

- save a valid launch profile
- restart the installation
- confirm the managed process stops and starts again
