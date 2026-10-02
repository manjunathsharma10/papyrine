#!/usr/bin/env node
// Spike 0.1 measurement: launch-to-first-tile, idle memory over all app
// processes, bundle sizes. Works on macOS, Linux and Windows.
//
//   node scripts/measure-shell.mjs --bin <path-to-papyrine-binary> \
//        [--runs 10] [--idle-secs 10] [--bundle-dir <dir>] [--dist <dir>] [--json out.json]
//
// Launch time = wall clock from just before spawn() to the host receiving the
// UI's first-tile-painted event (printed as epoch_ms when PAPYRINE_TRACE=1).
// Idle memory = sum over every process of the app (including webview helpers)
// 'idle-secs' after first paint: macOS phys_footprint, Windows private working
// set. Linux: the GATED figure counts the app's own memory only (anonymous +
// private dirty pages, excluding shared file-backed library pages; ADR-026);
// PSS is still recorded for information.

import { execFileSync, spawn } from "node:child_process";
import { readFileSync, readdirSync, statSync, writeFileSync, mkdtempSync, existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve, extname } from "node:path";
import { gzipSync } from "node:zlib";

const args = parseArgs(process.argv.slice(2));
const platform = process.platform;
const runs = Number(args.runs ?? 10);
const idleSecs = Number(args["idle-secs"] ?? 10);
const here = new URL(".", import.meta.url).pathname.replace(/^\/([A-Za-z]:)/, "$1");
const dist = resolve(args.dist ?? join(here, "..", "dist"));
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const MB = 1024 * 1024;

function parseArgs(argv) {
  const out = {};
  for (let i = 0; i < argv.length; i++) {
    if (argv[i].startsWith("--")) out[argv[i].slice(2)] = argv[i + 1]?.startsWith("--") || argv[i + 1] === undefined ? true : argv[++i];
  }
  return out;
}

const median = (a) => {
  const s = [...a].sort((x, y) => x - y);
  const m = s.length >> 1;
  return s.length % 2 ? s[m] : (s[m - 1] + s[m]) / 2;
};
const p95 = (a) => [...a].sort((x, y) => x - y)[Math.min(a.length - 1, Math.ceil(a.length * 0.95) - 1)];

function dirSize(p) {
  const st = statSync(p);
  if (!st.isDirectory()) return st.size;
  return readdirSync(p).reduce((n, f) => n + dirSize(join(p, f)), 0);
}

// ---- process enumeration ----------------------------------------------------

function psTable() {
  if (platform === "win32") {
    const out = execFileSync(
      "powershell",
      ["-NoProfile", "-Command", "Get-CimInstance Win32_Process | ForEach-Object { \"$($_.ProcessId) $($_.ParentProcessId) $($_.Name)\" }"],
      { encoding: "utf8" },
    );
    return out.split(/\r?\n/).filter(Boolean).map((l) => {
      const [pid, ppid, ...name] = l.trim().split(" ");
      return { pid: +pid, ppid: +ppid, cmd: name.join(" ") };
    });
  }
  const out = execFileSync("ps", ["-axo", "pid=,ppid=,command="], { encoding: "utf8", maxBuffer: 64 * MB });
  return out.split("\n").filter(Boolean).map((l) => {
    const m = l.trim().match(/^(\d+)\s+(\d+)\s+(.*)$/);
    return { pid: +m[1], ppid: +m[2], cmd: m[3] };
  });
}

const WK = /com\.apple\.WebKit\.(WebContent|Networking|GPU)/;

/**
 * macOS: WebKit helpers are XPC services (ppid 1). Attribute them with the
 * private responsibility API (what Activity Monitor uses) via python3/ctypes.
 */
function macResponsible(pids) {
  const code = `import ctypes,sys\nl=ctypes.CDLL(None)\nl.responsibility_get_pid_responsible_for_pid.argtypes=[ctypes.c_int]\nl.responsibility_get_pid_responsible_for_pid.restype=ctypes.c_int\nprint(" ".join(str(l.responsibility_get_pid_responsible_for_pid(int(p))) for p in sys.argv[1:]))`;
  const out = execFileSync("python3", ["-c", code, ...pids.map(String)], { encoding: "utf8" }).trim().split(" ");
  return new Map(pids.map((p, i) => [p, Number(out[i])]));
}

const macWebKitPids = () => new Set(psTable().filter((p) => WK.test(p.cmd)).map((p) => p.pid));

