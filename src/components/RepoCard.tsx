import type { ManagedRepo } from "../types";

type Props = {
  repo: ManagedRepo;
  onUpdate?: () => void;
  onRollback?: () => void;
};

export default function RepoCard({ repo, onUpdate, onRollback }: Props) {
  return (
    <div className="card">
      <div className="row between">
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
          <div className="label">Branch</div>
          <div className="mono">{repo.currentBranch ?? "detached"}</div>
        </div>
        <div>
          <div className="label">HEAD</div>
          <div className="mono">{repo.currentHeadSha ?? "unknown"}</div>
        </div>
        <div>
          <div className="label">Remote</div>
          <div className="mono small">{repo.canonicalRemote ?? "unknown"}</div>
        </div>
        <div>
          <div className="label">Tracked target</div>
          <div className="mono small">{repo.trackedTargetInput ?? "none"}</div>
        </div>
      </div>
      <div className="row gap">
        {onUpdate ? <button onClick={onUpdate}>Update</button> : null}
        {onRollback ? <button className="secondary" onClick={onRollback}>Rollback</button> : null}
      </div>
    </div>
  );
}
