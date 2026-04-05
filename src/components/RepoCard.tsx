import { useState } from "react";
import { api } from "../api";
import type {
  ManagedRepo,
  OverlayApplyStatus,
  OverlayMoveDirection,
  RepoActionPreview,
  RepoCheckpoint,
  RepoCheckpointComparison,
  TrackedPrOverlay
} from "../types";

type Props = {
  repo: ManagedRepo;
  onUpdate?: () => Promise<void>;
  onRollback?: () => Promise<void>;
  onSetBaseTarget?: (input: string, clearOverlays: boolean) => Promise<boolean>;
  onAddOverlay?: (input: string) => Promise<boolean>;
  onSetOverlayEnabled?: (overlayId: string, enabled: boolean) => Promise<boolean>;
  onRemoveOverlay?: (overlayId: string) => Promise<boolean>;
  onMoveOverlay?: (overlayId: string, direction: OverlayMoveDirection) => Promise<boolean>;
};

type EffectiveOverlayStatus = OverlayApplyStatus | "disabled" | "pending";

function overlayEffectiveStatus(overlay: TrackedPrOverlay): EffectiveOverlayStatus {
  if (!overlay.enabled) {
    return "disabled";
  }
  return overlay.lastApplyStatus ?? "pending";
}

function overlayStatusClass(overlay: TrackedPrOverlay): string {
  switch (overlayEffectiveStatus(overlay)) {
    case "applied":
      return "ok";
    case "conflict":
    case "error":
      return "danger";
    case "disabled":
      return "";
    default:
      return "warn";
  }
}

function repoStatusClass(repo: ManagedRepo): string {
  switch (repo.liveStatus) {
    case "clean":
      return "ok";
    case "dirty":
      return "warn";
    case "drifted":
    case "missing":
    case "not_git":
      return "danger";
    default:
      return "";
  }
}

function formatTimestamp(value: string | null): string {
  if (!value) {
    return "unknown";
  }
  return new Date(value).toLocaleString();
}

function toErrorMessage(error: unknown): string {
  if (error instanceof Error) {
    return error.message;
  }
  return String(error);
}

