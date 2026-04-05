import { useState } from "react";
import type {
  ManagedRepo,
  OverlayApplyStatus,
  OverlayMoveDirection,
  TrackedPrOverlay
} from "../types";

type Props = {
  repo: ManagedRepo;
  onUpdate?: () => void;
  onRollback?: () => void;
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
  const integrationBranch = trackedState?.materializedBranch ?? repo.currentBranch ?? "detached";
  const hasOverlays = overlays.length > 0;

  async function runStackAction(action: () => Promise<boolean>): Promise<boolean> {
    if (isSubmitting) {
      return false;
    }
    setIsSubmitting(true);
    try {
      return await action();
    } finally {
      setIsSubmitting(false);
    }
  }

  return (
    <div className="card">
      <div className="row between repo-card-header">
        <div>
          <h3>{repo.displayName}</h3>
          <div className="muted mono">{repo.localPath}</div>
        </div>
        <div className="badge-group">
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
          <div className="label">Remote</div>
          <div className="mono small">{repo.canonicalRemote ?? "unknown"}</div>
        </div>
      </div>

      <div className="repo-stack-panel">
        <div className="row between repo-stack-header">
          <div>
            <div className="label">Base target</div>
            <div className="mono small">
              {trackedState ? trackedState.base.summaryLabel : repo.trackedTargetInput ?? "none"}
            </div>
          </div>
          <span className="badge">{hasOverlays ? `${overlays.filter((overlay) => overlay.enabled).length}/${overlays.length} overlays` : "base only"}</span>
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
          {onSetBaseTarget ? (
            <button
              className="secondary"
              disabled={!baseInput.trim() || isSubmitting}
              onClick={async () => {
                if (!onSetBaseTarget || !baseInput.trim() || isSubmitting) {
                  return;
                }
                const ok = await runStackAction(() =>
                  onSetBaseTarget(baseInput, hasOverlays)
                );
                if (ok) {
                  setBaseInput("");
                }
              }}
            >
              {hasOverlays ? "Replace base" : "Set base"}
            </button>
          ) : null}
        </div>
        {hasOverlays ? (
          <div className="muted small">Replacing the base clears the stored overlay stack before rebuilding.</div>
        ) : null}

        <div className="row gap repo-stack-inputs">
          <input
            className="grow"
            disabled={isSubmitting}
            placeholder="GitHub PR URL"
            value={overlayInput}
            onChange={(event) => setOverlayInput(event.target.value)}
          />
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
                }
              }}
            >
              Add PR overlay
            </button>
          ) : null}
        </div>

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

      <div className="row gap">
        {onUpdate ? <button onClick={onUpdate}>Update</button> : null}
        {onRollback ? <button className="secondary" onClick={onRollback}>Rollback</button> : null}
      </div>
    </div>
  );
}
