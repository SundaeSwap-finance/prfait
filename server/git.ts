import { $ } from "bun";

export interface ChangedFile {
  path: string;
  status: "added" | "modified" | "deleted" | "renamed";
  additions: number;
  deletions: number;
}

export interface FileContent {
  path: string;
  content: string;
  language: string;
}

export interface Commit {
  hash: string;
  shortHash: string;
  subject: string;
  author: string;
  date: string;
}

const LANG_MAP: Record<string, string> = {
  ts: "typescript",
  tsx: "typescript",
  js: "javascript",
  jsx: "javascript",
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
  fish: "fish",
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
  ex: "elixir",
  exs: "elixir",
  nix: "nix",
};

function langFromPath(path: string): string {
  const ext = path.split(".").pop() ?? "";
  return LANG_MAP[ext] ?? ext;
}

function statusChar(s: string): ChangedFile["status"] {
  if (s.startsWith("A")) return "added";
  if (s.startsWith("D")) return "deleted";
  if (s.startsWith("R")) return "renamed";
  return "modified";
}

export async function getRepoRoot(cwd: string): Promise<string> {
  const result = await $`git -C ${cwd} rev-parse --show-toplevel`.text();
  return result.trim();
}

export async function getMergeBase(
  cwd: string,
  base: string
): Promise<string> {
  const result =
    await $`git -C ${cwd} merge-base ${base} HEAD`.text();
  return result.trim();
}

export async function getChangedFiles(
  cwd: string,
  base: string
): Promise<ChangedFile[]> {
  const mergeBase = await getMergeBase(cwd, base);
  const result =
    await $`git -C ${cwd} diff --numstat --diff-filter=ADMR ${mergeBase} HEAD`.text();

  const nameStatus =
    await $`git -C ${cwd} diff --name-status --diff-filter=ADMR ${mergeBase} HEAD`.text();

  const statuses = new Map<string, string>();
  for (const line of nameStatus.trim().split("\n")) {
    if (!line) continue;
    const parts = line.split("\t");
    const status = parts[0];
    const file = parts.length === 3 ? parts[2] : parts[1]; // renamed: old\tnew
    statuses.set(file, status);
  }

  const files: ChangedFile[] = [];
  for (const line of result.trim().split("\n")) {
    if (!line) continue;
    const [add, del, ...pathParts] = line.split("\t");
    const path = pathParts.join("\t"); // handle renames with =>
    files.push({
      path,
      status: statusChar(statuses.get(path) ?? "M"),
      additions: parseInt(add) || 0,
      deletions: parseInt(del) || 0,
    });
  }

  return files;
}

export async function getFileDiff(
  cwd: string,
  base: string,
  file: string
): Promise<string> {
  const mergeBase = await getMergeBase(cwd, base);
  const result =
    await $`git -C ${cwd} diff -U8 ${mergeBase} HEAD -- ${file}`.text();
  return result;
}

export async function getFullDiff(
  cwd: string,
  base: string
): Promise<string> {
  const mergeBase = await getMergeBase(cwd, base);
  const result =
    await $`git -C ${cwd} diff -U8 ${mergeBase} HEAD`.text();
  return result;
}

export async function getFileContent(
  cwd: string,
  ref: string,
  path: string
): Promise<FileContent> {
  const content = await $`git -C ${cwd} show ${ref}:${path}`.text();
  return { path, content, language: langFromPath(path) };
}

export async function getCommits(
  cwd: string,
  base: string
): Promise<Commit[]> {
  const mergeBase = await getMergeBase(cwd, base);
  const result =
    await $`git -C ${cwd} log --format=%H%x00%h%x00%s%x00%an%x00%aI ${mergeBase}..HEAD`.text();

  return result
    .trim()
    .split("\n")
    .filter(Boolean)
    .map((line) => {
      const [hash, shortHash, subject, author, date] = line.split("\0");
      return { hash, shortHash, subject, author, date };
    });
}

export async function getCommitDiff(
  cwd: string,
  hash: string
): Promise<string> {
  const result =
    await $`git -C ${cwd} diff -U8 ${hash}~1 ${hash}`.text();
  return result;
}

export async function getCurrentBranch(cwd: string): Promise<string> {
  const result =
    await $`git -C ${cwd} rev-parse --abbrev-ref HEAD`.text();
  return result.trim();
}
