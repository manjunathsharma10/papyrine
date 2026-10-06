import { type ChildProcess, execFileSync, spawn } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { test as base, expect, type Page } from "@playwright/test";

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(here, "../../../..");

function bridgeBinary(): string {
  const exe = process.platform === "win32" ? "papyrine-bridge.exe" : "papyrine-bridge";
  const candidates = [
    process.env.PAPYRINE_BRIDGE_BIN,
    join(repoRoot, "target/host/debug", exe),
    join(repoRoot, "target/debug", exe),
  ].filter((p): p is string => !!p);
  const found = candidates.find((p) => existsSync(p));
  if (!found) throw new Error(`papyrine-bridge not built; looked in ${candidates.join(", ")}`);
  return found;
}

/** A running `papyrine-bridge` process. */
export class Bridge {
  constructor(
    public proc: ChildProcess,
    public url: string,
    public dataDir: string,
  ) {}

  kill9(): Promise<void> {
    return new Promise((res) => {
      this.proc.once("exit", () => res());
      this.proc.kill("SIGKILL");
    });
  }
}

export async function startBridge(dataDir: string): Promise<Bridge> {
  const proc = spawn(bridgeBinary(), ["--data-dir", dataDir], { stdio: ["ignore", "pipe", "inherit"] });
  const url = await new Promise<string>((res, rej) => {
    let buf = "";
    const t = setTimeout(() => rej(new Error("bridge did not announce itself")), 30_000);
    proc.stdout!.on("data", (d: Buffer) => {
      buf += d.toString();
      const m = /PAPYRINE_BRIDGE (ws:\/\/\S+)/.exec(buf);
      if (m) {
        clearTimeout(t);
        res(m[1]!);
      }
    });
    proc.once("exit", (c) => rej(new Error(`bridge exited early (${c})`)));
  });
  return new Bridge(proc, url, dataDir);
}

/** A valid PDF with `pages` pages, each carrying its number and a line of text. */
export function buildPdf(pages: number): Buffer {
  const chunks: string[] = ["%PDF-1.4\n%\xE2\xE3\xCF\xD3\n"];
  const offsets: number[] = [];
  let len = chunks[0]!.length;
  const obj = (body: string) => {
    offsets.push(len);
    const s = `${offsets.length} 0 obj\n${body}\nendobj\n`;
    chunks.push(s);
    len += s.length;
  };
  obj("<< /Type /Catalog /Pages 2 0 R >>");
  obj(`<< /Type /Pages /Count ${pages} /Kids [${Array.from({ length: pages }, (_, i) => `${4 + 2 * i} 0 R`).join(" ")}] >>`);
  obj("<< /Producer (e2e) /Title (E2E document) >>");
  for (let i = 0; i < pages; i++) {
    const content = `BT /F1 48 Tf 72 700 Td (Page ${i + 1}) Tj ET\nBT /F1 14 Tf 72 640 Td (The quick brown fox jumps over the lazy dog) Tj ET\n`;
    obj(
      `<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents ${5 + 2 * i} 0 R /Resources << /Font << /F1 << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> >> >> >>`,
    );
    obj(`<< /Length ${content.length} >>\nstream\n${content}endstream`);
  }
  const n = offsets.length + 1;
  let x = `xref\n0 ${n}\n0000000000 65535 f \n`;
  for (const o of offsets) x += `${String(o).padStart(10, "0")} 00000 n \n`;
  x += `trailer\n<< /Size ${n} /Root 1 0 R /Info 3 0 R /ID [<00112233445566778899aabbccddeeff> <00112233445566778899aabbccddeeff>] >>\nstartxref\n${len}\n%%EOF\n`;
  chunks.push(x);
  return Buffer.from(chunks.join(""), "latin1");
}

export function qpdfCheck(path: string): void {
  try {
    execFileSync("qpdf", ["--check", path], { stdio: "pipe" });
  } catch (e) {
    const err = e as { code?: string; stdout?: Buffer; stderr?: Buffer };
    if (err.code === "ENOENT") return; // qpdf is a test tool; skip when absent
    throw new Error(`qpdf --check failed: ${err.stdout?.toString()}${err.stderr?.toString()}`);
  }
}

export interface Fixtures {
  dataDir: string;
  workDir: string;
  bridge: Bridge;
  /** Open the app on `bridge` and wait for the welcome screen. */
  openApp(page: Page, bridge: Bridge): Promise<void>;
}

export const test = base.extend<Fixtures>({
  dataDir: async ({}, use) => {
    const d = mkdtempSync(join(tmpdir(), "papyrine-e2e-data-"));
    // The first-run privacy question is a UI concern; answer it so it does not cover the app.
    writeFileSync(join(d, "prefs.json"), JSON.stringify({ updateCheck: "off" }));
    await use(d);
  },
  workDir: async ({}, use) => use(mkdtempSync(join(tmpdir(), "papyrine-e2e-work-"))),
  bridge: async ({ dataDir }, use) => {
    const b = await startBridge(dataDir);
    await use(b);
    if (b.proc.exitCode === null) b.proc.kill("SIGKILL");
  },
  openApp: async ({}, use) =>
    use(async (page, bridge) => {
      await page.addInitScript(() => {
        try {
          localStorage.clear();
        } catch {
          /* ignore */
        }
      });
      await page.goto(`/?bridge=${encodeURIComponent(bridge.url)}`);
      await expect(page.getByTestId("welcome")).toBeVisible();
    }),
});

export { expect, readFileSync, writeFileSync };

/** Call the host through the page (the UI exposes it on `window.__papyrine` in dev). */
export function hostCall<T>(page: Page, method: string, ...args: unknown[]): Promise<T> {
  return page.evaluate(
    ([m, a]) => {
      const host = (window as unknown as { __papyrine: { host: Record<string, (...x: unknown[]) => unknown> } }).__papyrine.host;
      return (host[m] as (...x: unknown[]) => Promise<unknown>)(...(a as unknown[]));
    },
    [method, args] as const,
  ) as Promise<T>;
}

/** Dev-only bridge commands (`dev.killEngine`, `dev.queueOpen`, ...). */
export function dev<T = unknown>(page: Page, method: string, ...args: unknown[]): Promise<T> {
  return page.evaluate(
    ([m, a]) => {
      const host = (window as unknown as { __papyrine: { host: { dev: (m: string, ...x: unknown[]) => Promise<unknown> } } }).__papyrine.host;
      return host.dev(m as string, ...(a as unknown[]));
    },
    [method, args] as const,
  ) as Promise<T>;
}
