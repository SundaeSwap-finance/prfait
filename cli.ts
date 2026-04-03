#!/usr/bin/env bun

import { $ } from "bun";
import { existsSync } from "fs";
import { join, resolve } from "path";

const args = process.argv.slice(2);
const command = args[0];

const REPO_DIR = process.cwd();

async function getCurrentBranch(): Promise<string> {
  return (await $`git rev-parse --abbrev-ref HEAD`.text()).trim();
}

async function getStoryPath(): Promise<string> {
  const branch = await getCurrentBranch();
  const safe = branch.replace(/[^a-zA-Z0-9._-]/g, "_");
  return join(REPO_DIR, ".prfait", `${safe}.json`);
}

// ── Commands ────────────────────────────────────────────────────────

async function confirm(message: string): Promise<boolean> {
  process.stdout.write(`${message} [y/N] `);
  for await (const line of console) {
    const answer = line.trim().toLowerCase();
    return answer === "y" || answer === "yes";
  }
  return false;
}

async function startEditor(base: string) {
  console.log(`Starting prfait editor...`);
  console.log(`  branch: ${await getCurrentBranch()}`);
  console.log(`  base: ${base}`);

  const prfaitDir = import.meta.dir;

  const env = {
    ...process.env,
    PRFAIT_REPO: REPO_DIR,
    PRFAIT_BASE: base,
  };

  const proc = Bun.spawn(["bun", join(prfaitDir, "dev.ts")], {
    cwd: prfaitDir,
    env,
    stdout: "inherit",
    stderr: "inherit",
  });

  await proc.exited;
}

async function cmdNew() {
  const base = args[1] ?? "main";
  const storyPath = await getStoryPath();

  if (existsSync(storyPath)) {
    const ok = await confirm(
      "A story already exists for this branch. This will wipe out the existing draft. Are you sure?"
    );
    if (!ok) {
      console.log("Aborted.");
      process.exit(0);
    }
    // Clear the existing story
    const { mkdir } = await import("fs/promises");
    await mkdir(join(REPO_DIR, ".prfait"), { recursive: true });
    await Bun.write(storyPath, JSON.stringify({ version: 1, panels: [], updatedAt: new Date().toISOString() }, null, 2));
    console.log("Cleared existing story.");
  }

  await startEditor(base);
}

async function cmdEdit() {
  const base = args[1] ?? "main";
  const storyPath = await getStoryPath();

  if (!existsSync(storyPath)) {
    console.error(`No story found for branch '${await getCurrentBranch()}'.`);
    console.error(`Run 'prfait new' to create one.`);
    process.exit(1);
  }

  await startEditor(base);
}

async function cmdRender() {
  const storyPath = args[1] ?? (await getStoryPath());

  if (!existsSync(storyPath)) {
    console.error(`No story found at ${storyPath}`);
    process.exit(1);
  }

  const story = JSON.parse(await Bun.file(storyPath).text());
  const md = renderStoryToMarkdown(story);
  console.log(md);
}

async function cmdInit() {
  const workflowDir = join(REPO_DIR, ".github", "workflows");
  const workflowPath = join(workflowDir, "prfait.yml");

  if (existsSync(workflowPath)) {
    console.log("prfait workflow already exists at .github/workflows/prfait.yml");
    return;
  }

  await $`mkdir -p ${workflowDir}`;

  // Read the workflow template from prfait's own directory
  const templatePath = join(import.meta.dir, "workflow-template.yml");
  const workflow = await Bun.file(templatePath).text();

  await Bun.write(workflowPath, workflow);
  console.log("Created .github/workflows/prfait.yml");

  // Create .prfait directory
  const prfaitDir = join(REPO_DIR, ".prfait");
  if (!existsSync(prfaitDir)) {
    await $`mkdir -p ${prfaitDir}`;
    await Bun.write(join(prfaitDir, ".gitkeep"), "");
    console.log("Created .prfait/ directory");
  }

  console.log("\nSetup complete! Workflow:");
  console.log("  1. Run 'prfait new' to craft your PR story");
  console.log("  2. Commit the .prfait/ directory with your branch");
  console.log("  3. Push — the GitHub Action will render your story into the PR description");
  console.log("\nNote: The action needs permission to update PR descriptions.");
  console.log("  The workflow requests 'pull-requests: write' which works with the default");
  console.log("  GITHUB_TOKEN. If your org restricts default token permissions, ensure");
  console.log("  'pull-requests: write' is allowed under Settings → Actions → General.");
}

