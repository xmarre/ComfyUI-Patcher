import { useEffect, useMemo, useRef, useState } from "react";
import { api } from "../api";
import type { OperationRecord } from "../types";

type Props = {
  installationId: string | null;
};

function formatTimestamp(value: string | null): string {
  if (!value) {
    return "—";
  }
  return new Date(value).toLocaleString();
}

function operationSearchText(operation: OperationRecord): string {
  return [
    operation.kind,
    operation.id,
    operation.status,
    operation.requestedInput ?? "",
    operation.errorMessage ?? "",
    operation.repoId ?? ""
  ]
    .join("\n")
    .toLocaleLowerCase();
}

export default function OperationPanel({ installationId }: Props) {
  const [operations, setOperations] = useState<OperationRecord[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [query, setQuery] = useState("");
  const [log, setLog] = useState("");
  const [logLoading, setLogLoading] = useState(false);
  const selectedIdRef = useRef<string | null>(null);
  const logRequestSeq = useRef(0);

  const selected = useMemo(
    () => operations.find((operation) => operation.id === selectedId) ?? null,
    [operations, selectedId]
  );

  const filteredOperations = useMemo(() => {
    const normalizedQuery = query.trim().toLocaleLowerCase();
    if (!normalizedQuery) {
      return operations;
    }
    return operations.filter((operation) => operationSearchText(operation).includes(normalizedQuery));
  }, [operations, query]);

  const runningCount = useMemo(
    () => operations.filter((operation) => operation.status === "running").length,
    [operations]
  );

  async function loadLog(operationId: string) {
    const requestId = ++logRequestSeq.current;
    setLogLoading(true);
    try {
      const nextLog = await api.getOperationLog(operationId);
      if (logRequestSeq.current !== requestId) {
        return;
      }
      setLog(nextLog);
    } finally {
      if (logRequestSeq.current === requestId) {
        setLogLoading(false);
      }
    }
  }

  useEffect(() => {
    selectedIdRef.current = selectedId;
  }, [selectedId]);

  async function refresh() {
    const next = await api.listOperations(installationId);
    setOperations(next);

    const currentSelectedId = selectedIdRef.current;
    if (!currentSelectedId) {
      return;
    }
    const stillSelected = next.find((operation) => operation.id === currentSelectedId) ?? null;
    if (!stillSelected) {
      selectedIdRef.current = null;
      setSelectedId(null);
      setLog("");
      setLogLoading(false);
      logRequestSeq.current += 1;
      return;
    }
    await loadLog(stillSelected.id);
  }

  useEffect(() => {
    setOperations([]);
    setSelectedId(null);
    selectedIdRef.current = null;
    setQuery("");
    setLog("");
    setLogLoading(false);
    logRequestSeq.current += 1;

    void refresh();
    const intervalId = window.setInterval(() => {
      void refresh();
    }, 2500);
    return () => window.clearInterval(intervalId);
  }, [installationId]);

  return (
    <div className="card fill operations-card">
      <div className="row between operations-header">
        <div>
          <h3>Operations</h3>
          <div className="muted small">
            {operations.length} total · {runningCount} running
          </div>
        </div>
        <button className="secondary" type="button" onClick={() => void refresh()}>
          Refresh
        </button>
      </div>

      <div className="row gap operations-toolbar">
        <input
          className="grow"
          placeholder="Search kind, id, requested input, or status"
          value={query}
          onChange={(event) => setQuery(event.target.value)}
        />
        <div className="small muted operations-summary">
          Showing {filteredOperations.length} of {operations.length}
        </div>
      </div>

      <div className="operations-layout">
        <div className="operations-list-panel panel-scroll">
          {filteredOperations.length ? (
            <div className="list operations-list">
              {filteredOperations.map((operation) => (
                <button
                  key={operation.id}
                  type="button"
                  className={`list-item ${selected?.id === operation.id ? "active" : ""}`}
                  onClick={async () => {
                    setSelectedId(operation.id);
                    await loadLog(operation.id);
                  }}
                >
                  <div className="row between">
                    <strong>{operation.kind}</strong>
                    <span
                      className={`badge ${operation.status === "failed" ? "danger" : operation.status === "succeeded" ? "ok" : operation.status === "running" ? "warn" : ""}`}
                    >
                      {operation.status}
                    </span>
                  </div>
                  <div className="mono small operation-id">{operation.id}</div>
                  <div className="small operation-input-preview">
                    {operation.requestedInput ?? "no input"}
                  </div>
                  <div className="small muted">{formatTimestamp(operation.createdAt)}</div>
                </button>
              ))}
            </div>
          ) : (
            <div className="muted">No operations matched the current search.</div>
          )}
        </div>

        <div className="log-viewer operation-detail-panel">
          {selected ? (
            <>
              <div className="row between operation-detail-header">
                <div>
                  <div>
                    <strong>{selected.kind}</strong>
                  </div>
                  <div className="small mono">{selected.id}</div>
                </div>
                <div className="row gap operation-detail-actions">
                  <span
                    className={`badge ${selected.status === "failed" ? "danger" : selected.status === "succeeded" ? "ok" : selected.status === "running" ? "warn" : ""}`}
                  >
                    {selected.status}
                  </span>
                  <button
                    className="secondary"
                    type="button"
                    onClick={() => {
                      setSelectedId(null);
                      setLog("");
                      setLogLoading(false);
                      logRequestSeq.current += 1;
                    }}
                  >
                    Close
                  </button>
                </div>
              </div>

              <div className="operation-meta-grid">
                <div>
                  <div className="label">Created</div>
                  <div className="small mono">{formatTimestamp(selected.createdAt)}</div>
                </div>
                <div>
                  <div className="label">Started</div>
                  <div className="small mono">{formatTimestamp(selected.startedAt)}</div>
                </div>
                <div>
                  <div className="label">Finished</div>
                  <div className="small mono">{formatTimestamp(selected.finishedAt)}</div>
                </div>
                <div>
                  <div className="label">Repo</div>
                  <div className="small mono">{selected.repoId ?? "installation-wide"}</div>
                </div>
              </div>

              {selected.requestedInput ? (
                <div className="preview operation-preview-block">
                  <div className="label">Requested input</div>
                  <div className="small mono">{selected.requestedInput}</div>
                </div>
              ) : null}

              {selected.errorMessage ? (
                <div className="preview operation-preview-block operation-error-block">
                  <div className="label">Error</div>
                  <div className="small mono">{selected.errorMessage}</div>
                </div>
              ) : null}

              <pre className="panel-scroll operation-log-output">
                {logLoading ? "Loading log output…" : log || "No log output yet."}
              </pre>
            </>
          ) : (
            <div className="operation-empty-state muted">
              Select an operation to inspect its metadata and log output.
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
