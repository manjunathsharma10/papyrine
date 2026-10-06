#!/usr/bin/env python3
"""Run the large-document memory matrix (ARCHITECTURE 1.2 / Spike 0.2) one process at a time.

  run.py --memprobe PATH [--settle 10] [--out results.jsonl]
  run.py --memprobe PATH --role-exe PATH_TO_render_role --large-json large.json [--gate-only]

With --role-exe each file also runs the full ARCHITECTURE 1.2 script against the real renderer
role in a sandboxed child process (`memprobe gate`) and writes the rows `tools/check-budgets
--large-doc` reads. Build the role with
`cargo build --release -p papyrine-render --example render_role`.

Each memprobe run is a fresh process, so peaks are per-process and cannot leak between runs.
Prints a markdown table; the raw MEM lines go to --out.
"""
import argparse, json, os, subprocess, sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
GEN = os.path.join(ROOT, "corpus/cache/generated")
FILES = ["large-2000p-text", "large-1000p-images", "large-objects", "large-single-page"]

ap = argparse.ArgumentParser()
ap.add_argument("--memprobe", required=True)
ap.add_argument("--settle", default="10")
ap.add_argument("--out", default="memory-results.jsonl")
ap.add_argument("--files", nargs="*", default=FILES)
ap.add_argument("--role-exe", help="render_role example binary: also run the full 1.2 script")
ap.add_argument("--large-json", default="large.json")
ap.add_argument("--gate-only", action="store_true", help="skip the per-engine matrix")
ap.add_argument("--gate-args", nargs="*", default=[], help="extra args for memprobe gate")
a = ap.parse_args()

results = []


def run(label, args):
    p = subprocess.run([a.memprobe] + args, capture_output=True, text=True)
    line = next((l for l in p.stdout.splitlines() if l.startswith("MEM ")), None)
    if not line:
        print(f"{label}: FAILED rc={p.returncode} {p.stderr[-300:]}", file=sys.stderr)
        return None
    r = json.loads(line[4:])
    r["label"] = label
    r["name"] = os.path.basename(r.get("file", ""))
    results.append(r)
    with open(a.out, "a") as f:
        f.write(json.dumps(r) + "\n")
    print(label, line[4:200], file=sys.stderr)
    return r


def gate(name):
    f = os.path.join(GEN, name + ".pdf")
    p = subprocess.run([a.memprobe, "gate", f, "--role-exe", a.role_exe, "--settle", a.settle] + a.gate_args,
                       capture_output=True, text=True)
    line = next((l for l in p.stdout.splitlines() if l.startswith("GATE ")), None)
    if not line:
        print(f"gate {name}: FAILED rc={p.returncode} {p.stderr[-400:]}", file=sys.stderr)
        return None
    r = json.loads(line[5:])
    with open(a.out, "a") as fh:
        fh.write(json.dumps({"label": f"gate {name}", **r}) + "\n")
    print("gate", name, line[5:240], file=sys.stderr)
    return r


if a.role_exe:
    rows, out = [], []
    for name in a.files:
        r = gate(name)
        if r:
            out.append(r)
            rows.append({"file": name + ".pdf", "peak_mb": r["peak_mb"], "settled_mb": r["settled_mb"],
                         "engine_mb": r["engine_settled_mb"], "renderer_mb": r["renderer_settled_mb"]})
    with open(a.large_json, "w") as fh:
        json.dump(rows, fh, indent=1)
    print("\n| file | renderer peak | renderer settled | engine peak | engine settled | whole-app peak* | whole-app settled* | first tile ms | search s | edit p95 ms |")
    print("|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|")
    for r in out:
        print(f"| {r['file']} | {r['renderer_peak_mb']} | {r['renderer_settled_mb']} | {r['engine_peak_mb']} | "
              f"{r['engine_settled_mb']} | {r['peak_mb']} | {r['settled_mb']} | {r['first_tile_ms']} | "
              f"{r['search_s']} | {r['edit_tile_ms_p95']} |")
    print("\n* adds the shell/webview constant (Spike 0.1) and the host tile-cache estimate; not measured here.")
    if a.gate_only:
        sys.exit(0)

run("baseline", ["baseline"])
for name in a.files:
    f = os.path.join(GEN, name + ".pdf")
    run(f"qpdf mmap {name}", ["cos", f, "--settle", a.settle])
    run(f"qpdf path {name}", ["cos", f, "--path", "--settle", a.settle])
    run(f"pdfium {name}", ["pdfium", f, "--settle", a.settle])
    if name == "large-2000p-text":
        run(f"pdfium+text {name}", ["pdfium", f, "--text", "--settle", a.settle])
    if name in ("large-2000p-text", "large-1000p-images"):
        for n in ("5", "50"):
            run(f"pdfium reopen/{n} {name}", ["pdfium", f, "--reopen-every", n, "--settle", a.settle])
    if name.startswith("large-objects"):
        run(f"qpdf mmap --all {name}", ["cos", f, "--all", "--settle", a.settle])
    run(f"qpdf lazy {name}", ["cos-lazy", f, "--iters", "5"])
