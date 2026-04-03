interface DiffViewerProps {
  diff: string;
  loading: boolean;
  filePath: string | null;
  commitHash: string | null;
}

interface DiffLine {
  type: "context" | "addition" | "deletion" | "hunk-header";
  oldNum: number | null;
  newNum: number | null;
  content: string;
}

function parseDiff(raw: string): { file: string; lines: DiffLine[] }[] {
  const files: { file: string; lines: DiffLine[] }[] = [];
  let current: { file: string; lines: DiffLine[] } | null = null;
  let oldLine = 0;
  let newLine = 0;

  for (const line of raw.split("\n")) {
    // New file header
    if (line.startsWith("diff --git")) {
      const match = line.match(/b\/(.+)$/);
      current = { file: match?.[1] ?? "unknown", lines: [] };
      files.push(current);
      continue;
    }

    if (!current) continue;

    // Skip file metadata lines
    if (
      line.startsWith("index ") ||
      line.startsWith("---") ||
      line.startsWith("+++") ||
      line.startsWith("new file") ||
      line.startsWith("deleted file") ||
      line.startsWith("similarity index") ||
      line.startsWith("rename from") ||
      line.startsWith("rename to") ||
      line.startsWith("Binary files")
    ) {
      continue;
    }

    // Hunk header
    if (line.startsWith("@@")) {
      const match = line.match(/@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@(.*)/);
      if (match) {
        oldLine = parseInt(match[1]);
        newLine = parseInt(match[2]);
        current.lines.push({
          type: "hunk-header",
          oldNum: null,
          newNum: null,
          content: line,
        });
      }
      continue;
    }

    // Diff content lines
    if (line.startsWith("+")) {
      current.lines.push({
        type: "addition",
        oldNum: null,
        newNum: newLine++,
        content: line.slice(1),
      });
    } else if (line.startsWith("-")) {
      current.lines.push({
        type: "deletion",
        oldNum: oldLine++,
        newNum: null,
        content: line.slice(1),
      });
    } else if (line.startsWith(" ") || line === "") {
      current.lines.push({
        type: "context",
        oldNum: oldLine++,
        newNum: newLine++,
        content: line.startsWith(" ") ? line.slice(1) : line,
      });
    }
  }

  return files;
}

export default function DiffViewer({
  diff,
  loading,
  filePath,
  commitHash,
}: DiffViewerProps) {
  if (loading) {
    return (
      <div className="diff-container">
        <div className="loading">Loading diff</div>
      </div>
    );
  }

  if (!diff) {
    return (
      <div className="diff-container">
        <div className="diff-empty">
          Select a file or commit to view its diff
        </div>
      </div>
    );
  }

  const parsed = parseDiff(diff);

  return (
    <div className="diff-container">
      {parsed.map((file, fi) => (
        <div key={fi}>
          {/* Show file header when viewing a commit (multiple files) */}
          {(commitHash || parsed.length > 1) && (
            <div className="diff-header">{file.file}</div>
          )}

          <table className="diff-table">
            <colgroup>
              <col style={{ width: 50 }} />
              <col style={{ width: 50 }} />
              <col style={{ width: 20 }} />
              <col />
            </colgroup>
            <tbody>
              {file.lines.map((line, li) => {
                if (line.type === "hunk-header") {
                  return (
                    <tr key={li} className="diff-hunk-header">
                      <td colSpan={4}>{line.content}</td>
                    </tr>
                  );
                }

                const sign =
                  line.type === "addition"
                    ? "+"
                    : line.type === "deletion"
                      ? "-"
                      : " ";

                return (
                  <tr key={li} className={`diff-line ${line.type}`}>
                    <td className="line-num">
                      {line.oldNum ?? ""}
                    </td>
                    <td className="line-num">
                      {line.newNum ?? ""}
                    </td>
                    <td className="line-sign">{sign}</td>
                    <td className="line-content">{line.content}</td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      ))}
    </div>
  );
}
