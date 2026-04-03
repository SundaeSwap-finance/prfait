// Starts both the API server and Vite dev server as a single command.
// Visit http://localhost:3001 — Vite serves the frontend with HMR, proxies /api to the backend.

import { $ } from "bun";

// Start Vite (port 3001) — this is the URL you visit
const vite = Bun.spawn(["bunx", "--bun", "vite", "--config", "vite.config.ts", "--port", "3001"], {
  stdout: "inherit",
  stderr: "inherit",
});

// Open browser once Vite is ready
const URL = "http://localhost:3001";
(async () => {
  // Wait for Vite to be listening
  for (let i = 0; i < 50; i++) {
    try {
      await fetch(URL);
      break;
    } catch {
      await Bun.sleep(100);
    }
  }
  // xdg-open on Linux
  Bun.spawn(["xdg-open", URL], { stdout: "ignore", stderr: "ignore" });
})();

// Start the API server with hot reload
const server = Bun.spawn(["bun", "run", "--hot", "server/index.ts"], {
  stdout: "inherit",
  stderr: "inherit",
});

async function cleanup() {
  vite.kill();
  server.kill();
  await Promise.allSettled([vite.exited, server.exited]);
  // Reset terminal to sane state
  process.stdout.write("\x1b[?1049l\x1b[?25h\x1bc");
  process.exit(0);
}

process.on("SIGINT", cleanup);
process.on("SIGTERM", cleanup);

// Wait for either to exit
await Promise.race([vite.exited, server.exited]);
await cleanup();
