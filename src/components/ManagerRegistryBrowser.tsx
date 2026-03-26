import { useEffect, useRef, useState } from "react";
import { api } from "../api";
import type { ManagerRegistryCustomNode } from "../types";

type Props = {
  installationId: string;
  refreshToken: number;
  onInstall: (sourceInput: string) => Promise<void>;
  onUseSourceInput: (sourceInput: string) => void;
};

function toErrorMessage(error: unknown): string {
  if (error instanceof Error) return error.message;
  return String(error);
}

export default function ManagerRegistryBrowser({
  installationId,
  refreshToken,
  onInstall,
  onUseSourceInput
}: Props) {
  const [query, setQuery] = useState("");
  const [entries, setEntries] = useState<ManagerRegistryCustomNode[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const requestSeq = useRef(0);

  async function refresh(nextQuery: string) {
    const requestId = ++requestSeq.current;
    setLoading(true);
    setError(null);
    try {
      const next = await api.listManagerCustomNodes({
        installationId,
        query: nextQuery.trim() || null,
        limit: 120
      });
      if (requestSeq.current !== requestId) return;
      setEntries(next);
    } catch (err) {
      if (requestSeq.current !== requestId) return;
      setEntries([]);
      setError(toErrorMessage(err));
    } finally {
      if (requestSeq.current === requestId) {
        setLoading(false);
      }
    }
  }

  useEffect(() => {
    const timeoutId = window.setTimeout(() => {
      void refresh(query);
    }, 180);
    return () => window.clearTimeout(timeoutId);
  }, [installationId, query, refreshToken]);

  return (
    <div className="card">
      <div className="row between registry-header">
        <div>
          <h3>Browse ComfyUI-Manager registry</h3>
          <div className="muted small">
            Search the Manager catalog and install through ComfyUI Patcher.
          </div>
        </div>
        <button className="secondary" onClick={() => void refresh(query)}>
          Refresh
        </button>
      </div>

      <input
        placeholder="Search title, id, author, description, or repo URL"
        value={query}
        onChange={(event) => setQuery(event.target.value)}
      />

      {error ? <div className="muted">{error}</div> : null}
      {loading ? <div className="muted">Loading registry entries…</div> : null}

      <div className="list registry-list">
        {entries.map((entry) => (
          <div key={`${entry.registryId}:${entry.canonicalRepoUrl ?? entry.sourceInput ?? entry.title}`} className="list-item registry-item">
            <div className="row between registry-item-header">
              <div>
                <strong>{entry.title}</strong>
                <div className="muted small">
                  {entry.author ? `${entry.author} · ` : ""}
                  {entry.registryId}
                </div>
              </div>
              <div className="badge-group registry-badges">
                <span className="badge">{entry.installType}</span>
                {entry.isInstalled ? (
                  <span className="badge ok">installed</span>
                ) : entry.isInstallable ? (
                  <span className="badge">available</span>
                ) : (
                  <span className="badge warn">unsupported</span>
                )}
              </div>
            </div>

            {entry.description ? <div className="small registry-description">{entry.description}</div> : null}

            <div className="mono small registry-source">
              {entry.canonicalRepoUrl ?? entry.sourceInput ?? "No source URL available"}
            </div>

            {entry.isInstalled && entry.installedLocalPath ? (
              <div className="small muted">
                Installed at <span className="mono">{entry.installedLocalPath}</span>
              </div>
            ) : null}

            <div className="row gap registry-actions">
              <button
                disabled={!entry.isInstallable || !entry.sourceInput}
                onClick={() => entry.sourceInput && void onInstall(entry.sourceInput)}
              >
                {entry.isInstalled ? "Patch existing" : "Install"}
              </button>
              {entry.sourceInput && entry.isInstallable ? (
                <button
                  className="secondary"
                  onClick={() => onUseSourceInput(entry.sourceInput!)}
                >
                  Use URL
                </button>
              ) : null}
            </div>
          </div>
        ))}
      </div>

      {!loading && !entries.length && !error ? (
        <div className="muted">No registry entries matched the current search.</div>
      ) : null}
    </div>
  );
}
