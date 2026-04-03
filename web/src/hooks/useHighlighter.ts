import { useEffect, useState } from "react";
import { createHighlighter, type Highlighter } from "shiki";

const LANG_MAP: Record<string, string> = {
  ts: "typescript",
  tsx: "tsx",
  js: "javascript",
  jsx: "jsx",
  rs: "rust",
  py: "python",
  go: "go",
  rb: "ruby",
  java: "java",
  c: "c",
  h: "c",
  cpp: "cpp",
  cs: "csharp",
  sh: "bash",
  bash: "bash",
  zsh: "bash",
  fish: "bash",
  json: "json",
  yaml: "yaml",
  yml: "yaml",
  toml: "toml",
  md: "markdown",
  css: "css",
  scss: "scss",
  html: "html",
  sql: "sql",
  hs: "haskell",
  nix: "nix",
};

function langFromPath(path: string): string {
  const ext = path.split(".").pop() ?? "";
  return LANG_MAP[ext] ?? "";
}

// Singleton highlighter — created once, languages loaded on demand
let highlighterPromise: Promise<Highlighter> | null = null;
const loadedLangs = new Set<string>();

async function getHighlighter(): Promise<Highlighter> {
  if (!highlighterPromise) {
    highlighterPromise = createHighlighter({
      themes: ["github-dark-default"],
      langs: [],
    });
  }
  return highlighterPromise;
}

async function ensureLang(
  h: Highlighter,
  lang: string
): Promise<string | null> {
  if (!lang) return null;
  if (loadedLangs.has(lang)) return lang;
  try {
    await h.loadLanguage(lang as any);
    loadedLangs.add(lang);
    return lang;
  } catch {
    return null;
  }
}

export interface HighlightedLine {
  tokens: { content: string; color?: string }[];
}

export function useHighlightedLines(
  code: string,
  filePath: string
): HighlightedLine[] | null {
  const [result, setResult] = useState<HighlightedLine[] | null>(null);

  useEffect(() => {
    if (!code || !filePath) {
      setResult(null);
      return;
    }

    let cancelled = false;

    (async () => {
      const h = await getHighlighter();
      const lang = await ensureLang(h, langFromPath(filePath));

      if (cancelled || !lang) return;

      const tokens = h.codeToTokens(code, {
        lang,
        theme: "github-dark-default",
      });

      if (cancelled) return;

      setResult(
        tokens.tokens.map((lineTokens) => ({
          tokens: lineTokens.map((t) => ({
            content: t.content,
            color: t.color,
          })),
        }))
      );
    })();

    return () => {
      cancelled = true;
    };
  }, [code, filePath]);

  return result;
}
