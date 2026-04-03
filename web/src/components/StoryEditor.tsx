import React, { useState, useRef, useCallback, useEffect, useMemo } from "react";
import type { Panel, SnippetPanel, NarrationPanel } from "../hooks/useStory";
import type { ChangedFile } from "../hooks/useApi";
import { useHighlightedLines } from "../hooks/useHighlighter";
import MarkdownEditor from "./MarkdownEditor";
import { fetchParsedDiffLines, type DiffLineEntry } from "../hooks/useApi";

interface StoryEditorProps {
  summary: string;
  onUpdateSummary: (markdown: string) => void;
  panels: Panel[];
  files: ChangedFile[];
  onAddNarration: (atIndex: number) => string;
  onUpdateNarration: (id: string, markdown: string) => void;
  onUpdateSnippet: (id: string, update: { startLine: number; endLine: number; content: string }) => void;
  onSetSnippetComment: (id: string, afterLine: number, markdown: string) => void;
  onRemovePanel: (id: string) => void;
  onReorderPanels: (fromIdx: number, toIdx: number) => void;
  onRequestCapture: (insertAtIndex: number, filterFile?: string, sourceNarrationId?: string) => void;
}

// ── Drag reorder ────────────────────────────────────────────────────

function useDragReorder(onReorder: (from: number, to: number) => void) {
  const [dragIdx, setDragIdx] = useState<number | null>(null);
  const [overIdx, setOverIdx] = useState<number | null>(null);

  const handleDragStart = useCallback(
    (idx: number) => (e: React.DragEvent) => {
      setDragIdx(idx);
      e.dataTransfer.effectAllowed = "move";
      // The drag handle is inside the wrapper — use the wrapper as the drag image
      const wrapper = (e.currentTarget as HTMLElement).closest(".story-panel-wrapper") as HTMLElement | null;
      if (wrapper) {
        const rect = wrapper.getBoundingClientRect();
        e.dataTransfer.setDragImage(
          wrapper,
          e.clientX - rect.left,
          e.clientY - rect.top
        );
      }
    },
    []
  );

  const handleDragOver = useCallback(
    (idx: number) => (e: React.DragEvent) => {
      e.preventDefault();
      e.dataTransfer.dropEffect = "move";
      setOverIdx(idx);
    },
    []
  );

  const handleDrop = useCallback(
    (idx: number) => (e: React.DragEvent) => {
      e.preventDefault();
      if (dragIdx != null && dragIdx !== idx) {
        onReorder(dragIdx, idx);
      }
      setDragIdx(null);
      setOverIdx(null);
    },
    [dragIdx, onReorder]
  );

  const handleDragEnd = useCallback(() => {
    setDragIdx(null);
    setOverIdx(null);
  }, []);

  return { dragIdx, overIdx, handleDragStart, handleDragOver, handleDrop, handleDragEnd };
}

// ── @ command autocomplete ──────────────────────────────────────────

function AtMenu({
  query,
  files,
  position,
  onSelect,
  onDismiss,
}: {
  query: string;
  files: ChangedFile[];
  position: { top: number; left: number };
  onSelect: (file?: string) => void;
  onDismiss: () => void;
}) {
  const [selectedIdx, setSelectedIdx] = useState(0);

  const filtered = useMemo(() => {
    if (!query) return [{ label: "Browse all diffs", value: undefined }, ...files.map(f => ({ label: f.path, value: f.path }))];
    const q = query.toLowerCase();
    const matches = files
      .filter((f) => f.path.toLowerCase().includes(q))
      .map((f) => ({ label: f.path, value: f.path }));
    if (matches.length === 0) return [{ label: "Browse all diffs", value: undefined }];
    return matches;
  }, [query, files]);

  useEffect(() => {
    setSelectedIdx(0);
  }, [filtered.length]);

  useEffect(() => {
    function handleKey(e: KeyboardEvent) {
      if (e.key === "ArrowDown") {
        e.preventDefault();
        setSelectedIdx((i) => Math.min(i + 1, filtered.length - 1));
      } else if (e.key === "ArrowUp") {
        e.preventDefault();
        setSelectedIdx((i) => Math.max(i - 1, 0));
      } else if (e.key === "Enter") {
        e.preventDefault();
        onSelect(filtered[selectedIdx]?.value);
      } else if (e.key === "Escape") {
        e.preventDefault();
        onDismiss();
      }
    }
    window.addEventListener("keydown", handleKey);
    return () => window.removeEventListener("keydown", handleKey);
  }, [filtered, selectedIdx, onSelect, onDismiss]);

  return (
    <div className="at-menu" style={{ top: position.top, left: position.left }}>
      {filtered.map((item, i) => (
        <div
          key={item.label}
          className={`at-menu-item ${i === selectedIdx ? "active" : ""}`}
          onMouseDown={(e) => {
            e.preventDefault();
            onSelect(item.value);
          }}
          onMouseEnter={() => setSelectedIdx(i)}
        >
          {item.value ? (
            <span className="at-menu-file">{item.label}</span>
          ) : (
            <span className="at-menu-browse">{item.label}</span>
          )}
        </div>
      ))}
    </div>
  );
}

