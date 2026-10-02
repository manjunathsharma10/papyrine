#!/usr/bin/env python3
"""Aggregate Spike 0.4 results.

usage: analyze_corpus.py SCRIPTS.jsonl VERDICTS.jsonl FETCH_META.json [--json OUT]
 SCRIPTS.jsonl   from extract_scripts.py
 VERDICTS.jsonl  from `classify` run on the "script" records (ids = line index in SCRIPTS.jsonl)
"""
import collections, json, sys

scripts_path, verdicts_path, meta_path = sys.argv[1:4]
meta = json.load(open(meta_path))
raw = [json.loads(l) for l in open(scripts_path)]
ver = {}
for l in open(verdicts_path):
    d = json.loads(l)
    if "id" in d: ver[d["id"]] = d

forms = {}  # id -> dict
for i, r in enumerate(raw):
    f = forms.setdefault(r["form"], {"scripts": [], "error": None, "nonjs": 0})
    if "error" in r: f["error"] = r["error"]
    elif "script" in r:
        v = ver[str(i)]
        f["scripts"].append({"trigger": r["trigger"], "field": r.get("field", ""), "verdict": v["verdict"],
                             "blocker": v["blocker"], "detail": v["detail"], "script": r["script"]})
    elif "nonjs_action" in r: f["nonjs"] += 1

def source_of(fid):
    m = meta.get(fid)
    return m["source"] if m else "?"

def group(src):  # coarse grouping for the headline tables
    if src == "pdfjs-corpus": return "pdf.js corpus"
    if src == "gov-uk": return "UK gov.uk"
    if src in ("irs",): return "IRS"
    if src in ("ca", "ny", "fl", "tx", "il", "or", "ok", "pa", "nj", "oh", "va-state", "co", "mn", "ma", "ga", "mi", "wi", "nc", "az", "ky", "md", "ut", "ia", "wa"): return "US states"
    if src in ("ircc", "cra"): return "Canada"
    if src in ("homeaffairs", "ato"): return "Australia"
    return "US federal (non-IRS)"

COVERED = ("empty", "accepted", "boilerplate")
out = {"forms_total": len(forms)}
js_forms = {}
for fid, f in forms.items():
    nonempty = [s for s in f["scripts"] if s["script"].strip()]
    if nonempty: js_forms[fid] = f
out["js_forms"] = len(js_forms)
out["errors"] = sum(1 for f in forms.values() if f["error"])

def covered(f, allow_benign=True):
    for s in f["scripts"]:
        if s["verdict"] in COVERED: continue
        return False
    return True

def stats(subset):
    n = len(subset)
    c = sum(1 for f in subset.values() if covered(f))
    sc = [s for f in subset.values() for s in f["scripts"] if s["script"].strip()]
    ac = sum(1 for s in sc if s["verdict"] in COVERED)
    pure = sum(1 for s in sc if s["verdict"] == "accepted")
    bp = sum(1 for s in sc if s["verdict"] == "boilerplate")
    # strict-only: forms covered with no boilerplate recognizer
    strict = sum(1 for f in subset.values() if all(s["verdict"] in ("empty", "accepted") or not s["script"].strip() for s in f["scripts"]))
    return {"forms": n, "covered": c, "strict_only": strict, "scripts": len(sc), "scripts_covered": ac, "scripts_pure_af": pure, "scripts_boilerplate": bp}

out["overall"] = stats(js_forms)
by_group = collections.defaultdict(dict)
by_source = collections.defaultdict(dict)
for fid, f in js_forms.items():
    by_group[group(source_of(fid))][fid] = f
    by_source[source_of(fid)][fid] = f
out["by_group"] = {g: stats(s) for g, s in sorted(by_group.items())}
out["by_source"] = {g: stats(s) for g, s in sorted(by_source.items())}
real = {k: v for k, v in js_forms.items() if group(source_of(k)) != "pdf.js corpus"}
out["real_world"] = stats(real)
out["real_world_excl_irs"] = stats({k: v for k, v in real.items() if source_of(k) != "irs"})
out["excl_irs_all"] = stats({k: v for k, v in js_forms.items() if source_of(k) != "irs"})

# blockers
bscripts = collections.Counter(); bforms = collections.defaultdict(set)
for fid, f in real.items():
    for s in f["scripts"]:
        if s["verdict"] == "rejected":
            bscripts[s["blocker"]] += 1; bforms[s["blocker"]].add(fid)
out["blockers_real"] = {b: {"scripts": bscripts[b], "forms": len(bforms[b])} for b in bscripts}
# what-if: forms that become covered if one blocker category were implemented
whatif = {}
for b in bscripts:
    whatif[b] = sum(1 for fid, f in real.items() if not covered(f) and all(s["verdict"] in COVERED or s["blocker"] == b for s in f["scripts"]))
out["unlock_if_supported"] = whatif
# partial coverage
fr = []
for fid, f in real.items():
    sc = [s for s in f["scripts"] if s["script"].strip()]
    if sc: fr.append(sum(1 for s in sc if s["verdict"] in COVERED) / len(sc))
out["real_forms_ge_90pct_scripts"] = sum(1 for x in fr if x >= 0.9)
out["real_forms_ge_50pct_scripts"] = sum(1 for x in fr if x >= 0.5)
# trigger breakdown of accepted/rejected
trig = collections.defaultdict(collections.Counter)
for f in real.values():
    for s in f["scripts"]:
        if s["script"].strip(): trig[s["trigger"]][s["verdict"]] += 1
out["by_trigger_real"] = {t: dict(c) for t, c in trig.items()}
# AF function usage among accepted scripts (from details unavailable) -> count rejected AF names later
unk = collections.Counter()
for f in real.values():
    for s in f["scripts"]:
        if s["verdict"] == "rejected" and s["blocker"].startswith("AF-function-not"):
            unk[s["detail"]] += 1
out["af_not_allowlisted"] = unk.most_common(15)
bad_args = collections.Counter()
for f in real.values():
    for s in f["scripts"]:
        if s["verdict"] == "rejected" and s["blocker"] == "AF-call-bad-arguments":
            bad_args[s["detail"]] += 1
out["af_bad_arguments"] = bad_args.most_common(15)
bp = collections.Counter()
for f in real.values():
    for s in f["scripts"]:
        if s["verdict"] == "boilerplate": bp[s["detail"]] += 1
out["boilerplate_hits"] = dict(bp)
# worst offenders
blk_forms = []
for fid, f in real.items():
    if not covered(f):
        rej = [s for s in f["scripts"] if s["verdict"] == "rejected"]
        cats = collections.Counter(s["blocker"] for s in rej)
        blk_forms.append((len(rej), fid, dict(cats)))
blk_forms.sort(reverse=True)
out["top_uncovered_forms"] = blk_forms[:15]
out["uncovered_forms"] = {fid: cats for _, fid, cats in blk_forms}

if "--json" in sys.argv:
    json.dump(out, open(sys.argv[sys.argv.index("--json") + 1], "w"), indent=1)
print(json.dumps(out, indent=1))
