const { spawnSync } = require("node:child_process")
const path = require("node:path")

const root = path.resolve(__dirname, "..")

const steps = [
  [process.execPath, [path.join(root, "node_modules", "typescript", "bin", "tsc"), "--noEmit"]],
  [process.execPath, [path.join(root, "node_modules", "vite", "bin", "vite.js"), "build"]],
]

for (const [command, args] of steps) {
  const result = spawnSync(command, args, {
    cwd: root,
    stdio: "inherit",
    shell: false,
  })

  if (result.status !== 0) {
    if (result.error) {
      console.error(result.error)
    }
    process.exit(result.status ?? 1)
  }
}
