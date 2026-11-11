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
  async function define(name, options, cap = 1000, wing = name) {
    const unset_options = Object.keys(savedOptions.get(name) ?? {}).filter(key => !(key in options))
    // All miners use the same default workspace locator. Core, not the source,
    // partitions their state, including when their destinations also match.
    await request(`/api/miners/${name}`, { method: "PUT", json: { source: "notion", source_config: "notion-work", wing, max_documents: cap, options, unset_options, allow_broaden: true } })
    savedOptions.set(name, options)
  }
  async function job(name, full = false) {
    const submitted = await request(`/api/miners/${name}/run`, { method: "POST", json: { full } })
    for (let i = 0; i < 600; i++) {
      const result = await request(`/api/jobs/${submitted.id}`)
      if (["completed", "failed", "cancelled"].includes(result.status)) return result
      await new Promise(done => setTimeout(done, 50))
    }
    throw new Error("job timed out")
  }
  async function run(name, full = false) {
    const totals = {}
    for (let i = 0; i < 100; i++) {
      // A truncated full scan continues normally; starting another full scan on
      // every continuation would restart its inventory walk indefinitely.
      const result = await job(name, full && i === 0)
      assert.equal(result.status, "completed", JSON.stringify(result))
      for (const key of ["documents", "discovered", "ingested", "created", "superseded", "retired", "unchanged", "skipped"]) {
        totals[key] = (totals[key] ?? 0) + result.result[key]
      }
      if (!result.result.truncated) return { ...result.result, ...totals }
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
  const unchanged = await run("alpha")
  assert.ok(unchanged.documents >= 2, "Core must recheck known pages even before the next discovery interval")
  assert.equal(unchanged.created, 0)
  assert.equal(unchanged.superseded, 0)

  // The first limited job acquires a real document, not a registration candidate.
  await define("registration", { pages: [A] }, 1)
  const first = await job("registration")
  assert.equal(first.status, "completed", JSON.stringify(first))
  assert.equal(first.result.ingested, 1)
  await stop(); await start()
  scenario({ missing: A })
  await run("registration", true)
  await absent("registration", "Quasar")

  // Nested edits do not require a page timestamp change or provider rescan key.
  scenario({ edit: "Pulsar changed" })
  await run("alpha")
  await present("alpha", "Pulsar")
  await define("alpha", { roots: [A], exclude_blocks: [P] }, 1)
  await run("alpha")
  await absent("alpha", "Nebula")
  await absent("alpha", "Quasar")
  await present("alpha", "Satellite")
  scenario({ moved: true })
  await define("alpha", { roots: [A] }, 1)
  await run("alpha")
  await absent("alpha", "Satellite")
  await present("beta", "Orion")

  // Physical toggle/list ancestry can differ from page parent metadata.
  scenario({ nested_child: true })
  await define("alpha", { roots: [A], exclude_blocks: [P] }, 1)
  await run("alpha")
  await absent("alpha", "Satellite")
  await define("alpha", { roots: [A] }, 1)
  await run("alpha")
  await present("alpha", "Satellite")

  scenario({ failure: 429 })
  const failed = await job("beta")
  assert.equal(failed.status, "failed")
  assert.match(failed.error, /75 seconds/)
  assert.ok(!JSON.stringify(failed).includes("fixture-selected-token"))
  await present("beta", "Orion")
  scenario({ missing: B })
  await run("beta", true)
  await absent("beta", "Orion")
  const repeated = await run("beta", true)
  assert.equal(repeated.retired, 0, "repeated retirement must be idempotent")
  scenario({})
  await run("beta")
  await present("beta", "Orion")
  const restoredUnchanged = await run("beta", true)
  assert.equal(restoredUnchanged.created, 0)
  assert.equal(restoredUnchanged.superseded, 0)

  // Failure before first ingestion leaves registration in Core, not the cursor.
  await define("pending", { pages: [A] }, 1)
  scenario({ failure: 429, failure_path: `/blocks/${A}/children` })
  assert.equal((await job("pending")).status, "failed")
  await stop(); await start()
  scenario({ missing: A })
  await run("pending", true)
  await absent("pending", "Quasar")
  scenario({})
  await run("pending")
  await present("pending", "Quasar")
  // A failed refresh must also leave previously current content intact.
  scenario({ failure: 429, failure_path: `/blocks/${A}/children` })
  assert.equal((await job("pending")).status, "failed")
  await present("pending", "Quasar")
  scenario({})
  await run("pending")

  await define("beta", { pages: [B], title_prefix: "Beta", property_filter: JSON.stringify({ property: "Status", status: { equals: "Ready" } }) })
  scenario({ status: "Draft" })
  await run("beta")
  await absent("beta", "Orion")
  scenario({})
  await define("beta", { pages: [B] })
  await run("beta")
  await present("beta", "Orion")
  scenario({ trash: B })
  await run("beta", true)
  await absent("beta", "Orion")
  scenario({})
  await run("beta")
  await present("beta", "Orion")
  await define("alpha", { roots: [B] }, 1)
  await run("alpha", true)
  await absent("alpha", "Quasar")
  await absent("alpha", "Satellite")
  await present("alpha", "Orion")

  await define("workspace", { workspace: true })
  await run("workspace")
  await absent("workspace", "Database")
  scenario({ newly_shared: true })
  await run("workspace", true)
  await present("workspace", "Database")

  // Same account, locator, options AND destination: only the miner name differs.
  scenario({})
  await define("twin-one", { pages: [A] }, 1000, "shared")
  await define("twin-two", { pages: [A] }, 1000, "shared")
  const one = await run("twin-one")
  const two = await run("twin-two")
  assert.notEqual(one.source_id, two.source_id)
  assert.equal((await search("shared", "Quasar")).length, 2)
  await define("twin-one", { pages: [A], title_prefix: "No match" }, 1000, "shared")
  const narrowed = await run("twin-one", true)
  assert.equal(narrowed.source_id, one.source_id, "filter edits preserve Core inventory identity")
  assert.equal((await search("shared", "Quasar")).length, 1, "retirement must not affect the other miner")

  // Two manual requests for one miner are serialized by Core's source lease.
  const concurrent = await Promise.all([job("beta"), job("beta")])
  assert.notEqual(concurrent[0].id, concurrent[1].id, "exercise two distinct mining jobs")
  assert.ok(concurrent.every(result => result.status === "completed"), JSON.stringify(concurrent))
  assert.equal(concurrent[0].result.source_id, concurrent[1].result.source_id)
  assert.equal(concurrent[0].result.created + concurrent[1].result.created, 0)
  await present("beta", "Orion")
  assert.ok(beta.source_id)
  const sources = await request("/api/sources")
  const serialized = JSON.stringify(sources)
  assert.ok(!serialized.includes("fixture-selected-token"))
  assert.ok(!serialized.includes("notion-progress"))
  assert.ok(!serialized.includes('"known":'), "provider cursors must not contain document inventories")
  assert.ok(!errors().includes("fixture-selected-token"))
  console.log("Notion Core-managed reconciliation lifecycle checks passed")
})