function appPids(rootPid, baseline = new Set()) {
  const table = psTable();
  const tree = new Set([rootPid]);
  for (let grew = true; grew; ) {
    grew = false;
    for (const p of table) if (!tree.has(p.pid) && tree.has(p.ppid)) (tree.add(p.pid), (grew = true));
  }
  if (platform === "darwin") {
    const cand = table.filter((p) => WK.test(p.cmd)).map((p) => p.pid);
    const resp = macResponsible(cand);
    for (const [pid, r] of resp) if (r === rootPid) tree.add(pid);
    if (process.env.MEASURE_DEBUG) console.error("responsible", Object.fromEntries(resp), "root", rootPid);
    // Fallback (e.g. CI runners where responsibility is not per-app): helpers that appeared after launch.
    if (tree.size === 1) {
      for (const pid of cand) if (!baseline.has(pid)) tree.add(pid);
      fallbackAttribution++;
    }
  }
  return [...tree];
}

function memoryBytes(pids) {
  const per = {};
  const pssSplit = {};
  const own = {}; // Linux only: anonymous + private dirty, the gated figure
  if (platform === "darwin") {
    const dir = mkdtempSync(join(tmpdir(), "fp-"));
    for (const pid of pids) {
      try {
        const f = join(dir, `${pid}.json`);
        execFileSync("footprint", ["-p", String(pid), "--json", f], { stdio: "ignore" });
        const j = JSON.parse(readFileSync(f, "utf8"));
        const proc = j.processes[0];
        per[`${proc.name}[${pid}]`] = proc.footprint;
      } catch (e) {
        if (process.env.MEASURE_DEBUG) console.error("footprint failed", pid, e.message);
      }
    }
  } else if (platform === "linux") {
    for (const pid of pids) {
      try {
        const t = readFileSync(`/proc/${pid}/smaps_rollup`, "utf8");
        const kb = Number(t.match(/^Pss:\s+(\d+) kB/m)?.[1] ?? 0);
        const name = readFileSync(`/proc/${pid}/comm`, "utf8").trim();
        const anon = Number(t.match(/^Pss_Anon:\s+(\d+) kB/m)?.[1] ?? 0);
        const file = Number(t.match(/^Pss_File:\s+(\d+) kB/m)?.[1] ?? 0);
        const privDirty = Number(t.match(/^Private_Dirty:\s+(\d+) kB/m)?.[1] ?? 0);
        per[`${name}[${pid}]`] = kb * 1024;
        own[`${name}[${pid}]`] = Math.max(anon, privDirty) * 1024;
        pssSplit[name] = `anon ${(anon / 1024).toFixed(1)} MB, file-backed ${(file / 1024).toFixed(1)} MB`;
      } catch {
        /* exited */
      }
    }
  } else if (platform === "win32") {
    const out = execFileSync(
      "powershell",
      ["-NoProfile", "-Command", "Get-CimInstance Win32_PerfFormattedData_PerfProc_Process | ForEach-Object { \"$($_.IDProcess) $($_.WorkingSetPrivate) $($_.Name)\" }"],
      { encoding: "utf8" },
    );
    const want = new Set(pids);
    for (const l of out.split(/\r?\n/)) {
      const [pid, ws, name] = l.trim().split(" ");
      if (want.has(+pid)) per[`${name}[${pid}]`] = +ws;
    }
  }
  const total = Object.values(per).reduce((a, b) => a + b, 0);
  const ownTotal = platform === "linux" ? Object.values(own).reduce((a, b) => a + b, 0) : total;
  return { total, ownTotal, per, own, pssSplit };
}

// ---- launching ------------------------------------------------------------------

function killTree(child) {
  try {
    if (platform === "win32") execFileSync("taskkill", ["/pid", String(child.pid), "/T", "/F"], { stdio: "ignore" });
    else process.kill(child.pid, "SIGKILL");
  } catch {
    /* already gone */
  }
}

/**
 * Start the app and expose its trace output. On macOS an .app is started
 * through LaunchServices (`open -n`), like a Finder launch; that also makes the
 * app the "responsible process" of its WebKit helpers, which is how their
 * memory is attributed. Otherwise the binary is spawned directly.
 */