// ── Markdown renderer ───────────────────────────────────────────────

interface StoryDocument {
  version: number;
  panels: Panel[];
}

type Panel = {
  kind: "snippet";
  file: string;
  startLine: number;
  endLine: number;
  content: string;
  comments?: { afterLine: number; markdown: string }[];
} | {
  kind: "narration";
  markdown: string;
};

const LANG_MAP: Record<string, string> = {
  ts: "typescript", tsx: "tsx", js: "javascript", jsx: "jsx",
  rs: "rust", py: "python", go: "go", rb: "ruby", java: "java",
  c: "c", h: "c", cpp: "cpp", cs: "csharp", sh: "bash",
  json: "json", yaml: "yaml", yml: "yaml", toml: "toml",
  md: "markdown", css: "css", scss: "scss", html: "html",
  sql: "sql", hs: "haskell", nix: "nix", ex: "elixir",
};

function langFromFile(file: string): string {
  const ext = file.split(".").pop() ?? "";
  return LANG_MAP[ext] ?? ext;
}

function renderStoryToMarkdown(story: StoryDocument): string {
  const parts: string[] = [];

  // Header with link to viewer (placeholder URL for now)
  parts.push(`<!-- Rendered by prfait -->\n`);

  for (const panel of story.panels) {
    if (panel.kind === "narration") {
      if (panel.markdown.trim()) {
        parts.push(panel.markdown.trim());
      }
    } else if (panel.kind === "snippet") {
      const lineRange =
        panel.startLine > 0
          ? panel.startLine === panel.endLine
            ? `:${panel.startLine}`
            : `:${panel.startLine}-${panel.endLine}`
          : "";

      const lang = langFromFile(panel.file);
      const hasDeletions = panel.content.split("\n").some(
        (l) => l.startsWith("-")
      );

      // File header
      parts.push(`**\`${panel.file}${lineRange}\`**`);

      // Split content into segments separated by comments
      const lines = panel.content.split("\n");
      const commentMap = new Map<number, string>();
      if (panel.comments) {
        for (const c of panel.comments) {
          commentMap.set(c.afterLine, c.markdown);
        }
      }

      let codeBuffer: string[] = [];

      function flushCode() {
        if (codeBuffer.length === 0) return;
        if (hasDeletions) {
          // Has removals — use diff fence for red/green coloring
          parts.push("```diff\n" + codeBuffer.join("\n") + "\n```");
        } else {
          // All additions or context — use language fence, strip prefixes
          const stripped = codeBuffer.map((l) =>
            l.startsWith("+") || l.startsWith(" ") ? l.slice(1) : l
          );
          parts.push("```" + lang + "\n" + stripped.join("\n") + "\n```");
        }
        codeBuffer = [];
      }

      for (let i = 0; i < lines.length; i++) {
        codeBuffer.push(lines[i]);

        const comment = commentMap.get(i);
        if (comment) {
          flushCode();
          // Render comment as a blockquote so it's visually distinct
          const quoted = comment
            .split("\n")
            .map((l) => `> ${l}`)
            .join("\n");
          parts.push(quoted);
        }
      }

      flushCode();
    }
  }

  return parts.join("\n\n");
}

// ── Main ────────────────────────────────────────────────────────────

switch (command) {
  case "new":
    await cmdNew();
    break;
  case "edit":
    await cmdEdit();
    break;
  case "render":
    await cmdRender();
    break;
  case "init":
    await cmdInit();
    break;
  case "help":
  case undefined:
    console.log(`prfait — craft PR stories

Commands:
  prfait new [base]    Create a new story (default base: main)
  prfait edit [base]   Edit existing story for this branch
  prfait render [path] Render story to markdown (stdout)
  prfait init          Set up GitHub Action for this repo
  prfait help          Show this help
`);
    break;
  default:
    console.error(`Unknown command: ${command}`);
    process.exit(1);
}
