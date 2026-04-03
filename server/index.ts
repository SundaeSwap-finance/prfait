import { Hono } from "hono";
import { serveStatic } from "hono/bun";
import { mkdir, readFile, writeFile } from "fs/promises";
import { join } from "path";
import {
  getChangedFiles,
  getCommitDiff,
  getCommits,
  getCurrentBranch,
  getFileContent,
  getFileDiff,
  getFileLineCount,
  getFileLines,
  getFullDiff,
} from "./git";

const app = new Hono();

// The repo we're analyzing — passed via env or defaults to cwd
const REPO_DIR = process.env.PRFAIT_REPO ?? process.cwd();
const BASE_BRANCH = process.env.PRFAIT_BASE ?? "main";
const PORT = parseInt(process.env.PORT ?? "3000");

// --- API routes ---

app.get("/api/meta", async (c) => {
  const branch = await getCurrentBranch(REPO_DIR);
  return c.json({ repo: REPO_DIR, branch, base: BASE_BRANCH });
});

app.get("/api/files", async (c) => {
  const files = await getChangedFiles(REPO_DIR, BASE_BRANCH);
  return c.json(files);
});

app.get("/api/diff/:path{.+}", async (c) => {
  const path = c.req.param("path");
  const diff = await getFileDiff(REPO_DIR, BASE_BRANCH, path);
  return c.text(diff);
});

app.get("/api/diff", async (c) => {
  const diff = await getFullDiff(REPO_DIR, BASE_BRANCH);
  return c.text(diff);
});

app.get("/api/content/:ref/:path{.+}", async (c) => {
  const ref = c.req.param("ref");
  const path = c.req.param("path");
  try {
    const file = await getFileContent(REPO_DIR, ref, path);
    return c.json(file);
  } catch {
    return c.json({ error: "File not found at ref" }, 404);
  }
});

// Lines from a file at a ref
app.get("/api/lines/:ref/:path{.+}", async (c) => {
  const ref = c.req.param("ref");
  const path = c.req.param("path");
  const start = parseInt(c.req.query("start") ?? "1");
  const end = parseInt(c.req.query("end") ?? "50");
  try {
    const lines = await getFileLines(REPO_DIR, ref, path, start, end);
    const total = await getFileLineCount(REPO_DIR, ref, path);
    return c.json({ lines, total, start, end });
  } catch {
    return c.json({ error: "File not found at ref" }, 404);
  }
});

app.get("/api/commits", async (c) => {
  const commits = await getCommits(REPO_DIR, BASE_BRANCH);
  return c.json(commits);
});

app.get("/api/commits/:hash/diff", async (c) => {
  const hash = c.req.param("hash");
  const diff = await getCommitDiff(REPO_DIR, hash);
  return c.text(diff);
});

// --- Story persistence ---

const STORY_DIR = join(REPO_DIR, ".prfait");

async function storyPath(): Promise<string> {
  const branch = await getCurrentBranch(REPO_DIR);
  // Sanitize branch name for filename
  const safe = branch.replace(/[^a-zA-Z0-9._-]/g, "_");
  return join(STORY_DIR, `${safe}.json`);
}

app.get("/api/story", async (c) => {
  try {
    const path = await storyPath();
    const data = await readFile(path, "utf-8");
    return c.json(JSON.parse(data));
  } catch {
    return c.json(null);
  }
});

app.post("/api/story", async (c) => {
  const body = await c.req.json();
  const path = await storyPath();
  await mkdir(STORY_DIR, { recursive: true });
  await writeFile(path, JSON.stringify(body, null, 2));
  return c.json({ ok: true });
});

// In production, serve the built frontend
if (process.env.NODE_ENV === "production") {
  app.use("/*", serveStatic({ root: "./dist" }));
  app.get("/*", serveStatic({ path: "./dist/index.html" }));
}

console.log(`prfait API on http://localhost:${PORT}`);
console.log(`  repo: ${REPO_DIR}`);
console.log(`  base: ${BASE_BRANCH}`);

export default {
  port: PORT,
  fetch: app.fetch,
};
