import type { ChangedFile, Commit } from "../hooks/useApi";

interface FileTreeProps {
  files: ChangedFile[];
  commits: Commit[];
  selectedFile: string | null;
  selectedCommit: string | null;
  onSelectFile: (path: string) => void;
  onSelectCommit: (hash: string | null) => void;
}

const STATUS_LABELS: Record<string, string> = {
  added: "A",
  modified: "M",
  deleted: "D",
  renamed: "R",
};

function splitPath(path: string): { dir: string; name: string } {
  const idx = path.lastIndexOf("/");
  if (idx === -1) return { dir: "", name: path };
  return { dir: path.slice(0, idx + 1), name: path.slice(idx + 1) };
}

export default function FileTree({
  files,
  commits,
  selectedFile,
  selectedCommit,
  onSelectFile,
  onSelectCommit,
}: FileTreeProps) {
  return (
    <div className="sidebar">
      <div className="sidebar-section">
        <div className="sidebar-section-header">
          Changed files <span className="count">{files.length}</span>
        </div>
      </div>
      <div className="file-list">
        {files.map((f) => {
          const { dir, name } = splitPath(f.path);
          return (
            <div
              key={f.path}
              className={`file-item ${selectedFile === f.path ? "active" : ""}`}
              onClick={() => {
                onSelectCommit(null);
                onSelectFile(f.path);
              }}
            >
              <span className={`status ${f.status}`}>
                {STATUS_LABELS[f.status]}
              </span>
              <span className="name">
                {dir && <span className="dir">{dir}</span>}
                {name}
              </span>
              <span className="stats">
                {f.additions > 0 && (
                  <span className="add">+{f.additions}</span>
                )}
                {f.deletions > 0 && (
                  <span className="del">-{f.deletions}</span>
                )}
              </span>
            </div>
          );
        })}
      </div>

      {commits.length > 0 && (
        <>
          <div className="sidebar-section">
            <div className="sidebar-section-header">
              Commits <span className="count">{commits.length}</span>
            </div>
          </div>
          <div className="commit-list">
            {commits.map((c) => (
              <div
                key={c.hash}
                className={`commit-item ${selectedCommit === c.hash ? "active" : ""}`}
                onClick={() => onSelectCommit(c.hash)}
              >
                <span className="hash">{c.shortHash}</span>
                <span className="subject">{c.subject}</span>
              </div>
            ))}
          </div>
        </>
      )}
    </div>
  );
}
