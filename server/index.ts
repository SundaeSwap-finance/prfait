import { Hono } from "hono";
import { cors } from "hono/cors";
import { serveStatic } from "hono/bun";
import {
  getChangedFiles,
  getCommitDiff,
  getCommits,
  getCurrentBranch,
  getFileContent,
  getFileDiff,
  getFullDiff,
} from "./git";

const app = new Hono();

// The repo we're analyzing — passed via env or defaults to cwd
const REPO_DIR = process.env.PRFAIT_REPO ?? process.cwd();
const BASE_BRANCH = process.env.PRFAIT_BASE ?? "main";
const PORT = parseInt(process.env.PORT ?? "3000");

app.use("/api/*", cors());

// Repo metadata
app.get("/api/meta", async (c) => {
  const branch = await getCurrentBranch(REPO_DIR);
  return c.json({ repo: REPO_DIR, branch, base: BASE_BRANCH });
});

// Changed files between base and HEAD
app.get("/api/files", async (c) => {
  const files = await getChangedFiles(REPO_DIR, BASE_BRANCH);
  return c.json(files);
});

// Unified diff for a single file
app.get("/api/diff/:path{.+}", async (c) => {
  const path = c.req.param("path");
  const diff = await getFileDiff(REPO_DIR, BASE_BRANCH, path);
  return c.text(diff);
});

// Full diff (all files)
app.get("/api/diff", async (c) => {
  const diff = await getFullDiff(REPO_DIR, BASE_BRANCH);
  return c.text(diff);
});

// File content at a ref
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

// Commits between base and HEAD
app.get("/api/commits", async (c) => {
  const commits = await getCommits(REPO_DIR, BASE_BRANCH);
  return c.json(commits);
});

// Diff for a specific commit
app.get("/api/commits/:hash/diff", async (c) => {
  const hash = c.req.param("hash");
  const diff = await getCommitDiff(REPO_DIR, hash);
  return c.text(diff);
});

// In production, serve the built frontend
if (process.env.NODE_ENV === "production") {
  app.use("/*", serveStatic({ root: "./dist" }));
  app.get("/*", serveStatic({ path: "./dist/index.html" }));
}

console.log(`prfait server on http://localhost:${PORT}`);
console.log(`  repo: ${REPO_DIR}`);
console.log(`  base: ${BASE_BRANCH}`);

export default {
  port: PORT,
  fetch: app.fetch,
};
