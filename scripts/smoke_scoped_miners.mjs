import assert from "node:assert/strict"
import { chmodSync, copyFileSync, mkdirSync, readFileSync, writeFileSync } from "node:fs"
import { join } from "node:path"
import { ROOT, isolated, pluginArchive } from "./smoke_common.mjs"

const [A, B, C, D] = "1234".split("").map(c => `${c.repeat(8)}-${c.repeat(4)}-${c.repeat(4)}-${c.repeat(4)}-${c.repeat(12)}`)
const P = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa"
let scenarioFile
const scenario = value => writeFileSync(scenarioFile, JSON.stringify(value))
await isolated(({ root, env }) => {
  const bin = join(root, "fixture-bin")
  mkdirSync(bin)
  copyFileSync(join(ROOT, "sources/notion/fixtures/bin/curl"), join(bin, "curl"))
  chmodSync(join(bin, "curl"), 0o755)
  scenarioFile = join(bin, "scenario.json")
  scenario({})
  env.PATH = `${bin}:${env.PATH}`
  env.NOTION_FIXTURE_TOKEN = "fixture-selected-token"
}, async ({ request, stop, start, errors }) => {
  const archive = readFileSync(pluginArchive())
  const preview = await request("/api/plugins/preview", { method: "POST", data: archive })
  const consents = Object.fromEntries(preview.source_consents.map(({ source, digest }) => [source, digest]))
  await request("/api/plugins", { method: "POST", data: archive, headers: { "X-MemCastle-Consents": JSON.stringify(consents) } })
  await request("/api/source-packages/notion/enable", { method: "POST" })
  await request("/api/source-configs/notion-work", { method: "PUT", json: { source: "notion", credential: { type: "env", name: "NOTION_FIXTURE_TOKEN" }, options: { account: "fixture-work" } } })

  const savedOptions = new Map()
  async function define(name, options, cap = 1000) {
    const unset_options = Object.keys(savedOptions.get(name) ?? {}).filter(key => !(key in options))
    await request(`/api/miners/${name}`, { method: "PUT", json: { source: "notion", source_config: "notion-work", locator: `notion-scope:${name}`, wing: name, max_documents: cap, options, unset_options, allow_broaden: true } })
    savedOptions.set(name, options)
  }
  async function job(name) {
    const submitted = await request(`/api/miners/${name}/run`, { method: "POST" })
    for (let i = 0; i < 600; i++) {
      const result = await request(`/api/jobs/${submitted.id}`)
      if (["completed", "failed", "cancelled"].includes(result.status)) return result
      await new Promise(done => setTimeout(done, 50))
    }
    throw new Error("job timed out")
  }
  async function run(name) {
    for (let i = 0; i < 100; i++) {
      const result = await job(name)
      assert.equal(result.status, "completed", JSON.stringify(result))
      if (!result.result.truncated) return result.result
    }
    throw new Error("too many continuation jobs")
  }
  async function search(wing, query) { return request(`/api/search?wing=${wing}&q=${encodeURIComponent(query)}`) }
  const present = async (wing, query) => assert.ok((await search(wing, query)).length > 0, `${wing}: expected ${query}`)
  const absent = async (wing, query) => assert.equal((await search(wing, query)).length, 0, `${wing}: unexpected ${query}`)

  await define("alpha", { roots: [A] }, 1)
  await define("beta", { pages: [B] })
  await define("rows", { databases: [D] })
  const [, beta] = await Promise.all([run("alpha"), run("beta"), run("rows")])
  await present("alpha", "Quasar")
  await present("alpha", "Nebula")
  await present("alpha", "Satellite")
  await present("beta", "Orion")
  await present("rows", "Database")
  await present("rows", "Orbit")
  await present("rows", "Alternate")
  assert.equal((await run("alpha")).documents, 0)

  // The progress registration survives a job limit and daemon restart.
  await define("registration", { pages: [A] }, 1)
  assert.equal((await job("registration")).status, "completed")
  await stop(); await start()
  scenario({ missing: A })
  await run("registration")
  await absent("registration", "Quasar")
  scenario({ edit: "Pulsar changed" })
  await define("alpha", { roots: [A], rescan: "edit" }, 1)
  await run("alpha")
  await present("alpha", "Pulsar")

  // Same scope, reduced block selection: no stale searchable section remains.
  await define("alpha", { roots: [A], exclude_blocks: [P], rescan: "blocks" }, 1)
  await run("alpha")
  await absent("alpha", "Nebula")
  await absent("alpha", "Quasar")
  await present("alpha", "Satellite")

  // Moving a known child outside the root retires it without touching another scope.
  scenario({ moved: true })
  await define("alpha", { roots: [A], rescan: "move" }, 1)
  await run("alpha")
  await absent("alpha", "Satellite")
  await present("beta", "Orion")

  // Page parent metadata can hide a physical toggle/list ancestor.
  scenario({ nested_child: true })
  await define("alpha", { roots: [A], exclude_blocks: [P], rescan: "nested-branch" }, 1)
  await run("alpha")
  await absent("alpha", "Satellite")
  await define("alpha", { roots: [A], rescan: "restore-nested-branch" }, 1)
  await run("alpha")
  await present("alpha", "Satellite")

  // Global authentication/rate failures must not retire anything or leak response bodies.
  scenario({ failure: 429 })
  await define("beta", { pages: [B], rescan: "rate" })
  const failed = await job("beta")
  assert.equal(failed.status, "failed")
  assert.match(failed.error, /75 seconds/)
  assert.ok(!JSON.stringify(failed).includes("fixture-selected-token"))
  await present("beta", "Orion")
  scenario({ missing: B })
  await run("beta")
  await absent("beta", "Orion")
  await define("beta", { pages: [B], rescan: "repeat-tombstone" })
  const repeated = await run("beta")
  assert.equal(repeated.created, 0)
  scenario({})
  await define("beta", { pages: [B], rescan: "restore" })
  await run("beta")
  await present("beta", "Orion")
  assert.equal((await run("beta")).documents, 0)
  await define("beta", { pages: [B], rescan: "idempotent" })
  const unchanged = await run("beta")
  assert.equal(unchanged.created, 0)
  assert.equal(unchanged.superseded, 0)

  // An acquisition failure retries the pending read instead of losing its inventory.
  await define("recover", { pages: [A] }, 1)
  await job("recover")
  scenario({ failure: 429, failure_path: `/blocks/${A}/children` })
  assert.equal((await job("recover")).status, "failed")
  scenario({})
  await run("recover")
  await present("recover", "Quasar")

  // Property/title changes remove formerly selected records; trash and restoration work too.
  await define("beta", { pages: [B], title_prefix: "Beta", property_filter: JSON.stringify({ property: "Status", status: { equals: "Ready" } }), rescan: "filter" })
  scenario({ status: "Draft" })
  await run("beta")
  await absent("beta", "Orion")
  scenario({})
  await define("beta", { pages: [B], rescan: "unfilter" })
  await run("beta")
  await present("beta", "Orion")
  scenario({ trash: B })
  await define("beta", { pages: [B], rescan: "trash" })
  await run("beta")
  await absent("beta", "Orion")
  scenario({})
  await define("beta", { pages: [B], rescan: "untrash" })
  await run("beta")
  await present("beta", "Orion")

  // Replacing anchors uses the old inventory to retire the original scope.
  await define("alpha", { roots: [B], rescan: "new-root" }, 1)
  await run("alpha")
  await absent("alpha", "Quasar")
  await absent("alpha", "Satellite")
  await present("alpha", "Orion")

  // Search periodically discovers newly shared pages even when their edit time is old.
  await define("workspace", { workspace: true })
  await run("workspace")
  await absent("workspace", "Database")
  scenario({ newly_shared: true })
  await define("workspace", { workspace: true, rescan: "shared-old-page" })
  await run("workspace")
  await present("workspace", "Database")
  assert.ok(beta.source_id)
  const sources = await request("/api/sources")
  assert.ok(!JSON.stringify(sources).includes("fixture-selected-token"))
  assert.ok(!errors().includes("fixture-selected-token"))
  console.log("Notion installed-archive lifecycle checks passed")
})
