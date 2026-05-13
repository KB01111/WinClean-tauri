const { spawn } = require("node:child_process")
const path = require("node:path")

const root = path.resolve(__dirname, "..")
const vite = path.join(root, "node_modules", "vite", "bin", "vite.js")
const child = spawn(process.execPath, [vite, "--host", "127.0.0.1"], {
  cwd: root,
  stdio: "inherit",
  shell: false,
})

child.on("exit", (code) => process.exit(code ?? 0))
