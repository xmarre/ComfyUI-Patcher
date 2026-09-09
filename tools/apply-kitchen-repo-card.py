from pathlib import Path

path = Path("src/components/RepoCard.tsx")
text = path.read_text()


def replace_once(old: str, new: str, label: str) -> None:
    global text
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{label}: expected exactly one match, found {count}")
    text = text.replace(old, new, 1)


replace_once(
    '''function formatTimestamp(value: string | null): string {\n''',
    '''function materializationStatusClass(status: string): string {\n  switch (status) {\n    case "current":\n      return "ok";\n    case "stale":\n      return "warn";\n    case "missing":\n    case "import_failed":\n    case "replaced":\n    case "failed":\n      return "danger";\n    default:\n      return "";\n  }\n}\n\nfunction formatTimestamp(value: string | null): string {\n''',
    "materialization status style helper",
)

materialization_panel = '''\n      {repo.kind === "kitchen" ? (\n        <div className="preview">\n          <div className="row between repo-preview-header">\n            <div>\n              <strong>Runtime materialization</strong>\n              <div className="muted small">Source checkout state and installed Python artifact are tracked separately.</div>\n            </div>\n            {repo.materializationState ? (\n              <span className={`badge ${materializationStatusClass(repo.materializationState.status)}`}>\n                {repo.materializationState.status.replace("_", " ")}\n              </span>\n            ) : (\n              <span className="badge">ComfyUI-managed runtime</span>\n            )}\n          </div>\n          {repo.materializationState ? (\n            <>\n              <div className="grid two compact-grid">\n                <div>\n                  <div className="label">Materialized source HEAD</div>\n                  <div className="mono small">{repo.materializationState.materializedHeadSha ?? "unknown"}</div>\n                </div>\n                <div>\n                  <div className="label">Installed version</div>\n                  <div className="mono small">{repo.materializationState.installedVersion ?? "unknown"}</div>\n                </div>\n                <div>\n                  <div className="label">Built artifact SHA-256</div>\n                  <div className="mono small">{repo.materializationState.artifactSha256 ?? "unknown"}</div>\n                </div>\n                <div>\n                  <div className="label">Installed RECORD SHA-256</div>\n                  <div className="mono small">{repo.materializationState.installedRecordSha256 ?? "unknown"}</div>\n                </div>\n              </div>\n              <div className="muted small">\n                Last materialized {formatTimestamp(repo.materializationState.lastMaterializedAt)}\n              </div>\n              {repo.materializationState.lastError ? (\n                <div className="muted small">{repo.materializationState.lastError}</div>\n              ) : null}\n            </>\n          ) : (\n            <div className="muted small">\n              This checkout is not overriding the Python environment. The current ComfyUI requirement owns the installed comfy-kitchen distribution.\n            </div>\n          )}\n        </div>\n      ) : null}\n'''
replace_once(
    '''\n      <div className="repo-stack-panel">\n''',
    materialization_panel + '''\n      <div className="repo-stack-panel">\n''',
    "Kitchen materialization panel",
)

replace_once(
    '''                  {checkpoint.dependencyState ? (\n                    <div className="muted small">\n                      Dependency snapshot:{" "}\n                      {checkpoint.dependencyState.plan\n                        ? `${checkpoint.dependencyState.plan.strategy} (${checkpoint.dependencyState.plan.reason})`\n                        : checkpoint.dependencyState.error ?? "No dependency metadata"}\n                    </div>\n                  ) : null}\n''',
    '''                  {checkpoint.dependencyState ? (\n                    <div className="muted small">\n                      Dependency snapshot:{" "}\n                      {checkpoint.dependencyState.plan\n                        ? `${checkpoint.dependencyState.plan.strategy} (${checkpoint.dependencyState.plan.reason})`\n                        : checkpoint.dependencyState.error ?? "No dependency metadata"}\n                    </div>\n                  ) : null}\n                  {checkpoint.materializationState ? (\n                    <div className="muted small">\n                      Runtime snapshot: {checkpoint.materializationState.status.replace("_", " ")} at{" "}\n                      <span className="mono">\n                        {checkpoint.materializationState.materializedHeadSha ?? "unknown source HEAD"}\n                      </span>\n                    </div>\n                  ) : repo.kind === "kitchen" ? (\n                    <div className="muted small">Runtime snapshot: ComfyUI-managed comfy-kitchen requirement</div>\n                  ) : null}\n''',
    "checkpoint Kitchen runtime snapshot",
)

replace_once(
    '''        {onRollback ? (\n          <button\n            className="secondary"\n            disabled={isSubmitting}\n            onClick={() => void runLocalAction(onRollback)}\n          >\n            Rollback latest\n          </button>\n        ) : null}\n        <button className="secondary" disabled={isSubmitting} onClick={() => void toggleHistory()}>\n''',
    '''        {onRollback ? (\n          <button\n            className="secondary"\n            disabled={isSubmitting}\n            onClick={() => void runLocalAction(onRollback)}\n          >\n            Rollback latest\n          </button>\n        ) : null}\n        {repo.kind === "kitchen" && repo.materializationState ? (\n          <button\n            className="secondary"\n            disabled={isSubmitting}\n            onClick={() =>\n              void runLocalAction(async () => {\n                if (\n                  !window.confirm(\n                    "Restore the comfy-kitchen requirement declared by the current ComfyUI checkout? The source checkout stays on disk, but its built runtime override is deactivated."\n                  )\n                ) {\n                  return;\n                }\n                await api.restoreComfyManagedKitchen({\n                  repoId: repo.id,\n                  restartAfterSuccess: false\n                });\n              })\n            }\n          >\n            Restore ComfyUI Kitchen\n          </button>\n        ) : null}\n        <button className="secondary" disabled={isSubmitting} onClick={() => void toggleHistory()}>\n''',
    "Kitchen restore action",
)

replace_once(
    '''          <div className="muted small">\n            Lifecycle actions do not create new checkpoints. They remove or hide the repo directly, and untrack also suppresses future reconcile rediscovery for this path.\n          </div>\n''',
    '''          <div className="muted small">\n            {repo.kind === "kitchen" && repo.materializationState\n              ? "Before Kitchen is uninstalled, disabled, or untracked, Patcher first restores the comfy-kitchen requirement declared by the current ComfyUI checkout. Lifecycle actions do not create new checkpoints."\n              : "Lifecycle actions do not create new checkpoints. They remove or hide the repo directly, and untrack also suppresses future reconcile rediscovery for this path."}\n          </div>\n''',
    "Kitchen lifecycle copy",
)

path.write_text(text)