// ── Narration editor ────────────────────────────────────────────────

function NarrationEditor({
  panel,
  panelIndex,
  files,
  onUpdate,
  onRemove,
  onRequestCapture,
  autoFocus,
}: {
  panel: NarrationPanel;
  panelIndex: number;
  files: ChangedFile[];
  onUpdate: (markdown: string) => void;
  onRemove: () => void;
  onRequestCapture: (insertAtIndex: number, filterFile?: string, sourceNarrationId?: string) => void;
  autoFocus: boolean;
}) {
  const [atMenu, setAtMenu] = useState<{ query: string; position: { top: number; left: number } } | null>(null);
  const editorWrapperRef = useRef<HTMLDivElement>(null);

  // Watch for @ being typed in the editor
  function handleEditorKeyDown(e: React.KeyboardEvent) {
    if (e.key === "@" && !atMenu) {
      requestAnimationFrame(() => {
        const wrapper = editorWrapperRef.current;
        if (!wrapper) return;
        // Find the cursor position via the selection
        const sel = window.getSelection();
        if (sel && sel.rangeCount > 0) {
          const range = sel.getRangeAt(0);
          const rect = range.getBoundingClientRect();
          setAtMenu({
            query: "",
            position: {
              top: rect.bottom + 4,
              left: rect.left,
            },
          });
        }
      });
      return;
    }

    if (atMenu) {
      if (e.key === "Escape") {
        e.preventDefault();
        setAtMenu(null);
        return;
      }
      // Update query by looking at text after @ in the current markdown
      requestAnimationFrame(() => {
        // Get text content near cursor
        const sel = window.getSelection();
        if (!sel || !sel.focusNode) return;
        const text = sel.focusNode.textContent ?? "";
        const pos = sel.focusOffset;
        const atIdx = text.lastIndexOf("@", pos - 1);
        if (atIdx >= 0) {
          setAtMenu((prev) =>
            prev ? { ...prev, query: text.slice(atIdx + 1, pos) } : null
          );
        } else {
          setAtMenu(null);
        }
      });
    }
  }

  function handleAtSelect(file?: string) {
    // Remove the @query from the current content
    const md = panel.markdown;
    const atIdx = md.lastIndexOf("@");
    if (atIdx >= 0) {
      onUpdate(md.slice(0, atIdx) + md.slice(md.indexOf(" ", atIdx + 1) >= 0 ? md.indexOf(" ", atIdx + 1) : md.length));
    }
    setAtMenu(null);
    onRequestCapture(panelIndex + 1, file, panel.id);
  }

  function handleAtDismiss() {
    setAtMenu(null);
  }

  return (
    <div className="story-panel narration-panel" style={{ position: "relative" }} ref={editorWrapperRef}>
      <MarkdownEditor
        value={panel.markdown}
        onChange={onUpdate}
        onKeyDown={handleEditorKeyDown}
        placeholder="Write narration... (type @ to insert a snippet)"
        autoFocus={autoFocus}
      />
      {panel.markdown === "" && (
        <button className="panel-remove" onClick={onRemove} title="Remove">
          ×
        </button>
      )}
      {atMenu && (
        <AtMenu
          query={atMenu.query}
          files={files}
          position={atMenu.position}
          onSelect={handleAtSelect}
          onDismiss={handleAtDismiss}
        />
      )}
    </div>
  );
}

// ── Inline comment on a snippet line ─────────────────────────────────

function InlineComment({
  comment,
  onChange,
  onRemove,
  autoFocus,
}: {
  comment: string;
  onChange: (markdown: string) => void;
  onRemove: () => void;
  autoFocus: boolean;
}) {
  return (
    <div className="snippet-inline-comment">
      <div className="snippet-inline-comment-bar" />
      <div className="snippet-inline-comment-body">
        <MarkdownEditor
          value={comment}
          onChange={onChange}
          placeholder="Add a comment..."
          autoFocus={autoFocus}
        />
        {comment === "" && (
          <button className="snippet-inline-comment-remove" onClick={onRemove}>×</button>
        )}
      </div>
    </div>
  );
}

