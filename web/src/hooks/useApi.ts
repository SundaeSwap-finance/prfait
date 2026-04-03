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