function renderPreview(preview: RepoActionPreview | null, emptyLabel: string) {
  if (!preview) {
    return <div className="muted small">{emptyLabel}</div>;
  }
  return (
    <div className="preview repo-preview-card">
      <div className="row between repo-preview-header">
        <div>
          <div><strong>{preview.targetSummary}</strong></div>
          <div className="muted small">{preview.action}</div>
        </div>
        {preview.targetHeadSha ? <div className="mono small">{preview.targetHeadSha}</div> : null}
      </div>

      <div className="grid two compact-grid">
        <div>
          <div className="label">Current HEAD</div>
          <div className="mono small">{preview.currentHeadSha ?? "none"}</div>
        </div>
        <div>
          <div className="label">Target ref</div>
          <div className="mono small">{preview.targetRef ?? "n/a"}</div>
        </div>
      </div>

      {preview.stackPreview.length ? (
        <div className="stack">
          <div className="label">Stack preview</div>
          <div className="repo-stack-preview-list">
            {preview.stackPreview.map((item, index) => (
              <div key={`${item.kind}-${item.label}-${index}`} className="repo-stack-preview-item">
                <span className="badge">{item.kind}</span>
                <span>{item.label}</span>
                {!item.enabled ? <span className="muted small">disabled</span> : null}
              </div>
            ))}
          </div>
        </div>
      ) : null}

      {preview.warnings.length ? (
        <div className="stack">
          <div className="label">Warnings</div>
          <div className="repo-warning-list">
            {preview.warnings.map((warning) => (
              <div key={warning} className="muted small">{warning}</div>
            ))}
          </div>
        </div>
      ) : null}

      {preview.conflictFiles.length ? (
        <div className="stack">
          <div className="label">Conflict surface</div>
          <div className="repo-warning-list">
            {preview.conflictFiles.map((path) => (
              <div key={path} className="mono small">{path}</div>
            ))}
          </div>
        </div>
      ) : null}

      {preview.commits.length ? (
        <div className="stack">
          <div className="label">Commits introduced</div>
          <div className="repo-change-list">
            {preview.commits.slice(0, 10).map((commit) => (
              <div key={commit.sha} className="repo-change-item">
                <div className="mono small">{commit.sha.slice(0, 12)}</div>
                <div className="small">{commit.subject}</div>
              </div>
            ))}
          </div>
        </div>
      ) : null}

      {preview.fileChanges.length ? (
        <div className="stack">
          <div className="label">Files changed</div>
          <div className="repo-change-list">
            {preview.fileChanges.slice(0, 20).map((file) => (
              <div key={`${file.status}-${file.path}`} className="repo-change-item">
                <span className="badge">{file.status}</span>
                <span className="mono small">{file.path}</span>
              </div>
            ))}
          </div>
        </div>
      ) : null}

      {preview.dependencyState ? (
        <div className="stack">
          <div className="label">Dependency state</div>
          <div className="small">
            {preview.dependencyState.plan
              ? `${preview.dependencyState.plan.strategy}: ${preview.dependencyState.plan.reason}`
              : preview.dependencyState.error ?? "No dependency plan"}
          </div>
          {preview.dependencyState.relevantChangedFiles.length ? (
            <div className="muted small">
              Changed manifests: {preview.dependencyState.relevantChangedFiles.join(", ")}
            </div>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}

export default function RepoCard({
  repo,
  onUpdate,
  onRollback,
  onSetBaseTarget,
  onAddOverlay,
  onSetOverlayEnabled,
  onRemoveOverlay,
  onMoveOverlay
}: Props) {
  const trackedState = repo.trackedState;
  const overlays = trackedState?.overlays ?? [];
  const [baseInput, setBaseInput] = useState("");
  const [overlayInput, setOverlayInput] = useState("");
  const [isSubmitting, setIsSubmitting] = useState(false);
  const [panelError, setPanelError] = useState<string | null>(null);
  const [historyOpen, setHistoryOpen] = useState(false);
  const [checkpoints, setCheckpoints] = useState<RepoCheckpoint[]>([]);
  const [checkpointComparison, setCheckpointComparison] = useState<RepoCheckpointComparison | null>(null);
  const [updatePreview, setUpdatePreview] = useState<RepoActionPreview | null>(null);
  const [basePreview, setBasePreview] = useState<RepoActionPreview | null>(null);
  const [overlayPreview, setOverlayPreview] = useState<RepoActionPreview | null>(null);
  const integrationBranch = trackedState?.materializedBranch ?? repo.currentBranch ?? "detached";
  const hasOverlays = overlays.length > 0;
  const lifecycleSupported = repo.kind !== "core";

  async function runStackAction(action: () => Promise<boolean>): Promise<boolean> {
    if (isSubmitting) {
      return false;
    }
    setPanelError(null);
    setIsSubmitting(true);
    try {
      return await action();
    } catch (error) {
      setPanelError(toErrorMessage(error));
      return false;
    } finally {
      setIsSubmitting(false);
    }
  }

  async function runLocalAction(action: () => Promise<void>) {
    if (isSubmitting) {
      return;
    }
    setPanelError(null);
    setIsSubmitting(true);
    try {
      await action();
    } catch (error) {
      setPanelError(toErrorMessage(error));
    } finally {
      setIsSubmitting(false);
    }
  }

  async function toggleHistory() {
    if (historyOpen) {
      setHistoryOpen(false);
      return;
    }
    await runLocalAction(async () => {
      const next = await api.listCheckpoints(repo.id);
      setCheckpoints(next);
      setHistoryOpen(true);
    });
  }

  async function previewTarget(input: string, clearOverlays: boolean, target: "base" | "overlay") {
    await runLocalAction(async () => {
      const preview = await api.previewRepoTarget({
        installationId: repo.installationId,
        kind: repo.kind,
        input,
        repoId: repo.id,
        clearOverlays
      });
      if (target === "base") {
        setBasePreview(preview);
      } else {
        setOverlayPreview(preview);
      }
    });
  }

  return (
    <div className="card">
      <div className="row between repo-card-header">
        <div>
          <h3>{repo.displayName}</h3>
          <div className="muted mono">{repo.localPath}</div>
          {repo.lastScannedAt ? (
            <div className="muted small">Last reconciled {formatTimestamp(repo.lastScannedAt)}</div>
          ) : null}
        </div>
        <div className="badge-group repo-badge-wrap">
          <span className={`badge ${repoStatusClass(repo)}`}>{repo.liveStatus.replace("_", " ")}</span>
          <span className={`badge ${repo.isDirty ? "warn" : "ok"}`}>{repo.isDirty ? "dirty" : "clean"}</span>
          <span className="badge">{repo.kind}</span>
        </div>
      </div>

      <div className="grid two">
        <div>
          <div className="label">Current branch</div>
          <div className="mono">{repo.currentBranch ?? "detached"}</div>
        </div>
        <div>
          <div className="label">Integration branch</div>
          <div className="mono">{integrationBranch}</div>
        </div>
        <div>
          <div className="label">HEAD</div>
          <div className="mono">{repo.currentHeadSha ?? "unknown"}</div>
        </div>
        <div>
          <div className="label">Tracked HEAD</div>
          <div className="mono">{repo.trackedTargetResolvedSha ?? "none"}</div>
        </div>
        <div>
          <div className="label">Remote</div>
          <div className="mono small">{repo.canonicalRemote ?? "unknown"}</div>
        </div>
        <div>
          <div className="label">Changed files</div>
          <div className="small">{repo.changedFiles.length}</div>
        </div>
      </div>

      {repo.liveWarnings.length ? (
        <div className="repo-warning-list">
          {repo.liveWarnings.map((warning) => (
            <div key={warning} className="muted small">{warning}</div>
          ))}
        </div>
      ) : null}

      {repo.dependencyState ? (
        <div className="preview">
          <div className="row between repo-preview-header">
            <div>
              <strong>Dependency state</strong>
            </div>
            {repo.dependencyState.plan ? (
              <span className="badge">{repo.dependencyState.plan.strategy}</span>
            ) : null}
          </div>
          <div className="small">
            {repo.dependencyState.plan
              ? repo.dependencyState.plan.reason
              : repo.dependencyState.error ?? "No supported dependency manifest detected"}
          </div>
          {repo.dependencyState.relevantChangedFiles.length ? (
            <div className="muted small">
              Manifest drift: {repo.dependencyState.relevantChangedFiles.join(", ")}
            </div>
          ) : null}
        </div>
      ) : null}

      <div className="repo-stack-panel">
        <div className="row between repo-stack-header">
          <div>
            <div className="label">Base target</div>
            <div className="mono small">
              {trackedState ? trackedState.base.summaryLabel : repo.trackedTargetInput ?? "none"}
            </div>
          </div>
          <span className="badge">
            {hasOverlays ? `${overlays.filter((overlay) => overlay.enabled).length}/${overlays.length} overlays` : "base only"}
          </span>
        </div>

        {trackedState ? (
          <div className="muted small">
            Base ref: <span className="mono">{trackedState.base.checkoutRef}</span>
          </div>
        ) : null}

        <div className="row gap repo-stack-inputs">
          <input
            className="grow"
            disabled={isSubmitting}
            placeholder="Branch, tag, commit, or GitHub tree URL"
            value={baseInput}
            onChange={(event) => setBaseInput(event.target.value)}
          />
          <button
            className="secondary"
            disabled={!baseInput.trim() || isSubmitting}
            onClick={() => void previewTarget(baseInput, hasOverlays, "base")}
          >
            Preview
          </button>
          {onSetBaseTarget ? (
            <button
              className="secondary"
              disabled={!baseInput.trim() || isSubmitting}
              onClick={async () => {
                if (!onSetBaseTarget || !baseInput.trim() || isSubmitting) {
                  return;
                }
                const ok = await runStackAction(() => onSetBaseTarget(baseInput, hasOverlays));
                if (ok) {
                  setBaseInput("");
                  setBasePreview(null);
                }
              }}
            >
              {hasOverlays ? "Replace base" : "Set base"}
            </button>
          ) : null}
        </div>
        {renderPreview(basePreview, "Preview a base change to inspect the commit/file delta before applying it.")}

        <div className="row gap repo-stack-inputs">
          <input
            className="grow"
            disabled={isSubmitting}
            placeholder="GitHub PR URL"
            value={overlayInput}
            onChange={(event) => setOverlayInput(event.target.value)}
          />
          <button
            className="secondary"
            disabled={!overlayInput.trim() || isSubmitting}
            onClick={() => void previewTarget(overlayInput, false, "overlay")}
          >
            Preview
          </button>
          {onAddOverlay ? (
            <button
              disabled={!overlayInput.trim() || isSubmitting}
              onClick={async () => {
                if (!onAddOverlay || !overlayInput.trim() || isSubmitting) {
                  return;
                }
                const ok = await runStackAction(() => onAddOverlay(overlayInput));
                if (ok) {
                  setOverlayInput("");
                  setOverlayPreview(null);
                }
              }}
            >
              Add PR overlay
            </button>
          ) : null}
        </div>
        {renderPreview(overlayPreview, "Preview a PR overlay to inspect the stack order, introduced commits, and conflict surface.")}

        {hasOverlays ? (
          <div className="overlay-list">
            {overlays.map((overlay, index) => {
              const status = overlayEffectiveStatus(overlay);
              return (
                <div key={overlay.id} className="overlay-item">
                  <div className="row between overlay-row">
                    <label className="overlay-toggle">
                      <input
                        type="checkbox"
                        disabled={isSubmitting}
                        checked={overlay.enabled}
                        onChange={async (event) => {
                          if (!onSetOverlayEnabled || isSubmitting) {
                            return;
                          }
                          await runStackAction(() =>
                            onSetOverlayEnabled(overlay.id, event.target.checked)
                          );
                        }}
                      />
                      <span>
                        <strong>{overlay.summaryLabel}</strong>
                        <span className="mono small overlay-meta">
                          PR #{overlay.prNumber} on {overlay.prBaseRef}
                        </span>
                      </span>
                    </label>
                    <span className={`badge ${overlayStatusClass(overlay)}`}>{status}</span>
                  </div>
                  <div className="row between overlay-row">
                    <div className="mono small overlay-meta">{overlay.resolvedSha ?? overlay.sourceInput}</div>
                    <div className="row gap overlay-actions">
                      <button
                        className="secondary"
                        disabled={index === 0 || isSubmitting}
                        onClick={async () => {
                          if (!onMoveOverlay || isSubmitting) {
                            return;
                          }
                          await runStackAction(() => onMoveOverlay(overlay.id, "up"));
                        }}
                      >
                        Up
                      </button>
                      <button
                        className="secondary"
                        disabled={index === overlays.length - 1 || isSubmitting}
                        onClick={async () => {
                          if (!onMoveOverlay || isSubmitting) {
                            return;
                          }
                          await runStackAction(() => onMoveOverlay(overlay.id, "down"));
                        }}
                      >
                        Down
                      </button>
                      <button
                        className="secondary"
                        disabled={isSubmitting}
                        onClick={async () => {
                          if (!onRemoveOverlay || isSubmitting) {
                            return;
                          }
                          await runStackAction(() => onRemoveOverlay(overlay.id));
                        }}
                      >
                        Remove
                      </button>
                    </div>
                  </div>
                  {overlay.lastError ? <div className="muted small">{overlay.lastError}</div> : null}
                </div>
              );
            })}
          </div>
        ) : (
          <div className="muted small">No PR overlays are stored for this repo.</div>
        )}
      </div>

      <div className="row gap repo-action-wrap">
        <button
          className="secondary"
          disabled={isSubmitting}
          onClick={() =>
            void runLocalAction(async () => {
              const preview = await api.previewTrackedRepoUpdate(repo.id);
              setUpdatePreview(preview);
            })
          }
        >
          Preview update
        </button>
        {onUpdate ? (
          <button disabled={isSubmitting} onClick={() => void runLocalAction(onUpdate)}>
            Update
          </button>
        ) : null}
        {onRollback ? (
          <button
            className="secondary"
            disabled={isSubmitting}
            onClick={() => void runLocalAction(onRollback)}
          >
            Rollback latest
          </button>
        ) : null}
        <button className="secondary" disabled={isSubmitting} onClick={() => void toggleHistory()}>
          {historyOpen ? "Hide history" : "History"}
        </button>
      </div>
      {renderPreview(updatePreview, "Preview the tracked update plan to inspect incoming commits and files before mutating the checkout.")}

      {historyOpen ? (
        <div className="repo-history-panel">
          <div className="row between repo-history-header">
            <div>
              <strong>Checkpoint history</strong>
              <div className="muted small">{checkpoints.length} checkpoint{checkpoints.length === 1 ? "" : "s"}</div>
            </div>
            <button
              className="secondary"
              disabled={isSubmitting}
              onClick={() =>
                void runLocalAction(async () => {
                  const next = await api.listCheckpoints(repo.id);
                  setCheckpoints(next);
                })
              }
            >
              Refresh
            </button>
          </div>

          {checkpoints.length ? (
            <div className="stack">
              {checkpoints.map((checkpoint) => (
                <div key={checkpoint.id} className="overlay-item">
                  <div className="row between overlay-row">
                    <div>
                      <strong>{checkpoint.label ?? checkpoint.id}</strong>
                      <div className="muted small">{checkpoint.reason ?? "No reason recorded"}</div>
                    </div>
                    <span className="badge">{formatTimestamp(checkpoint.createdAt)}</span>
                  </div>
                  <div className="grid two compact-grid">
                    <div>
                      <div className="label">Restore HEAD</div>
                      <div className="mono small">{checkpoint.oldHeadSha}</div>
                    </div>
                    <div>
                      <div className="label">Branch</div>
                      <div className="mono small">{checkpoint.oldBranch ?? "detached"}</div>
                    </div>
                  </div>
                  {checkpoint.dependencyState ? (
                    <div className="muted small">
                      Dependency snapshot:{" "}
                      {checkpoint.dependencyState.plan
                        ? `${checkpoint.dependencyState.plan.strategy} (${checkpoint.dependencyState.plan.reason})`
                        : checkpoint.dependencyState.error ?? "No dependency metadata"}
                    </div>
                  ) : null}
                  <div className="row gap repo-action-wrap">
                    <button
                      className="secondary"
                      disabled={isSubmitting}
                      onClick={() =>
                        void runLocalAction(async () => {
                          const comparison = await api.compareCheckpoint(repo.id, checkpoint.id);
                          setCheckpointComparison(comparison);
                        })
                      }
                    >
                      Compare
                    </button>
                    <button
                      disabled={isSubmitting}
                      onClick={() =>
                        void runLocalAction(async () => {
                          await api.restoreCheckpoint({
                            repoId: repo.id,
                            checkpointId: checkpoint.id,
                            restoreStash: true,
                            syncDependencies: true,
                            restartAfterSuccess: false
                          });
                        })
                      }
                    >
                      Restore
                    </button>
                  </div>
                </div>
              ))}
            </div>
          ) : (
            <div className="muted small">No checkpoints have been recorded for this repo yet.</div>
          )}

          {checkpointComparison ? (
            <div className="preview repo-preview-card">
              <div className="row between repo-preview-header">
                <div>
                  <strong>{checkpointComparison.checkpoint.label ?? checkpointComparison.checkpoint.id}</strong>
                  <div className="muted small">Current state vs selected checkpoint</div>
                </div>
                <div className="mono small">{checkpointComparison.currentHeadSha ?? "missing"}</div>
              </div>

              {checkpointComparison.warnings.length ? (
                <div className="repo-warning-list">
                  {checkpointComparison.warnings.map((warning) => (
                    <div key={warning} className="muted small">{warning}</div>
                  ))}
                </div>
              ) : null}

              {checkpointComparison.commits.length ? (
                <div className="stack">
                  <div className="label">Commits since this checkpoint</div>
                  <div className="repo-change-list">
                    {checkpointComparison.commits.slice(0, 10).map((commit) => (
                      <div key={commit.sha} className="repo-change-item">
                        <div className="mono small">{commit.sha.slice(0, 12)}</div>
                        <div className="small">{commit.subject}</div>
                      </div>
                    ))}
                  </div>
                </div>
              ) : null}

              {checkpointComparison.fileChanges.length ? (
                <div className="stack">
                  <div className="label">Files drifted from checkpoint</div>
                  <div className="repo-change-list">
                    {checkpointComparison.fileChanges.slice(0, 20).map((file) => (
                      <div key={`${file.status}-${file.path}`} className="repo-change-item">
                        <span className="badge">{file.status}</span>
                        <span className="mono small">{file.path}</span>
                      </div>
                    ))}
                  </div>
                </div>
              ) : null}

              {checkpointComparison.currentDependencyState ? (
                <div className="muted small">
                  Current dependency state:{" "}
                  {checkpointComparison.currentDependencyState.plan
                    ? `${checkpointComparison.currentDependencyState.plan.strategy} (${checkpointComparison.currentDependencyState.plan.reason})`
                    : checkpointComparison.currentDependencyState.error ?? "No dependency metadata"}
                </div>
              ) : null}
            </div>
          ) : null}
        </div>
      ) : null}

      {lifecycleSupported ? (
        <div className="stack">
          <div className="muted small">
            Lifecycle actions do not create new checkpoints. They remove or hide the repo directly, and untrack also suppresses future reconcile rediscovery for this path.
          </div>
          <div className="row gap repo-action-wrap">
            <button
              className="secondary"
              disabled={isSubmitting}
              onClick={() =>
                void runLocalAction(async () => {
                  if (!window.confirm(`Uninstall ${repo.displayName}? This deletes ${repo.localPath}.`)) {
                    return;
                  }
                  await api.uninstallRepo({ repoId: repo.id });
                })
              }
            >
              Uninstall
            </button>
            <button
              className="secondary"
              disabled={isSubmitting}
              onClick={() =>
                void runLocalAction(async () => {
                  if (!window.confirm(`Disable ${repo.displayName}? This moves it out of the active install.`)) {
                    return;
                  }
                  await api.disableRepo({ repoId: repo.id });
                })
              }
            >
              Disable
            </button>
            <button
              className="secondary"
              disabled={isSubmitting}
              onClick={() =>
                void runLocalAction(async () => {
                  if (!window.confirm(`Untrack ${repo.displayName}? Files stay on disk, but future reconcile passes will ignore this path.`)) {
                    return;
                  }
                  await api.untrackRepo({ repoId: repo.id });
                })
              }
            >
              Untrack
            </button>
          </div>
        </div>
      ) : null}

      {panelError ? <div className="muted small">{panelError}</div> : null}
    </div>
  );
}
