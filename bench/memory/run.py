#!/usr/bin/env python3
"""Run the large-document memory matrix (ARCHITECTURE 1.2 / Spike 0.2) one process at a time.

  run.py --memprobe PATH [--settle 10] [--out results.jsonl]

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
