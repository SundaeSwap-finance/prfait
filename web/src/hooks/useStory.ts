import { useState, useCallback } from "react";

export interface Snippet {
  id: string;
  file: string;
  startLine: number;
  endLine: number;
  content: string;
  type: "hunk" | "lines" | "text";
}

let nextId = 1;

export function useStory() {
  const [snippets, setSnippets] = useState<Snippet[]>([]);

  const addSnippet = useCallback((snippet: Omit<Snippet, "id">) => {
    setSnippets((prev) => [...prev, { ...snippet, id: String(nextId++) }]);
  }, []);

  const removeSnippet = useCallback((id: string) => {
    setSnippets((prev) => prev.filter((s) => s.id !== id));
  }, []);

  return { snippets, addSnippet, removeSnippet };
}
