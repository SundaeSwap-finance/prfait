import { useMemo, useState, useCallback, useEffect, useRef } from "react";
import { useHighlightedLines } from "../hooks/useHighlighter";
import type { Snippet } from "../hooks/useStory";

interface DiffViewerProps {
  diff: string;
  loading: boolean;
  filePath: string | null;
  commitHash: string | null;
  onAddSnippet: (snippet: Omit<Snippet, "id">) => void;
}

interface DiffLine {
  type: "context" | "addition" | "deletion" | "hunk-header";
  oldNum: number | null;
  newNum: number | null;
  content: string;
  /** Index in the flat lines array (for selection tracking) */
  index: number;
}

interface ParsedFile {
  file: string;
  lines: DiffLine[];
}

/** Which lines belong to each hunk (by index range in the lines array) */
interface HunkRange {
  start: number; // index of hunk-header in lines array
  end: number; // index of last line in this hunk
}

function parseDiff(raw: string): ParsedFile[] {
  const files: ParsedFile[] = [];
  let current: ParsedFile | null = null;
  let oldLine = 0;
  let newLine = 0;
  let idx = 0;

  for (const line of raw.split("\n")) {
    if (line.startsWith("diff --git")) {
      const match = line.match(/b\/(.+)$/);
      current = { file: match?.[1] ?? "unknown", lines: [] };
      files.push(current);
      idx = 0;
      continue;
    }

    if (!current) continue;

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
          index: idx++,
        });
      }
      continue;
    }

    if (line.startsWith("+")) {
      current.lines.push({
        type: "addition",
        oldNum: null,
        newNum: newLine++,
        content: line.slice(1),
        index: idx++,
      });
    } else if (line.startsWith("-")) {
      current.lines.push({
        type: "deletion",
        oldNum: oldLine++,
        newNum: null,
        content: line.slice(1),
        index: idx++,
      });
    } else if (line.startsWith(" ") || line === "") {
      current.lines.push({
        type: "context",
        oldNum: oldLine++,
        newNum: newLine++,
        content: line.startsWith(" ") ? line.slice(1) : line,
        index: idx++,
      });
    }
  }

  return files;
}

function getHunkRanges(lines: DiffLine[]): HunkRange[] {
  const ranges: HunkRange[] = [];
  for (let i = 0; i < lines.length; i++) {
    if (lines[i].type === "hunk-header") {
      if (ranges.length > 0) {
        ranges[ranges.length - 1].end = i - 1;
      }
      ranges.push({ start: i, end: lines.length - 1 });
    }
  }
  return ranges;
}

function lineLabel(line: DiffLine): string {
  if (line.newNum != null) return String(line.newNum);
  if (line.oldNum != null) return String(line.oldNum);
  return "";
}

// ── HighlightedFile ──────────────────────────────────────────────────