// ── Snippet card with resize handles ────────────────────────────────

/**
 * Resize works by fetching the full parsed diff for the file on drag start,
 * finding where the snippet's lines sit within it, and sliding a window
 * over those real diff lines as you drag. On commit, the new window
 * becomes the snippet content — with correct +/-/space prefixes.
 */

interface ResizeState {
  allLines: string[];
  startIdx: number;
  endIdx: number;
  lineDelta: number;
  direction: "top" | "bottom";
  /** Raw pixel offset from drag start — used to shift the panel during top-drag */
  pixelDelta: number;
}

function SnippetCard({
  panel,
  onRemove,
  onUpdate,
  onSetComment,
}: {
  panel: SnippetPanel;
  onRemove: () => void;
  onUpdate: (update: { startLine: number; endLine: number; content: string }) => void;
  onSetComment: (afterLine: number, markdown: string) => void;
}) {
  const [collapsed, setCollapsed] = useState(false);
  const [newCommentLine, setNewCommentLine] = useState<number | null>(null);
  const [resize, setResize] = useState<ResizeState | null>(null);
  const resizeRef = useRef<ResizeState | null>(null);
  resizeRef.current = resize;

  const lineRange =
    panel.startLine > 0
      ? panel.startLine === panel.endLine
        ? `L${panel.startLine}`
        : `L${panel.startLine}–${panel.endLine}`
      : "";

  const rawLines = panel.content.split("\n");

  // During resize, compute which lines from allLines to show
  const displayLines: string[] = useMemo(() => {
    if (!resize) return rawLines;
    const { allLines, startIdx, endIdx, lineDelta, direction } = resize;
    let newStart = startIdx;
    let newEnd = endIdx;
    if (direction === "top") {
      newStart = Math.max(0, Math.min(startIdx + lineDelta, endIdx));
    } else {
      newEnd = Math.max(startIdx, Math.min(endIdx + lineDelta, allLines.length - 1));
    }
    return allLines.slice(newStart, newEnd + 1);
  }, [rawLines, resize]);

  // Syntax highlighting
  const codeForHighlight = useMemo(() => {
    return displayLines
      .map((l) => {
        if (l.startsWith("+") || l.startsWith("-") || l.startsWith(" "))
          return l.slice(1);
        return l;
      })
      .join("\n");
  }, [displayLines]);

  const highlighted = useHighlightedLines(codeForHighlight, panel.file);

  const panelRef = useRef(panel);
  panelRef.current = panel;
  const topHandleRef = useRef<HTMLDivElement>(null);
  const bottomHandleRef = useRef<HTMLDivElement>(null);

  function commitResize() {
    const rs = resizeRef.current;
    if (!rs || rs.lineDelta === 0) {
      setResize(null);
      return;
    }

    const { allLines, startIdx, endIdx, lineDelta, direction } = rs;
    let newStart = startIdx;
    let newEnd = endIdx;
    if (direction === "top") {
      newStart = Math.max(0, Math.min(startIdx + lineDelta, endIdx));
    } else {
      newEnd = Math.max(startIdx, Math.min(endIdx + lineDelta, allLines.length - 1));
    }

    const newContent = allLines.slice(newStart, newEnd + 1).join("\n");

    // Compute new line numbers from the diff entries
    const firstLine = allLines[newStart];
    const lastLine = allLines[newEnd];
    // For line numbers, count from the original snippet and adjust
    const p = panelRef.current;
    const topShift = newStart - startIdx; // negative = expanded up
    const bottomShift = newEnd - endIdx;  // positive = expanded down

    onUpdate({
      startLine: p.startLine + topShift,
      endLine: p.endLine + bottomShift,
      content: newContent,
    });

    setResize(null);
  }

  function useResizeHandle(
    ref: React.RefObject<HTMLDivElement | null>,
    direction: "top" | "bottom"
  ) {
    useEffect(() => {
      const el = ref.current;
      if (!el) return;

      let startY = 0;
      const LINE_HEIGHT = 18;

      async function onMouseDown(e: MouseEvent) {
        e.preventDefault();
        startY = e.clientY;

        const p = panelRef.current;
        if (p.startLine <= 0) return;

        // Fetch the full diff for this file
        const diffEntries = await fetchParsedDiffLines(p.file);

        // Convert to prefixed strings (same format as snippet content)
        const allLines = diffEntries.map(
          (e) => e.prefix + e.content
        );

        // Find where the current snippet content lives in allLines
        // Match by finding the run of lines that equals our content
        const snippetLines = p.content.split("\n");
        let startIdx = -1;

        outer: for (let i = 0; i <= allLines.length - snippetLines.length; i++) {
          for (let j = 0; j < snippetLines.length; j++) {
            if (allLines[i + j] !== snippetLines[j]) continue outer;
          }
          startIdx = i;
          break;
        }

        if (startIdx === -1) {
          // Couldn't find snippet in diff — fall back to no-op
          return;
        }

        const endIdx = startIdx + snippetLines.length - 1;

        setResize({ allLines, startIdx, endIdx, lineDelta: 0, direction, pixelDelta: 0 });

        document.addEventListener("mousemove", onMouseMove);
        document.addEventListener("mouseup", onMouseUp);
        document.body.style.cursor =
          direction === "top" ? "n-resize" : "s-resize";
        document.body.style.userSelect = "none";
      }

      function onMouseMove(e: MouseEvent) {
        const deltaPixels = e.clientY - startY;
        const newDelta = Math.round(deltaPixels / LINE_HEIGHT);
        setResize((prev) =>
          prev ? { ...prev, lineDelta: newDelta, pixelDelta: deltaPixels } : null
        );
      }

      function onMouseUp() {
        document.removeEventListener("mousemove", onMouseMove);
        document.removeEventListener("mouseup", onMouseUp);
        document.body.style.cursor = "";
        document.body.style.userSelect = "";
        commitResize();
      }

      el.addEventListener("mousedown", onMouseDown);
      return () => el.removeEventListener("mousedown", onMouseDown);
    });
  }

  useResizeHandle(topHandleRef, "top");
  useResizeHandle(bottomHandleRef, "bottom");

  // When dragging the top handle, shift the panel so the handle tracks the mouse.
  // On mouseup (resize=null), the margin snaps back and the committed content fills in.
  const dragStyle: React.CSSProperties | undefined =
    resize?.direction === "top"
      ? { marginTop: resize.pixelDelta }
      : undefined;

  return (
    <div
      className={`story-panel snippet-panel ${resize ? "snippet-resizing" : ""}`}
      style={dragStyle}
    >
      <div className="snippet-panel-header">
        <button
          className="panel-collapse"
          onClick={() => setCollapsed(!collapsed)}
        >
          {collapsed ? "▸" : "▾"}
        </button>
        <span className="snippet-panel-file">{panel.file}</span>
        {lineRange && (
          <span className="snippet-panel-lines">{lineRange}</span>
        )}
        <button className="panel-remove" onClick={onRemove} title="Remove">
          ×
        </button>
      </div>
      {!collapsed && (
        <>
          {panel.startLine > 0 && (
            <div
              ref={topHandleRef}
              className="snippet-resize-handle snippet-resize-top"
            />
          )}
          <div className="snippet-panel-code">
            {displayLines.map((line, i) => {
              const prefix = line[0] ?? " ";
              const lineClass =
                prefix === "+"
                  ? "snippet-line-add"
                  : prefix === "-"
                    ? "snippet-line-del"
                    : "snippet-line-ctx";

              const tokens = highlighted?.[i];
              const comment = panel.comments?.find((c) => c.afterLine === i);
              const isNewComment = newCommentLine === i && !comment;

              return (
                <React.Fragment key={i}>
                  <div
                    className={`snippet-line ${lineClass} snippet-line-commentable`}
                    onClick={() => {
                      if (!comment && newCommentLine !== i) {
                        setNewCommentLine(i);
                      }
                    }}
                  >
                    <span className="snippet-line-prefix">
                      {prefix === "+" ? "+" : prefix === "-" ? "-" : " "}
                    </span>
                    <span className="snippet-line-content">
                      {tokens
                        ? tokens.tokens.map((t, ti) => (
                            <span
                              key={ti}
                              style={t.color ? { color: t.color } : undefined}
                            >
                              {t.content}
                            </span>
                          ))
                        : line.startsWith("+") || line.startsWith("-") || line.startsWith(" ")
                          ? line.slice(1)
                          : line}
                    </span>
                    <span className="snippet-line-comment-hint">+</span>
                  </div>
                  {(comment || isNewComment) && (
                    <InlineComment
                      comment={comment?.markdown ?? ""}
                      onChange={(md) => onSetComment(i, md)}
                      onRemove={() => {
                        onSetComment(i, "");
                        setNewCommentLine(null);
                      }}
                      autoFocus={isNewComment}
                    />
                  )}
                </React.Fragment>
              );
            })}
          </div>
          {panel.startLine > 0 && (
            <div
              ref={bottomHandleRef}
              className="snippet-resize-handle snippet-resize-bottom"
            />
          )}
        </>
      )}
    </div>
  );
}

