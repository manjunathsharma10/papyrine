#!/usr/bin/env python3
"""Summarise compare.py output as markdown tables. usage: summarize.py results.tsv"""
import sys, csv, collections, statistics

rows = list(csv.DictReader(open(sys.argv[1]), delimiter="\t"))
by = collections.defaultdict(dict)  # file -> lib -> row
for r in rows:
    by[r["file"]][r["lib"]] = r
LIBS = ["qpdf", "lopdf"]


def cls(r):
    s = r["status"]
    if s == "ok":
        return "open"
    if s == "ok-no-pages":
        return "open, 0 pages"
    if s.startswith("err"):
        return "typed error"
    return "CRASH/HANG (" + s + ")"


def tagsof(f):
    return by[f]["qpdf"]["tags"].split(",")


def table(title, pred):
    fs = [f for f in by if pred(f)]
    print(f"\n**{title}** ({len(fs)} files)\n")
    print("| outcome | " + " | ".join(LIBS) + " |")
    print("|---|" + "---:|" * len(LIBS))
    keys = sorted({cls(by[f][l]) for f in fs for l in LIBS})
    for k in keys:
        print(f"| {k} | " + " | ".join(str(sum(1 for f in fs if cls(by[f][l]) == k)) for l in LIBS) + " |")


table("All files", lambda f: True)
for t in ["malformed", "unreadable", "encrypted", "password", "forms", "xfa", "signed", "pdfa", "large"]:
    table(f"Tag `{t}`", lambda f, t=t: t in tagsof(f))
table("Synthetic malformed (`mal-*`)", lambda f: "gen-mal" in tagsof(f))

# Repair quality: page count against PDFium on files where qpdf or lopdf opened a malformed file.
print("\n**Page-count agreement with PDFium, files tagged `malformed`/`gen-mal` that PDFium opens**\n")
sel = [f for f in by if ("malformed" in tagsof(f) or "gen-mal" in tagsof(f)) and by[f]["pdfium"]["status"] == "ok"]
print(f"{len(sel)} files\n")
print("| | qpdf | lopdf |\n|---|---:|---:|")
for name, fn in [
    ("same page count as PDFium", lambda r, p: r["status"] == "ok" and r["pages"] == p["pages"]),
    ("different page count", lambda r, p: r["status"] == "ok" and r["pages"] != p["pages"]),
    ("opened, 0 pages", lambda r, p: r["status"] == "ok-no-pages"),
    ("error", lambda r, p: r["status"].startswith("err")),
]:
    print(f"| {name} | " + " | ".join(str(sum(1 for f in sel if fn(by[f][l], by[f]["pdfium"]))) for l in LIBS) + " |")

# Head to head.
both = [f for f in by]
q_only = [f for f in both if cls(by[f]["qpdf"]) == "open" and cls(by[f]["lopdf"]) != "open"]
l_only = [f for f in both if cls(by[f]["lopdf"]) == "open" and cls(by[f]["qpdf"]) != "open"]
print(f"\nOpened by qpdf but not lopdf: {len(q_only)}; opened by lopdf but not qpdf: {len(l_only)}")
if "-v" in sys.argv:
    for f in l_only:
        print("  lopdf-only:", f, by[f]["qpdf"]["status"], by[f]["qpdf"]["detail"][:80])

# Page-count disagreements where both open.
dis = [f for f in both if by[f]["qpdf"]["status"] == "ok" and by[f]["lopdf"]["status"] == "ok" and by[f]["qpdf"]["pages"] != by[f]["lopdf"]["pages"]]
print(f"Both open but page counts differ: {len(dis)}")
for f in dis[:12]:
    print(f"  {f}: qpdf {by[f]['qpdf']['pages']} lopdf {by[f]['lopdf']['pages']} pdfium {by[f]['pdfium']['pages']}")

# Resources.
def num(x):
    try:
        return float(x)
    except ValueError:
        return None

print("\n**Peak RSS (MB) and time over all files that open in that library**\n")
print("| | qpdf | lopdf | pdfium (ref) |\n|---|---:|---:|---:|")
for name, col, fn in [("median RSS", "rss_mb", statistics.median), ("p95 RSS", "rss_mb", None), ("median wall ms", "wall_ms", statistics.median), ("p95 wall ms", "wall_ms", None)]:
    cells = []
    for l in LIBS + ["pdfium"]:
        v = sorted(x for r in rows if r["lib"] == l and r["status"] == "ok" for x in [num(r[col])] if x is not None)
        cells.append(f"{(fn(v) if fn else v[int(len(v) * 0.95)]):.1f}")
    print(f"| {name} | " + " | ".join(cells) + " |")

print("\n**Generated large files (single run each, 4 parallel jobs: timings are noisy)**\n")
print("| file | lib | status | pages | objects | open ms | first-object ms | wall ms | peak RSS MB |\n|---|---|---|---:|---:|---:|---:|---:|---:|")
for f in sorted(by):
    if "gen-large" in tagsof(f) or f.endswith("typical-20p.pdf"):
        for l in LIBS + ["pdfium"]:
            r = by[f][l]
            print(f"| {f.split('/')[-1]} | {l} | {r['status']} | {r['pages']} | {r['objects']} | {r['open_ms']} | {r['first_ms']} | {r['wall_ms']} | {r['rss_mb']} |")