function HighlightedFile({
  file,
  commitHash,
  multiFile,
  onAddSnippet,
}: {
  file: ParsedFile;
  commitHash: string | null;
  multiFile: boolean;
  onAddSnippet: (snippet: Omit<Snippet, "id">) => void;
}) {
  const code = useMemo(
    () =>
      file.lines
        .filter((l) => l.type !== "hunk-header")
        .map((l) => l.content)
        .join("\n"),
    [file.lines]
  );

  const highlighted = useHighlightedLines(code, file.file);
  const hunkRanges = useMemo(() => getHunkRanges(file.lines), [file.lines]);

  // ── Line range selection state ──
  const [selStart, setSelStart] = useState<number | null>(null);
  const [selEnd, setSelEnd] = useState<number | null>(null);
  const tableRef = useRef<HTMLTableElement>(null);

  const selMin =
    selStart != null && selEnd != null
      ? Math.min(selStart, selEnd)
      : null;
  const selMax =
    selStart != null && selEnd != null
      ? Math.max(selStart, selEnd)
      : null;

  const clearLineSelection = useCallback(() => {
    setSelStart(null);
    setSelEnd(null);
  }, []);

  // ── Text selection floating button ──
  const [textSelPos, setTextSelPos] = useState<{
    x: number;
    y: number;
  } | null>(null);
  const containerRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    function handleSelectionChange() {
      const sel = window.getSelection();
      if (!sel || sel.isCollapsed || !sel.rangeCount) {
        setTextSelPos(null);
        return;
      }

      // Only show if selection is inside our container
      const range = sel.getRangeAt(0);
      if (
        !containerRef.current?.contains(range.commonAncestorContainer)
      ) {
        setTextSelPos(null);
        return;
      }

      const rect = range.getBoundingClientRect();
      setTextSelPos({
        x: rect.left + rect.width / 2,
        y: rect.top - 4,
      });
    }

    document.addEventListener("selectionchange", handleSelectionChange);
    return () =>
      document.removeEventListener(
        "selectionchange",
        handleSelectionChange
      );
  }, []);

  // ── Handlers ──

  function addHunk(hunkIdx: number) {
    const range = hunkRanges[hunkIdx];
    if (!range) return;
    const hunkLines = file.lines.slice(range.start + 1, range.end + 1);
    const content = hunkLines
      .map((l) => {
        const prefix =
          l.type === "addition" ? "+" : l.type === "deletion" ? "-" : " ";
        return prefix + l.content;
      })
      .join("\n");
    const startLine = parseInt(lineLabel(hunkLines[0]) || "0");
    const endLine = parseInt(
      lineLabel(hunkLines[hunkLines.length - 1]) || "0"
    );
    onAddSnippet({
      file: file.file,
      startLine,
      endLine,
      content,
      type: "hunk",
    });
  }

  function addTextSelection() {
    const sel = window.getSelection();
    if (!sel || sel.isCollapsed) return;
    const text = sel.toString();
    if (!text.trim()) return;
    onAddSnippet({
      file: file.file,
      startLine: 0,
      endLine: 0,
      content: text,
      type: "text",
    });
    sel.removeAllRanges();
    setTextSelPos(null);
  }

  // Track whether the user dragged (multi-line) vs single-clicked
  const isDragging = useRef(false);

  function handleLineNumMouseDown(lineIdx: number, e: React.MouseEvent) {
    e.preventDefault();
    isDragging.current = false;
    if (e.shiftKey && selStart != null) {
      // Shift-click: extend and immediately capture
      setSelEnd(lineIdx);
      // We need to capture after state updates, so use a microtask
      queueMicrotask(() => addLineRangeFromTo(selStart, lineIdx));
    } else {
      setSelStart(lineIdx);
      setSelEnd(lineIdx);
    }
  }

  function handleLineNumMouseEnter(lineIdx: number, e: React.MouseEvent) {
    if (e.buttons === 1 && selStart != null) {
      isDragging.current = true;
      setSelEnd(lineIdx);
    }
  }

  function handleLineNumMouseUp() {
    if (selStart != null && selEnd != null) {
      addLineRangeFromTo(selStart, selEnd);
    }
    isDragging.current = false;
  }

  /** Capture a line range given raw start/end (handles ordering) */
  function addLineRangeFromTo(start: number, end: number) {
    const lo = Math.min(start, end);
    const hi = Math.max(start, end);
    const selectedLines = file.lines.filter(
      (l) => l.type !== "hunk-header" && l.index >= lo && l.index <= hi
    );
    if (selectedLines.length === 0) return;
    const content = selectedLines
      .map((l) => {
        const prefix =
          l.type === "addition" ? "+" : l.type === "deletion" ? "-" : " ";
        return prefix + l.content;
      })
      .join("\n");
    const startLine = parseInt(lineLabel(selectedLines[0]) || "0");
    const endLine = parseInt(
      lineLabel(selectedLines[selectedLines.length - 1]) || "0"
    );
    onAddSnippet({
      file: file.file,
      startLine,
      endLine,
      content,
      type: "lines",
    });
    clearLineSelection();
  }

  // Build highlight index map
  let highlightIdx = 0;

  return (
    <div ref={containerRef} style={{ position: "relative" }}>
      {(commitHash || multiFile) && (
        <div className="diff-header">{file.file}</div>
      )}

      <table className="diff-table" ref={tableRef}>
        <colgroup>
          <col style={{ width: 28 }} />
          <col style={{ width: 50 }} />
          <col style={{ width: 50 }} />
          <col style={{ width: 20 }} />
          <col />
        </colgroup>
        <tbody>
          {file.lines.map((line, li) => {
            if (line.type === "hunk-header") {
              const hunkIdx = hunkRanges.findIndex(
                (r) => r.start === line.index
              );
              return (
                <tr key={li} className="diff-hunk-header">
                  <td className="hunk-add-btn-cell">
                    <button
                      className="hunk-add-btn"
                      title="Add this hunk to story"
                      onClick={() => addHunk(hunkIdx)}
                    >
                      +
                    </button>
                  </td>
                  <td colSpan={4}>{line.content}</td>
                </tr>
              );
            }

            const tokens = highlighted?.[highlightIdx++];
            const sign =
              line.type === "addition"
                ? "+"
                : line.type === "deletion"
                  ? "-"
                  : " ";

            const isSelected =
              selMin != null &&
              selMax != null &&
              line.index >= selMin &&
              line.index <= selMax;

            return (
              <tr
                key={li}
                className={`diff-line ${line.type} ${isSelected ? "line-selected" : ""}`}
              >
                <td className="line-gutter" />
                <td
                  className="line-num"
                  onMouseDown={(e) => handleLineNumMouseDown(line.index, e)}
                  onMouseEnter={(e) => handleLineNumMouseEnter(line.index, e)}
                  onMouseUp={handleLineNumMouseUp}
                >
                  {line.oldNum ?? ""}
                </td>
                <td
                  className="line-num"
                  onMouseDown={(e) => handleLineNumMouseDown(line.index, e)}
                  onMouseEnter={(e) => handleLineNumMouseEnter(line.index, e)}
                  onMouseUp={handleLineNumMouseUp}
                >
                  {line.newNum ?? ""}
                </td>
                <td className="line-sign">{sign}</td>
                <td className="line-content">
                  {tokens
                    ? tokens.tokens.map((t, ti) => (
                        <span
                          key={ti}
                          style={
                            t.color ? { color: t.color } : undefined
                          }
                        >
                          {t.content}
                        </span>
                      ))
                    : line.content}
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>

      {/* Floating + button for text selections */}
      {textSelPos && (
        <button
          className="text-sel-add-btn"
          style={{
            position: "fixed",
            left: textSelPos.x,
            top: textSelPos.y,
            transform: "translate(-50%, -100%)",
          }}
          onMouseDown={(e) => {
            e.preventDefault(); // Don't lose the selection
            addTextSelection();
          }}
        >
          +
        </button>
      )}
    </div>
  );
}

// ── Main DiffViewer ──────────────────────────────────────────────────

export default function DiffViewer({
  diff,
  loading,
  filePath,
  commitHash,
  onAddSnippet,
}: DiffViewerProps) {
  const parsed = useMemo(() => (diff ? parseDiff(diff) : []), [diff]);

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

  return (
    <div className="diff-container">
      {parsed.map((file, fi) => (
        <HighlightedFile
          key={fi}
          file={file}
          commitHash={commitHash}
          multiFile={parsed.length > 1}
          onAddSnippet={onAddSnippet}
        />
      ))}
    </div>
  );
}
