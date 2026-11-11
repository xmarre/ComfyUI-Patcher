import { useEffect, useState } from "react";
import { api } from "../api";
import type {
  ManagedRepo,
  OverlayApplyStatus,
  RepoActionPreview,
  RepoCheckpoint,
  RepoCheckpointComparison,
  TrackedPrOverlay,
  OverlaySelectionRequest,
  SelectedRepoOverlay,
  OperationStart
} from "../types";

type Props = {
  repo: ManagedRepo;
  onUpdate?: () => Promise<void>;
  onRollback?: () => Promise<void>;
  onSetBaseTarget?: (input: string, clearOverlays: boolean) => Promise<boolean>;
  onApplyOverlays?: (selection: OverlaySelectionRequest) => Promise<OperationStart>;
};

type DraftOverlay = SelectedRepoOverlay & { key: string };

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

function materializationStatusClass(status: string): string {
  switch (status) {
    case "current":
      return "ok";
    case "stale":
      return "warn";
    case "missing":
    case "import_failed":
    case "replaced":
    case "failed":
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
  onApplyOverlays
}: Props) {
  const trackedState = repo.trackedState;
  const overlays = trackedState?.overlays ?? [];
  const [baseInput, setBaseInput] = useState("");
  const [overlayInput, setOverlayInput] = useState("");
  const [draftOverlays, setDraftOverlays] = useState<DraftOverlay[] | null>(null);
  const [draftBaseline, setDraftBaseline] = useState<Pick<OverlaySelectionRequest, "expectedBaseInput" | "expectedOverlays"> | null>(null);
  const [pendingOperationId, setPendingOperationId] = useState<string | null>(null);
  const [panelNotice, setPanelNotice] = useState<string | null>(null);
  const [isSubmitting, setIsSubmitting] = useState(false);
  const [panelError, setPanelError] = useState<string | null>(null);
  const [historyOpen, setHistoryOpen] = useState(false);
  const [checkpoints, setCheckpoints] = useState<RepoCheckpoint[]>([]);
  const [checkpointComparison, setCheckpointComparison] = useState<RepoCheckpointComparison | null>(null);
  const [updatePreview, setUpdatePreview] = useState<RepoActionPreview | null>(null);
  const [basePreview, setBasePreview] = useState<RepoActionPreview | null>(null);
  const [overlayPreview, setOverlayPreview] = useState<RepoActionPreview | null>(null);
  const integrationBranch = trackedState?.materializedBranch ?? repo.currentBranch ?? "detached";
  const hasOverlays = overlays.length > 0 || (!trackedState && repo.trackedTargetKind === "pr");
  const editableBaseInput = trackedState?.base.sourceInput ??
    (repo.trackedTargetKind !== "pr" ? repo.trackedTargetInput : null);
  const selectedOverlays = draftOverlays ?? overlays.map((overlay) => ({
    key: overlay.id,
    overlayId: overlay.id,
    sourceInput: null,
    enabled: overlay.enabled
  }));
  const baselineOverlays = draftBaseline?.expectedOverlays ?? overlays.map((overlay) => ({ overlayId: overlay.id, enabled: overlay.enabled }));
  const selectionChanged = draftOverlays !== null && (
    draftOverlays.length !== baselineOverlays.length || draftOverlays.some((choice, index) =>
      choice.overlayId !== baselineOverlays[index]?.overlayId || choice.enabled !== baselineOverlays[index]?.enabled
    )
  );
  const isBusy = isSubmitting || pendingOperationId !== null;
  const hasTrackedUpdate =
    trackedState !== null || (repo.trackedTargetKind !== null && repo.trackedTargetInput !== null);
  const lifecycleSupported = repo.kind !== "core";

  async function runStackAction(action: () => Promise<boolean>): Promise<boolean> {
    if (isBusy) {
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
    if (isBusy) {
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

  // Capture the original selection on the first edit, not the latest refreshed props.
  function stageOverlayDraft(next: DraftOverlay[]) {
    if (!draftBaseline && editableBaseInput) {
      setDraftBaseline({
        expectedBaseInput: editableBaseInput,
        expectedOverlays: overlays.map((overlay) => ({ overlayId: overlay.id, enabled: overlay.enabled }))
      });
    }
    setDraftOverlays(next);
  }

  // One operation materializes the final desired selection. Draft edits never alter the checkout.
  async function applySelectedOverlays() {
    if (!onApplyOverlays || !draftBaseline || !selectionChanged || isBusy) return;
    await runStackAction(async () => {
      const started = await onApplyOverlays({
        ...draftBaseline,
        overlays: selectedOverlays.map(({ overlayId, sourceInput, enabled }) => ({
          overlayId, sourceInput, enabled
        }))
      });
      setPendingOperationId(started.operationId);
      setPanelNotice("Applying the complete overlay selection with checkpointed recovery if materialization fails.");
      return true;
    });
  }

  useEffect(() => {
    if (!pendingOperationId) return;
    let cancelled = false;
    let unlisten: (() => void) | null = null;
    const finish = (success: boolean, error?: string | null) => {
      if (cancelled) return;
      setPendingOperationId(null);
      if (success) {
        setDraftOverlays(null);
        setDraftBaseline(null);
        setPanelError(null);
        setPanelNotice("Overlay selection applied. Repository state is refreshing.");
      } else {
        setPanelError(error || "Overlay selection failed. The draft is retained; inspect the operation log.");
        setPanelNotice(null);
      }
    };
    void api.subscribeOperationEvents((event) => {
      if (event.operationId !== pendingOperationId) return;
      if (event.phase === "done") finish(true);
      if (event.phase === "error") finish(false, event.message);
    }).then(async (unsubscribe) => {
      if (cancelled) {
        unsubscribe();
        return;
      }
      unlisten = unsubscribe;
      // A fast operation may finish before the event listener is registered.
      const operations = await api.listOperations(repo.installationId);
      const operation = operations.find((item) => item.id === pendingOperationId);
      if (operation?.status === "succeeded") finish(true);
      if (operation?.status === "failed") finish(false, operation.errorMessage);
    }).catch((error) => finish(false, toErrorMessage(error)));
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [pendingOperationId, repo.installationId]);

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

      {repo.kind === "kitchen" ? (
        <div className="preview">
          <div className="row between repo-preview-header">
            <div>
              <strong>Runtime materialization</strong>
              <div className="muted small">Source checkout state and installed Python artifact are tracked separately.</div>
            </div>
            {repo.materializationState ? (
              <span className={`badge ${materializationStatusClass(repo.materializationState.status)}`}>
                {repo.materializationState.status.replace("_", " ")}
              </span>
            ) : (
              <span className="badge">ComfyUI-managed runtime</span>
            )}
          </div>
          {repo.materializationState ? (
            <>
              <div className="grid two compact-grid">
                <div>
                  <div className="label">Materialized source HEAD</div>
                  <div className="mono small">{repo.materializationState.materializedHeadSha ?? "unknown"}</div>
                </div>
                <div>
                  <div className="label">Installed version</div>
                  <div className="mono small">{repo.materializationState.installedVersion ?? "unknown"}</div>
                </div>
                <div>
                  <div className="label">Built artifact SHA-256</div>
                  <div className="mono small">{repo.materializationState.artifactSha256 ?? "unknown"}</div>
                </div>
                <div>
                  <div className="label">Installed RECORD SHA-256</div>
                  <div className="mono small">{repo.materializationState.installedRecordSha256 ?? "unknown"}</div>
                </div>
              </div>
              <div className="muted small">
                Last materialized {formatTimestamp(repo.materializationState.lastMaterializedAt)}
              </div>
              {repo.materializationState.lastError ? (
                <div className="muted small">{repo.materializationState.lastError}</div>
              ) : null}
            </>
          ) : (
            <div className="muted small">
              This checkout is not overriding the Python environment. The current ComfyUI requirement owns the installed comfy-kitchen distribution.
            </div>
          )}
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
            {selectedOverlays.length ? `${selectedOverlays.filter((overlay) => overlay.enabled).length}/${selectedOverlays.length} selected` : "base only"}
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
            disabled={isBusy}
            placeholder="Branch, tag, commit, or GitHub tree URL"
            value={baseInput}
            onChange={(event) => setBaseInput(event.target.value)}
          />
          <button
            className="secondary"
            disabled={!baseInput.trim() || isBusy}
            onClick={() => void previewTarget(baseInput, hasOverlays, "base")}
          >
            Preview
          </button>
          {onSetBaseTarget ? (
            <button
              className="secondary"
              disabled={!baseInput.trim() || isBusy || selectionChanged}
              onClick={async () => {
                if (!onSetBaseTarget || !baseInput.trim() || isBusy || selectionChanged) {
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
            disabled={isBusy}
            placeholder="GitHub PR URL"
            value={overlayInput}
            onChange={(event) => setOverlayInput(event.target.value)}
          />
          <button
            className="secondary"
            disabled={!overlayInput.trim() || isBusy}
            onClick={() => void previewTarget(overlayInput, false, "overlay")}
          >
            Preview
          </button>
          {onApplyOverlays && editableBaseInput ? (
            <button
              disabled={!overlayInput.trim() || isBusy}
              onClick={() => {
                const source = overlayInput.trim();
                if (!source || isBusy) return;
                if (selectedOverlays.some((item) => item.sourceInput === source)) {
                  setPanelError("This PR URL is already staged.");
                  return;
                }
                stageOverlayDraft([...selectedOverlays, {
                  key: `new:${source}`,
                  overlayId: null,
                  sourceInput: source,
                  enabled: true
                }]);
                setOverlayInput("");
                setOverlayPreview(null);
                setPanelError(null);
                setPanelNotice(null);
              }}
            >
              Stage PR
            </button>
          ) : null}
        </div>
        {renderPreview(overlayPreview, "Preview a PR overlay to inspect the stack order, introduced commits, and conflict surface.")}

        {selectedOverlays.length ? (
          <div className="overlay-list">
            {selectedOverlays.map((choice, index) => {
              const overlay = overlays.find((item) => item.id === choice.overlayId);
              const status = overlay
                ? choice.enabled === overlay.enabled ? overlayEffectiveStatus(overlay) : "staged"
                : "new";
              return (
                <div key={choice.key} className="overlay-item">
                  <div className="row between overlay-row">
                    <label className="overlay-toggle">
                      <input
                        type="checkbox"
                        disabled={isBusy || !onApplyOverlays}
                        checked={choice.enabled}
                        onChange={(event) => {
                          stageOverlayDraft(selectedOverlays.map((item, i) =>
                            i === index ? { ...item, enabled: event.target.checked } : item
                          ));
                          setPanelNotice(null);
                        }}
                      />
                      <span>
                        <strong>{overlay?.summaryLabel ?? choice.sourceInput}</strong>
                        <span className="mono small overlay-meta">
                          {overlay ? `PR #${overlay.prNumber} on ${overlay.prBaseRef}` : "New PR (pending apply)"}
                        </span>
                      </span>
                    </label>
                    <span className={`badge ${overlay && status === overlayEffectiveStatus(overlay) ? overlayStatusClass(overlay) : "warn"}`}>
                      {status}
                    </span>
                  </div>
                  <div className="row between overlay-row">
                    <div className="mono small overlay-meta">
                      {overlay?.resolvedSha ?? overlay?.sourceInput ?? choice.sourceInput}
                    </div>
                    <div className="row gap overlay-actions">
                      <button
                        className="secondary"
                        disabled={index === 0 || isBusy || !onApplyOverlays}
                        onClick={() => {
                          const next = [...selectedOverlays];
                          [next[index - 1], next[index]] = [next[index], next[index - 1]];
                          stageOverlayDraft(next);
                          setPanelNotice(null);
                        }}
                      >
                        Up
                      </button>
                      <button
                        className="secondary"
                        disabled={index === selectedOverlays.length - 1 || isBusy || !onApplyOverlays}
                        onClick={() => {
                          const next = [...selectedOverlays];
                          [next[index + 1], next[index]] = [next[index], next[index + 1]];
                          stageOverlayDraft(next);
                          setPanelNotice(null);
                        }}
                      >
                        Down
                      </button>
                      <button
                        className="secondary"
                        disabled={isBusy || !onApplyOverlays}
                        onClick={() => {
                          stageOverlayDraft(selectedOverlays.filter((_, i) => i !== index));
                          setPanelNotice(null);
                        }}
                      >
                        Remove
                      </button>
                    </div>
                  </div>
                  {overlay?.lastError ? <div className="muted small">{overlay.lastError}</div> : null}
                </div>
              );
            })}
          </div>
        ) : (
          <div className="muted small">No PR overlays selected.</div>
        )}
        {onApplyOverlays && !editableBaseInput ? (
          <div className="muted small">
            This legacy PR target has no editable base selection. Replace the base target to migrate it before staging overlays.
          </div>
        ) : null}
        {onApplyOverlays && editableBaseInput ? (
          <div className="stack">
            <div className="muted small">
              Editing overlays does not modify the checkout or start a download/build. Apply selection validates the complete stack (including stacked-PR dependencies) and starts one operation.
            </div>
            <div className="row gap repo-action-wrap">
              <button
                className="secondary"
                disabled={isBusy || selectedOverlays.length === 0}
                onClick={() => {
                  stageOverlayDraft([]);
                  setPanelNotice(null);
                }}
              >
                Remove all
              </button>
              <button
                className="secondary"
                disabled={isBusy || draftOverlays === null}
                onClick={() => {
                  setDraftOverlays(null);
                  setDraftBaseline(null);
                  setPanelError(null);
                  setPanelNotice(null);
                }}
              >
                Discard changes
              </button>
              <button
                disabled={isBusy || !selectionChanged}
                onClick={() => void applySelectedOverlays()}
              >
                {pendingOperationId ? "Applying selection..." : "Apply selection"}
              </button>
            </div>
            {selectionChanged ? (
              <div className="muted small">
                Pending: {selectedOverlays.length} overlays selected ({overlays.length} currently tracked).
              </div>
            ) : null}
          </div>
        ) : null}
      </div>

      <div className="row gap repo-action-wrap">
        {hasTrackedUpdate ? (
          <button
            className="secondary"
            disabled={isBusy}
            onClick={() =>
              void runLocalAction(async () => {
                const preview = await api.previewTrackedRepoUpdate(repo.id);
                setUpdatePreview(preview);
              })
            }
          >
            Preview update
          </button>
        ) : null}
        {hasTrackedUpdate && onUpdate ? (
          <button disabled={isBusy || selectionChanged} onClick={() => void runLocalAction(onUpdate)}>
            Update
          </button>
        ) : null}
        {onRollback ? (
          <button
            className="secondary"
            disabled={isBusy}
            onClick={() => void runLocalAction(onRollback)}
          >
            Rollback latest
          </button>
        ) : null}
        {repo.kind === "kitchen" && repo.materializationState ? (
          <button
            className="secondary"
            disabled={isBusy}
            onClick={() =>
              void runLocalAction(async () => {
                if (
                  !window.confirm(
                    "Restore the comfy-kitchen requirement declared by the current ComfyUI checkout? The source checkout stays on disk, but its tracked source target and built runtime override are deactivated. Local source changes stay in place and are retained in the rollback checkpoint. Use Install / Patch source or set a tracked base target to enable source management again."
                  )
                ) {
                  return;
                }
                await api.restoreComfyManagedKitchen({
                  repoId: repo.id,
                  restartAfterSuccess: false
                });
              })
            }
          >
            Restore ComfyUI Kitchen
          </button>
        ) : null}
        <button className="secondary" disabled={isBusy} onClick={() => void toggleHistory()}>
          {historyOpen ? "Hide history" : "History"}
        </button>
      </div>
      {hasTrackedUpdate
        ? renderPreview(updatePreview, "Preview the tracked update plan to inspect incoming commits and files before mutating the checkout.")
        : null}

      {historyOpen ? (
        <div className="repo-history-panel">
          <div className="row between repo-history-header">
            <div>
              <strong>Checkpoint history</strong>
              <div className="muted small">{checkpoints.length} checkpoint{checkpoints.length === 1 ? "" : "s"}</div>
            </div>
            <button
              className="secondary"
              disabled={isBusy}
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
                  {checkpoint.materializationState ? (
                    <div className="muted small">
                      Runtime snapshot: {checkpoint.materializationState.status.replace("_", " ")} at{" "}
                      <span className="mono">
                        {checkpoint.materializationState.materializedHeadSha ?? "unknown source HEAD"}
                      </span>
                    </div>
                  ) : repo.kind === "kitchen" ? (
                    <div className="muted small">Runtime snapshot: ComfyUI-managed comfy-kitchen requirement</div>
                  ) : null}
                  <div className="row gap repo-action-wrap">
                    <button
                      className="secondary"
                      disabled={isBusy}
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
                      disabled={isBusy}
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
            {repo.kind === "kitchen" && repo.materializationState
              ? "Uninstall and Disable first restore the comfy-kitchen requirement declared by the current ComfyUI checkout. Untrack instead leaves the currently installed runtime unchanged and only stops future Patcher source reassertion. Lifecycle actions do not create new checkpoints."
              : "Lifecycle actions do not create new checkpoints. They remove or hide the repo directly, and untrack also suppresses future reconcile rediscovery for this path."}
          </div>
          <div className="row gap repo-action-wrap">
            <button
              className="secondary"
              disabled={isBusy}
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
              disabled={isBusy}
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
              disabled={isBusy}
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

      {panelNotice ? <div className="muted small">{panelNotice}</div> : null}
      {panelError ? <div className="muted small">{panelError}</div> : null}
    </div>
  );
}
