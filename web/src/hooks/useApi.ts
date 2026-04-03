import { useState, useEffect } from "react";

export interface Meta {
  repo: string;
  branch: string;
  base: string;
}

export interface ChangedFile {
  path: string;
  status: "added" | "modified" | "deleted" | "renamed";
  additions: number;
  deletions: number;
}

export interface Commit {
  hash: string;
  shortHash: string;
  subject: string;
  author: string;
  date: string;
}

async function fetchJson<T>(url: string): Promise<T> {
  const res = await fetch(url);
  if (!res.ok) throw new Error(`${res.status} ${res.statusText}`);
  return res.json();
}

async function fetchText(url: string): Promise<string> {
  const res = await fetch(url);
  if (!res.ok) throw new Error(`${res.status} ${res.statusText}`);
  return res.text();
}

export function useMeta() {
  const [meta, setMeta] = useState<Meta | null>(null);
  useEffect(() => {
    fetchJson<Meta>("/api/meta").then(setMeta);
  }, []);
  return meta;
}

export function useChangedFiles() {
  const [files, setFiles] = useState<ChangedFile[]>([]);
  const [loading, setLoading] = useState(true);
  useEffect(() => {
    fetchJson<ChangedFile[]>("/api/files")
      .then(setFiles)
      .finally(() => setLoading(false));
  }, []);
  return { files, loading };
}

export function useCommits() {
  const [commits, setCommits] = useState<Commit[]>([]);
  useEffect(() => {
    fetchJson<Commit[]>("/api/commits").then(setCommits);
  }, []);
  return commits;
}

export function useFileDiff(path: string | null) {
  const [diff, setDiff] = useState<string>("");
  const [loading, setLoading] = useState(false);

  useEffect(() => {
    if (!path) {
      setDiff("");
      return;
    }
    setLoading(true);
    fetchText(`/api/diff/${path}`)
      .then(setDiff)
      .finally(() => setLoading(false));
  }, [path]);

  return { diff, loading };
}

export function useCommitDiff(hash: string | null) {
  const [diff, setDiff] = useState<string>("");
  const [loading, setLoading] = useState(false);

  useEffect(() => {
    if (!hash) {
      setDiff("");
      return;
    }
    setLoading(true);
    fetchText(`/api/commits/${hash}/diff`)
      .then(setDiff)
      .finally(() => setLoading(false));
  }, [hash]);

  return { diff, loading };
}

export interface FileLinesResult {
  lines: string[];
  total: number;
  start: number;
  end: number;
}

export async function fetchFileLines(
  ref: string,
  path: string,
  start: number,
  end: number
): Promise<FileLinesResult> {
  return fetchJson<FileLinesResult>(
    `/api/lines/${ref}/${path}?start=${start}&end=${end}`
  );
}

/** A diff line with its prefix and new-side line number */
export interface DiffLineEntry {
  prefix: "+" | "-" | " ";
  content: string;
  newLineNum: number | null; // null for deletions
}

const diffLineCache = new Map<string, DiffLineEntry[]>();

/**
 * Fetch and parse a file's diff into an ordered list of diff lines.
 * Cached so multiple calls for the same file are free.
 */
export async function fetchParsedDiffLines(
  path: string
): Promise<DiffLineEntry[]> {
  const cached = diffLineCache.get(path);
  if (cached) return cached;

  const raw = await fetchText(`/api/diff/${path}`);
  const entries: DiffLineEntry[] = [];
  let newLine = 0;

  for (const line of raw.split("\n")) {
    if (line.startsWith("@@")) {
      const match = line.match(/@@ -\d+(?:,\d+)? \+(\d+)(?:,\d+)? @@/);
      if (match) newLine = parseInt(match[1]);
      continue;
    }
    if (
      line.startsWith("diff ") || line.startsWith("index ") ||
      line.startsWith("---") || line.startsWith("+++") ||
      line.startsWith("new file") || line.startsWith("deleted file") ||
      line.startsWith("similarity") || line.startsWith("rename") ||
      line.startsWith("Binary")
    ) continue;

    if (line.startsWith("+")) {
      entries.push({ prefix: "+", content: line.slice(1), newLineNum: newLine++ });
    } else if (line.startsWith("-")) {
      entries.push({ prefix: "-", content: line.slice(1), newLineNum: null });
    } else if (line.startsWith(" ") || line === "") {
      entries.push({ prefix: " ", content: line.startsWith(" ") ? line.slice(1) : "", newLineNum: newLine++ });
    }
  }

  diffLineCache.set(path, entries);
  return entries;
}
