import { useState, useCallback, useEffect, useRef } from "react";

export type Panel = SnippetPanel | NarrationPanel;

export interface SnippetComment {
  /** Line index (in the snippet's content lines) this comment is attached after */
  afterLine: number;
  markdown: string;
}

export interface SnippetPanel {
  id: string;
  kind: "snippet";
  file: string;
  startLine: number;
  endLine: number;
  content: string;
  captureType: "hunk" | "lines" | "text";
  /** Inline comments attached to specific lines */
  comments?: SnippetComment[];
}

export interface NarrationPanel {
  id: string;
  kind: "narration";
  markdown: string;
}

export interface Snippet {
  id: string;
  file: string;
  startLine: number;
  endLine: number;
  content: string;
  type: "hunk" | "lines" | "text";
}

/** The shape we persist to disk */
interface StoryDocument {
  version: 1;
  summary: string;
  panels: Panel[];
  updatedAt: string;
}

let nextId = 1;
function genId() {
  return "p" + String(nextId++);
}

/** Ensure IDs don't collide after loading saved panels */
function syncIdCounter(panels: Panel[]) {
  for (const p of panels) {
    const num = parseInt(p.id.replace(/^p/, ""));
    if (!isNaN(num) && num >= nextId) {
      nextId = num + 1;
    }
  }
}

async function loadStory(): Promise<{ summary: string; panels: Panel[] }> {
  try {
    const res = await fetch("/api/story");
    const data: StoryDocument | null = await res.json();
    if (data) {
      if (data.panels?.length) syncIdCounter(data.panels);
      return { summary: data.summary ?? "", panels: data.panels ?? [] };
    }
  } catch { /* no saved story */ }
  return { summary: "", panels: [] };
}

async function saveStory(summary: string, panels: Panel[]) {
  const doc: StoryDocument = {
    version: 1,
    summary,
    panels,
    updatedAt: new Date().toISOString(),
  };
  await fetch("/api/story", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(doc),
  });
}

export function useStory() {
  const [summary, setSummary] = useState("");
  const [panels, setPanels] = useState<Panel[]>([]);
  const [loaded, setLoaded] = useState(false);

  // Load on mount
  useEffect(() => {
    loadStory().then(({ summary: s, panels: p }) => {
      setSummary(s);
      setPanels(p);
      setLoaded(true);
    });
  }, []);

  // Auto-save with debounce
  const saveTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(() => {
    if (!loaded) return;
    if (saveTimer.current) clearTimeout(saveTimer.current);
    saveTimer.current = setTimeout(() => {
      saveStory(summary, panels);
    }, 500);
    return () => {
      if (saveTimer.current) clearTimeout(saveTimer.current);
    };
  }, [summary, panels, loaded]);

  const addSnippet = useCallback(
    (snippet: Omit<Snippet, "id">) => {
      const panel: SnippetPanel = {
        id: genId(),
        kind: "snippet",
        file: snippet.file,
        startLine: snippet.startLine,
        endLine: snippet.endLine,
        content: snippet.content,
        originalContent: snippet.content,
        captureType: snippet.type,
      };
      setPanels((prev) => [...prev, panel]);
    },
    []
  );

  const addSnippetAt = useCallback(
    (snippet: Omit<Snippet, "id">, atIndex: number) => {
      const panel: SnippetPanel = {
        id: genId(),
        kind: "snippet",
        file: snippet.file,
        startLine: snippet.startLine,
        endLine: snippet.endLine,
        content: snippet.content,
        originalContent: snippet.content,
        captureType: snippet.type,
      };
      setPanels((prev) => {
        const next = [...prev];
        next.splice(Math.max(0, Math.min(atIndex, next.length)), 0, panel);
        return next;
      });
    },
    []
  );

  const addNarration = useCallback(
    (atIndex: number) => {
      const panel: NarrationPanel = {
        id: genId(),
        kind: "narration",
        markdown: "",
      };
      setPanels((prev) => {
        const next = [...prev];
        const idx = Math.max(0, Math.min(atIndex, next.length));
        next.splice(idx, 0, panel);
        return next;
      });
      return panel.id;
    },
    []
  );

  const updateNarration = useCallback(
    (id: string, markdown: string) => {
      setPanels((prev) =>
        prev.map((p) =>
          p.id === id && p.kind === "narration" ? { ...p, markdown } : p
        )
      );
    },
    []
  );

  /** Update snippet range + content (for resize). */
  const updateSnippet = useCallback(
    (id: string, update: { startLine: number; endLine: number; content: string }) => {
      setPanels((prev) =>
        prev.map((p) =>
          p.id === id && p.kind === "snippet"
            ? { ...p, ...update }
            : p
        )
      );
    },
    []
  );

  /** Add or update an inline comment on a snippet */
  const setSnippetComment = useCallback(
    (id: string, afterLine: number, markdown: string) => {
      setPanels((prev) =>
        prev.map((p) => {
          if (p.id !== id || p.kind !== "snippet") return p;
          const comments = [...(p.comments ?? [])];
          const existing = comments.findIndex((c) => c.afterLine === afterLine);
          if (markdown === "") {
            // Remove comment
            if (existing >= 0) comments.splice(existing, 1);
          } else if (existing >= 0) {
            comments[existing] = { afterLine, markdown };
          } else {
            comments.push({ afterLine, markdown });
            comments.sort((a, b) => a.afterLine - b.afterLine);
          }
          return { ...p, comments };
        })
      );
    },
    []
  );

  const removePanel = useCallback((id: string) => {
    setPanels((prev) => prev.filter((p) => p.id !== id));
  }, []);

  const reorderPanels = useCallback(
    (fromIdx: number, toIdx: number) => {
      setPanels((prev) => {
        if (fromIdx === toIdx) return prev;
        const next = [...prev];
        const [moved] = next.splice(fromIdx, 1);
        next.splice(toIdx, 0, moved);
        return next;
      });
    },
    []
  );

  return {
    summary,
    setSummary,
    panels,
    loaded,
    addSnippet,
    addSnippetAt,
    addNarration,
    updateNarration,
    updateSnippet,
    setSnippetComment,
    removePanel,
    reorderPanels,
  };
}