function startApp(bin) {
  const appDir = platform === "darwin" ? bin.match(/^(.*\.app)\/Contents\/MacOS\//)?.[1] : null;
  if (appDir) {
    const log = join(mkdtempSync(join(tmpdir(), "papyrine-trace-")), "stdout.log");
    writeFileSync(log, "");
    const before = new Set(psTable().filter((p) => p.cmd.startsWith(bin)).map((p) => p.pid));
    const baseline = macWebKitPids();
    const t0 = Date.now();
    execFileSync("open", ["-n", "--env", "PAPYRINE_TRACE=1", "--stdout", log, appDir]);
    return {
      t0,
      baseline,
      read: () => readFileSync(log, "utf8"),
      pid: () => psTable().find((p) => p.cmd.startsWith(bin) && !before.has(p.pid))?.pid,
      kill: (pid) => {
        try {
          if (pid) process.kill(pid, "SIGKILL");
        } catch {
          /* gone */
        }
      },
    };
  }
  const t0 = Date.now();
  const child = spawn(bin, [], { env: { ...process.env, PAPYRINE_TRACE: "1" }, stdio: ["ignore", "pipe", "ignore"] });
  let buf = "";
  child.stdout.on("data", (d) => (buf += d));
  return { t0, baseline: new Set(), read: () => buf, pid: () => child.pid, kill: () => killTree(child) };
}

let retries = 0;
let occludedFallbacks = 0;
let fallbackAttribution = 0;
/** Retry a launch that never reported a paint (observed rarely when the window is occluded); counted in the output. */
async function launchOnce(bin, opts) {
  for (let attempt = 0; ; attempt++) {
    try {
      return await launchAttempt(bin, opts);
    } catch (e) {
      if (attempt >= 2 || !/timeout/.test(String(e))) throw e;
      retries++;
      console.error(`retrying launch: ${e.message.slice(0, 200)}`);
    }
  }
}

async function launchAttempt(bin, { idle }) {
  const app = startApp(bin);
  try {
    let log = "";
    const deadline = Date.now() + 30_000;
    for (;;) {
      log = app.read();
      if (/stage=presented epoch_ms=/.test(log) || /ui_error/.test(log)) break;
      const drawn = log.match(/stage=drawn epoch_ms=(\d+)/);
      if (drawn && Date.now() - Number(drawn[1]) > 3000) {
        occludedFallbacks++; // no animation frames (window occluded): use the "drawn" stamp
        break;
      }
      if (Date.now() > deadline) throw new Error(`timeout waiting for first-tile-painted; trace so far: ${JSON.stringify(log)}`);
      await sleep(5);
    }
    const err = log.match(/ui_error (.*)/);
    if (err) throw new Error(`ui_error: ${err[1]}`);
    const tPaint = Number((log.match(/stage=presented epoch_ms=(\d+)/) ?? log.match(/stage=drawn epoch_ms=(\d+)/))[1]);
    const mainStart = Number(log.match(/main_start epoch_ms=(\d+)/)?.[1] ?? NaN);
    const res = { launchMs: tPaint - app.t0, execToMainMs: mainStart - app.t0, mainToPaintMs: tPaint - mainStart };
    if (idle) {
      await sleep(idleSecs * 1000);
      const pids = appPids(app.pid(), app.baseline);
      if (process.env.MEASURE_DEBUG) console.error("pids", pids);
      res.memory = memoryBytes(pids);
    }
    return res;
  } finally {
    app.kill(app.pid());
    await sleep(1500);
  }
}

// ---- main -----------------------------------------------------------------------

async function main() {
  const bin = args.bin && resolve(args.bin);
  const result = { platform, arch: process.arch, node: process.version, time: new Date().toISOString() };

  if (existsSync(dist)) {
    const js = [];
    for (const f of readdirSync(join(dist, "assets"))) {
      if (extname(f) === ".js") js.push({ file: f, raw: statSync(join(dist, "assets", f)).size, gzip: gzipSync(readFileSync(join(dist, "assets", f))).length });
    }
    result.jsBundle = js;
  }

  if (args["bundle-dir"]) {
    const dir = resolve(args["bundle-dir"]);
    const walk = (d) => readdirSync(d, { withFileTypes: true }).flatMap((e) => (e.isDirectory() && !e.name.endsWith(".app") ? walk(join(d, e.name)) : [join(d, e.name)]));
    result.artifacts = walk(dir)
      .filter((p) => /\.(dmg|app|msi|exe|deb|rpm|flatpak)$/.test(p))
      .map((p) => ({ file: p.slice(dir.length + 1), bytes: dirSize(p) }));
  }

  if (bin) {
    result.binaryBytes = statSync(bin).size;
    if (platform === "darwin" && args.purge) {
      try {
        execFileSync("sudo", ["-n", "purge"], { stdio: "ignore" });
        result.purge = "sudo purge before each run";
      } catch {
        result.purge = "sudo purge unavailable";
      }
    }
    const launches = [];
    for (let i = 0; i < runs; i++) {
      if (result.purge === "sudo purge before each run") execFileSync("sudo", ["-n", "purge"], { stdio: "ignore" });
      launches.push(await launchOnce(bin, { idle: false }));
    }
    const ms = launches.map((l) => l.launchMs);
    result.launch = {
      runs: launches,
      firstMs: ms[0],
      medianMs: median(ms),
      p95Ms: p95(ms),
      minMs: Math.min(...ms),
      medianWarmMs: ms.length > 1 ? median(ms.slice(1)) : null,
      medianExecToMainMs: median(launches.map((l) => l.execToMainMs)),
    };
    const idleRuns = [];
    for (let i = 0; i < Number(args["idle-runs"] ?? 3); i++) idleRuns.push((await launchOnce(bin, { idle: true })).memory);
    if (idleRuns.length) result.idle = { secsAfterPaint: idleSecs, totalsBytes: idleRuns.map((m) => m.total), medianBytes: median(idleRuns.map((m) => m.total)), ownTotalsBytes: idleRuns.map((m) => m.ownTotal), medianOwnBytes: median(idleRuns.map((m) => m.ownTotal)), breakdownLastRun: idleRuns.at(-1).per, pssSplitLastRun: idleRuns.at(-1).pssSplit };
  }

  if (result.idle) {
    // Input for tools/check-budgets --memory. idle_mb is the gated figure:
    // Linux = own memory (anon + private dirty), macOS/Windows = the platform figure.
    const mb = (b) => Math.round((b / MB) * 10) / 10;
    result.memoryGate = { idle_mb: mb(result.idle.medianOwnBytes) };
    if (platform === "linux") (result.memoryGate.idle_anon_mb = mb(result.idle.medianOwnBytes), (result.memoryGate.idle_pss_mb = mb(result.idle.medianBytes)));
  }

  result.launchRetries = retries;
  result.occludedFallbacks = occludedFallbacks;
  result.helperAttributionByDelta = fallbackAttribution;
  if (args.json) writeFileSync(String(args.json), JSON.stringify(result, null, 2));
  console.log(summary(result));
}

function summary(r) {
  const L = [`## Shell measurements (${r.platform}/${r.arch})`];
  if (r.jsBundle) for (const j of r.jsBundle) L.push(`- JS ${j.file}: ${(j.raw / 1024).toFixed(1)} KB raw, ${(j.gzip / 1024).toFixed(1)} KB gzip`);
  for (const a of r.artifacts ?? []) L.push(`- artifact ${a.file}: ${(a.bytes / MB).toFixed(2)} MB`);
  if (r.binaryBytes) L.push(`- binary: ${(r.binaryBytes / MB).toFixed(2)} MB`);
  if (r.launch) {
    const l = r.launch;
    L.push(`- launch -> first tile (${l.runs.length} runs): first ${l.firstMs} ms, median ${l.medianMs} ms, p95 ${l.p95Ms} ms, min ${l.minMs} ms, median excluding first ${l.medianWarmMs} ms; spawn->main median ${l.medianExecToMainMs} ms`);
    L.push(`- launch samples (ms): ${l.runs.map((x) => x.launchMs).join(", ")}`);
  }
  if (r.launchRetries) L.push(`- launch retries after a missed paint event: ${r.launchRetries}`);
  if (r.occludedFallbacks) L.push(`- runs where no animation frame arrived (window occluded) and the 'drawn' stamp was used: ${r.occludedFallbacks}`);
  if (r.helperAttributionByDelta) L.push(`- WebKit helpers attributed by 'new since launch' (responsibility API gave none) in ${r.helperAttributionByDelta} idle runs; may include unrelated WebKit processes started meanwhile`);
  if (r.idle) {
    if (r.platform === "linux") {
      L.push(`- idle memory ${r.idle.secsAfterPaint}s after paint, all processes, GATED own memory (anon + private dirty): median ${(r.idle.medianOwnBytes / MB).toFixed(1)} MB (runs: ${r.idle.ownTotalsBytes.map((b) => (b / MB).toFixed(1)).join(", ")})`);
      L.push(`- idle PSS (informational, includes shared library pages): median ${(r.idle.medianBytes / MB).toFixed(1)} MB`);
    } else {
      L.push(`- idle memory ${r.idle.secsAfterPaint}s after paint, all processes: median ${(r.idle.medianBytes / MB).toFixed(1)} MB (runs: ${r.idle.totalsBytes.map((b) => (b / MB).toFixed(1)).join(", ")})`);
    }
    for (const [k, v] of Object.entries(r.idle.pssSplitLastRun ?? {})) L.push(`  - PSS split ${k}: ${v}`);
    for (const [k, v] of Object.entries(r.idle.breakdownLastRun)) L.push(`  - ${k}: ${(v / MB).toFixed(1)} MB`);
  }
  return L.join("\n");
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
