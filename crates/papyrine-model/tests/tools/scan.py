#!/usr/bin/env python3
"""Feature table over the corpus, to pick curated files: python3 scan.py > /tmp/scan.tsv"""
import os, sys, json
from concurrent.futures import ThreadPoolExecutor
sys.path.insert(0, os.path.dirname(__file__))
from qpdf_facts import Doc

ROOT = os.path.join(os.path.dirname(__file__), "../../../../corpus/cache")
ROOT = os.path.abspath(ROOT)


def files():
    for sub in ("files", "generated"):
        for dp, _, fs in os.walk(os.path.join(ROOT, sub)):
            for f in sorted(fs):
                if f.endswith(".pdf"):
                    p = os.path.join(dp, f)
                    if os.path.getsize(p) < 1_500_000 and not f.startswith(("mal-", "large-")):
                        yield p


def feats(p):
    try:
        d = Doc(p)
    except Exception:
        return None
    try:
        n = len(d.page_objs())
        cf = d.catalog_flags()
        form = d.form_facts()
        ol = d.outline_stats()
        f0 = d.page_facts(0) if n else None
        rot = {d.page_facts(i)["rotate"] for i in range(min(n, 30))}
        an = {}
        for i in range(min(n, 5)):
            for k, v in d.annot_hist(i).items():
                an[k] = an.get(k, 0) + v
        return dict(id=os.path.relpath(p, ROOT), pages=n, labels=cf["has_page_labels"],
                    outline=(ol or {}).get("total", 0), widgets=(form or {}).get("widgets", 0),
                    types=(form or {}).get("types"), xfa=(form or {}).get("has_xfa"),
                    sigs=d.signed_fields(), js=d.doc_js_count(), rot=sorted(rot),
                    crop=(f0 and f0["crop"] != f0["media"]), annots=an,
                    enc=bool(d.encrypt()), ver=cf["catalog_version"], lang=cf["lang"])
    except Exception as e:
        return dict(id=os.path.relpath(p, ROOT), error=repr(e))


if __name__ == "__main__":
    with ThreadPoolExecutor(8) as ex:
        for r in ex.map(feats, files()):
            if r:
                print(json.dumps(r, default=str))
