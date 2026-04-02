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
  const [isSubmittingBase, setIsSubmittingBase] = useState(false);
  const [isSubmittingOverlay, setIsSubmittingOverlay] = useState(false);
  const integrationBranch = trackedState?.materializedBranch ?? repo.currentBranch ?? "detached";
  const hasOverlays = overlays.length > 0;

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
            placeholder="Branch, tag, commit, or GitHub tree URL"
            value={baseInput}
            onChange={(event) => setBaseInput(event.target.value)}
          />
          {onSetBaseTarget ? (
            <button
              className="secondary"
              disabled={!baseInput.trim() || isSubmittingBase}
              onClick={async () => {
                if (!onSetBaseTarget || !baseInput.trim() || isSubmittingBase) {
                  return;
                }
                setIsSubmittingBase(true);
                try {
                  const ok = await onSetBaseTarget(baseInput, hasOverlays);
                  if (ok) {
                    setBaseInput("");
                  }
                } finally {
                  setIsSubmittingBase(false);
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
            placeholder="GitHub PR URL"
            value={overlayInput}
            onChange={(event) => setOverlayInput(event.target.value)}
          />
          {onAddOverlay ? (
            <button
              disabled={!overlayInput.trim() || isSubmittingOverlay}
              onClick={async () => {
                if (!onAddOverlay || !overlayInput.trim() || isSubmittingOverlay) {
                  return;
                }
                setIsSubmittingOverlay(true);
                try {
                  const ok = await onAddOverlay(overlayInput);
                  if (ok) {
                    setOverlayInput("");
                  }
                } finally {
                  setIsSubmittingOverlay(false);
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
                        checked={overlay.enabled}
                        onChange={(event) =>
                          void onSetOverlayEnabled?.(overlay.id, event.target.checked)
                        }
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
                        disabled={index === 0}
                        onClick={() => void onMoveOverlay?.(overlay.id, "up")}
                      >
                        Up
                      </button>
                      <button
                        className="secondary"
                        disabled={index === overlays.length - 1}
                        onClick={() => void onMoveOverlay?.(overlay.id, "down")}
                      >
                        Down
                      </button>
                      <button
                        className="secondary"
                        onClick={() => void onRemoveOverlay?.(overlay.id)}
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
