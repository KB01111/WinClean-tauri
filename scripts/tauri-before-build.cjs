const { spawnSync } = require("node:child_process")
const path = require("node:path")

const root = path.resolve(__dirname, "..")
const vite = path.join(root, "node_modules", "vite", "bin", "vite.js")
const result = spawnSync(process.execPath, [vite, "build"], {
  cwd: root,
  stdio: "inherit",
  shell: false,
})

if (result.error) {
  console.error(result.error)
}

process.exit(result.status ?? 1)