// ── Gap zone ────────────────────────────────────────────────────────

function GapZone({ onClick, isFirst }: { onClick: () => void; isFirst?: boolean }) {
  return (
    <div
      className={`gap-zone ${isFirst ? "gap-zone-first" : ""}`}
      onClick={onClick}
    />
  );
}

// ── Main StoryEditor ────────────────────────────────────────────────

export default function StoryEditor({
  summary,
  onUpdateSummary,
  panels,
  files,
  onAddNarration,
  onUpdateNarration,
  onUpdateSnippet,
  onSetSnippetComment,
  onRemovePanel,
  onReorderPanels,
  onRequestCapture,
}: StoryEditorProps) {
  const [newNarrationId, setNewNarrationId] = useState<string | null>(null);
  const { dragIdx, overIdx, handleDragStart, handleDragOver, handleDrop, handleDragEnd } =
    useDragReorder(onReorderPanels);

  function insertNarration(atIndex: number) {
    const id = onAddNarration(atIndex);
    setNewNarrationId(id);
  }

  function handleCanvasClick(e: React.MouseEvent<HTMLDivElement>) {
    if (e.target === e.currentTarget) {
      insertNarration(panels.length);
    }
  }

  return (
    <div className="story-editor">
      <div className="story-editor-canvas" onClick={handleCanvasClick}>
        {/* Summary — renders into the PR description */}
        <div className="story-summary">
          <div className="story-summary-label">PR Summary</div>
          <div className="story-summary-hint">
            This appears in the PR description. The walkthrough below links from it.
          </div>
          <MarkdownEditor
            value={summary}
            onChange={onUpdateSummary}
            placeholder="What does this PR do, and why? What's notable about the approach?"
          />
        </div>

        <div className="story-walkthrough-label">
          Walkthrough
          <span className="story-walkthrough-hint">Detailed context for reviewers — hosted on the prfait viewer</span>
        </div>

        {panels.length === 0 && (
          <div className="story-empty" onClick={() => insertNarration(0)}>
            <p>Click to start writing, or grab snippets from the diff view.</p>
            <p style={{ fontSize: 12, marginTop: 4 }}>
              Type <kbd>@</kbd> in narration to insert a snippet inline.
            </p>
          </div>
        )}

        {panels.length > 0 && <GapZone isFirst onClick={() => insertNarration(0)} />}

        {panels.map((panel, idx) => {
          const isDragged = dragIdx === idx;
          const isOver = overIdx === idx && dragIdx !== idx;

          return (
            <div key={panel.id}>
              <div
                className={`story-panel-wrapper ${isDragged ? "dragging" : ""} ${isOver ? "drag-over" : ""}`}
                onDragOver={handleDragOver(idx)}
                onDrop={handleDrop(idx)}
                onDragEnd={handleDragEnd}
              >
                <div
                  className="panel-drag-handle"
                  title="Drag to reorder"
                  draggable
                  onDragStart={handleDragStart(idx)}
                >⠿</div>

                {panel.kind === "narration" ? (
                  <NarrationEditor
                    panel={panel}
                    panelIndex={idx}
                    files={files}
                    onUpdate={(md) => onUpdateNarration(panel.id, md)}
                    onRemove={() => onRemovePanel(panel.id)}
                    onRequestCapture={onRequestCapture}
                    autoFocus={panel.id === newNarrationId}
                  />
                ) : (
                  <SnippetCard
                    panel={panel}
                    onRemove={() => onRemovePanel(panel.id)}
                    onUpdate={(update) => onUpdateSnippet(panel.id, update)}
                    onSetComment={(afterLine, md) => onSetSnippetComment(panel.id, afterLine, md)}
                  />
                )}
              </div>

              <GapZone onClick={() => insertNarration(idx + 1)} />
            </div>
          );
        })}
      </div>
    </div>
  );
}
