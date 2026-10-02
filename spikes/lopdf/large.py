#!/usr/bin/env python3
"""Spike 0.3, large generated files, one process at a time (no parallel noise), 3 repetitions.
usage: large.py --lopdf PATH --memprobe PATH   (prints a markdown table of medians)"""
import argparse, os, statistics, sys
sys.path.insert(0, os.path.dirname(__file__))
from compare import run_one, ROOT

ap = argparse.ArgumentParser()
ap.add_argument("--lopdf", required=True)
ap.add_argument("--memprobe", required=True)
ap.add_argument("--reps", type=int, default=3)
a = ap.parse_args()
gen = os.path.join(ROOT, "corpus/cache/generated")
files = ["typical-20p", "large-2000p-text", "large-1000p-images", "large-single-page", "large-objects", "large-objects-objstm"]
cfgs = [
    ("qpdf open + page tree", [a.memprobe, "cos-open", "{f}", "--no-objects"]),
    ("qpdf + object count", [a.memprobe, "cos-open", "{f}"]),
    ("lopdf load + page tree", [a.lopdf, "{f}"]),
    ("pdfium open (ref)", [a.memprobe, "pdfium-open", "{f}"]),
]
print("| file | library / work | pages | objects | in-process open ms | first-object ms | wall ms (median) | peak RSS MB (median) |")
print("|---|---|---:|---:|---:|---:|---:|---:|")
for f in files:
    p = os.path.join(gen, f + ".pdf")
    if not os.path.exists(p):
        continue
    for name, cmd in cfgs:
        res = []
        for _ in range(a.reps):
            st, out, wall, rss = run_one([c.replace("{f}", p) for c in cmd], 120)
            line = next((l for l in out.splitlines() if l.startswith("RESULT\t")), "RESULT\t?\t-\t-\t-\t-\t").split("\t")
            res.append((wall, rss, line))
        wall = statistics.median(r[0] for r in res)
        rss = statistics.median(r[1] for r in res)
        l = res[0][2]
        print(f"| {f} | {name} | {l[2]} | {l[3]} | {l[4]} | {l[5]} | {int(wall)} | {rss} |")
