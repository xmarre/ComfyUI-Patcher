import { useEffect, useMemo, useRef, useState } from "react";
import { api } from "../api";
import type { ManagerRegistryCustomNode } from "../types";

type Props = {
  installationId: string;
  refreshToken: number;
  onInstall: (entry: ManagerRegistryCustomNode) => Promise<void>;
  onUseSourceInput: (sourceInput: string) => void;
};

function toErrorMessage(error: unknown): string {
  if (error instanceof Error) return error.message;
  return String(error);
}

const FETCH_LIMIT = 10000;
const PAGE_SIZE = 250;

function entrySearchText(entry: ManagerRegistryCustomNode): string {
  return [
    entry.title,
    entry.registryId,
    entry.author ?? "",
    entry.description ?? "",
    entry.canonicalRepoUrl ?? "",
    entry.sourceInput ?? ""
  ]
    .join("\n")
    .toLocaleLowerCase();
}

export default function ManagerRegistryBrowser({
  installationId,
  refreshToken,
  onInstall,
  onUseSourceInput
}: Props) {
  const [query, setQuery] = useState("");
  const [entries, setEntries] = useState<ManagerRegistryCustomNode[]>([]);
  const [visibleCount, setVisibleCount] = useState(PAGE_SIZE);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const requestSeq = useRef(0);

  async function refresh() {
    const requestId = ++requestSeq.current;
    setLoading(true);
    setError(null);
    try {
      const next = await api.listManagerCustomNodes({
        installationId,
        query: null,
        limit: FETCH_LIMIT
      });
      if (requestSeq.current !== requestId) return;
      setEntries(next);
    } catch (err) {
      if (requestSeq.current !== requestId) return;
      setError(toErrorMessage(err));
    } finally {
      if (requestSeq.current === requestId) {
        setLoading(false);
      }
    }
  }

  useEffect(() => {
    setEntries([]);
    setError(null);
    setVisibleCount(PAGE_SIZE);
    requestSeq.current += 1;
    void refresh();
  }, [installationId, refreshToken]);

  useEffect(() => {
    setVisibleCount(PAGE_SIZE);
  }, [query, entries]);

  const filteredEntries = useMemo(() => {
    const normalizedQuery = query.trim().toLocaleLowerCase();
    if (!normalizedQuery) {
      return entries;
    }
    return entries.filter((entry) => entrySearchText(entry).includes(normalizedQuery));
  }, [entries, query]);

  const visibleEntries = useMemo(
    () => filteredEntries.slice(0, visibleCount),
    [filteredEntries, visibleCount]
  );

  const hasGitInstallation = (entry: ManagerRegistryCustomNode) =>
    Boolean(entry.installedRepoId || entry.installedLocalPath);
  const hasTrackingInstallation = (entry: ManagerRegistryCustomNode) => entry.isTrackingManaged;
  const shouldOfferTrackingAdoption = (entry: ManagerRegistryCustomNode) =>
    hasTrackingInstallation(entry) && !hasGitInstallation(entry);

  return (
    <div className="card">
      <div className="row between registry-header">
        <div>
          <h3>Browse ComfyUI-Manager registry</h3>
          <div className="muted small">
            Search the Manager catalog and install through ComfyUI Patcher.
          </div>
        </div>
        <button className="secondary" onClick={() => void refresh()}>
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

      <div className="small muted">
        Showing {visibleEntries.length} of {filteredEntries.length} loaded matching entries
        {filteredEntries.length !== entries.length ? ` (from ${entries.length} loaded)` : ""}.
      </div>

      <div className="list registry-list">
        {visibleEntries.map((entry) => (
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
                {entry.hasAmbiguousInstallation ? (
                  <span className="badge warn">duplicate installs</span>
                ) : hasGitInstallation(entry) || hasTrackingInstallation(entry) ? (
                  <span className="badge ok">installed</span>
                ) : entry.isPresentNonGit ? (
                  <span className="badge">present</span>
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

            {hasGitInstallation(entry) && entry.installedLocalPath ? (
              <div className="small muted">
                Installed as git repo at <span className="mono">{entry.installedLocalPath}</span>
              </div>
            ) : null}

            {hasTrackingInstallation(entry) && entry.trackingLocalPath ? (
              <div className="small muted">
                Managed via .tracking at <span className="mono">{entry.trackingLocalPath}</span>
              </div>
            ) : null}

            {entry.isPresentNonGit && entry.presentLocalPath ? (
              <div className="small muted">
                Unmanaged folder also present at <span className="mono">{entry.presentLocalPath}</span>
              </div>
            ) : null}

            {entry.hasAmbiguousInstallation ? (
              <div className="small muted">
                Multiple local directories match this remote. Resolve duplicates manually before patching.
              </div>
            ) : null}

            <div className="row gap registry-actions">
              <button
                disabled={
                  !entry.isInstallable ||
                  !entry.sourceInput ||
                  entry.hasAmbiguousInstallation ||
                  (!hasGitInstallation(entry) && !hasTrackingInstallation(entry) && entry.isPresentNonGit)
                }
                onClick={() => void onInstall(entry)}
              >
                {entry.hasAmbiguousInstallation
                  ? "Resolve duplicates first"
                  : shouldOfferTrackingAdoption(entry)
                    ? "Adopt tracked install"
                  : hasGitInstallation(entry)
                    ? "Patch existing"
                  : entry.isPresentNonGit
                    ? "Manual migration needed"
                    : "Install"}
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

      {!loading && filteredEntries.length > visibleEntries.length ? (
        <div className="row gap">
          <button className="secondary" onClick={() => setVisibleCount((value) => value + PAGE_SIZE)}>
            Show more
          </button>
        </div>
      ) : null}

      {!loading && !filteredEntries.length && !error ? (
        <div className="muted">No registry entries matched the current search.</div>
      ) : null}
    </div>
  );
}
