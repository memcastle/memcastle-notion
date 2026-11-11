import assert from "node:assert/strict"
import { spawn } from "node:child_process"
import { mkdtempSync, mkdirSync, readdirSync, rmSync } from "node:fs"
import { createServer } from "node:net"
import { tmpdir } from "node:os"
import { dirname, join, resolve } from "node:path"
import { fileURLToPath } from "node:url"

export const ROOT = dirname(dirname(fileURLToPath(import.meta.url)))
const BIN = process.env.MEMCASTLE_BIN ? resolve(process.env.MEMCASTLE_BIN) : "memcastle"

export function pluginArchive() {
  if (process.env.ARCHIVE) return resolve(ROOT, process.env.ARCHIVE)
  const files = readdirSync(join(ROOT, "dist")).filter(f => /^notion-.*\.tar\.gz$/.test(f))
  assert.equal(files.length, 1, "set ARCHIVE to the Notion archive to test")
  return join(ROOT, "dist", files[0])
}

export async function isolated(configure, check) {
  const root = mkdtempSync(join(tmpdir(), "memcastle-notion-"))
  const assets = join(root, "assets")
  mkdirSync(assets)
  const env = Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith("MEMCASTLE_")))
  Object.assign(env, { HOME: root, XDG_DATA_HOME: join(root, "data"), XDG_CONFIG_HOME: join(root, "config"), XDG_STATE_HOME: join(root, "state"), MEMCASTLE_ASSETS_DIR: assets })
  const server = createServer()
  await new Promise(done => server.listen(0, "127.0.0.1", done))
  const port = server.address().port
  await new Promise(done => server.close(done))
  let child, errors = ""
  const request = async (route, { method = "GET", json, data, headers = {} } = {}) => {
    const response = await fetch(`http://127.0.0.1:${port}${route}`, { method, headers: { ...(json && { "Content-Type": "application/json" }), ...headers }, body: json ? JSON.stringify(json) : data, signal: AbortSignal.timeout(20000) })
    const text = await response.text()
    const body = text ? JSON.parse(text) : null
    assert.ok(response.ok, `HTTP ${response.status}: ${text}`)
    return body
  }
  const stop = async () => {
    if (!child || child.exitCode !== null) return
    const running = child
    const exited = new Promise(done => running.once("exit", done))
    running.kill("SIGTERM")
    const timer = setTimeout(() => running.kill("SIGKILL"), 20000)
    await exited
    clearTimeout(timer)
    child = undefined
  }
  const start = async () => {
    errors = ""
    child = spawn(BIN, ["--palace", join(root, "palace"), "serve", "--bind", "127.0.0.1", "--port", String(port)], { cwd: root, env, stdio: ["ignore", "ignore", "pipe"] })
    child.on("error", error => { errors += error.message })
    child.stderr.on("data", chunk => { errors += chunk })
    for (let attempt = 0; attempt < 120; attempt++) {
      assert.equal(child.exitCode, null, errors)
      try { await request("/api/health"); return } catch { await new Promise(done => setTimeout(done, 250)) }
    }
    throw new Error(`daemon did not start: ${errors}`)
  }
  try { await configure({ root, env }); await start(); await check({ root, request, stop, start, errors: () => errors }) }
  finally { await stop(); rmSync(root, { recursive: true, force: true }) }
}
