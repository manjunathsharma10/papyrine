#!/usr/bin/env python3
"""Independent expectations for papyrine-model, computed from `qpdf --json=2` only.

Used by gen_curated.py to write tests/curated.toml. Nothing here shares code with the Rust model:
page-tree inheritance, outline walks, label rendering and string decoding are all re-implemented
over qpdf's JSON dump of the file.
"""
import json
import subprocess
import sys

KEYS = ["--json-key=qpdf", "--json-key=pages", "--json-key=pagelabels",
        "--json-key=acroform", "--json-key=encrypt"]


def run_qpdf(path, password=None):
    cmd = ["qpdf", "--json=2"] + KEYS
    if password is not None:
        cmd.append(f"--password={password}")
    cmd.append(path)
    r = subprocess.run(cmd, capture_output=True)
    if r.returncode not in (0, 3):  # 3 = warnings
        return None
    return json.loads(r.stdout)


class Doc:
    def __init__(self, path, password=None):
        self.path = path
        d = run_qpdf(path, password)
        if d is None:
            raise RuntimeError("qpdf failed")
        self.j = d
        self.objs = d["qpdf"][1]
        self.trailer = self.objs["trailer"]["value"]

    # ---- object access -----------------------------------------------------------------
    def deref(self, v):
        seen = 0
        while isinstance(v, str) and v.endswith(" R") and v[:-2].replace(" ", "").isdigit() and seen < 50:
            o = self.objs.get("obj:" + v)
            if o is None:
                return None
            v = o["value"] if "value" in o else o.get("stream", {}).get("dict")
            seen += 1
        return v

    def get(self, d, key):
        d = self.deref(d)
        if not isinstance(d, dict):
            return None
        return self.deref(d.get("/" + key))

    def inherited(self, node, key):
        for _ in range(64):
            d = self.deref(node)
            if not isinstance(d, dict):
                return None
            if "/" + key in d:
                return self.deref(d["/" + key])
            node = d.get("/Parent")
            if node is None:
                return None
        return None

    @staticmethod
    def text(v):
        if isinstance(v, str):
            if v.startswith("u:"):
                return v[2:]
            if v.startswith("b:"):
                return None
        return None

    def nums(self, a):
        a = self.deref(a)
        if not isinstance(a, list):
            return None
        out = []
        for x in a:
            x = self.deref(x)
            if isinstance(x, bool) or not isinstance(x, (int, float)):
                return None
            out.append(x)
        return out

    # ---- facts -------------------------------------------------------------------------
    def root(self):
        return self.deref(self.trailer["/Root"])

    def page_objs(self):
        return [p["object"] for p in self.j["pages"]]

    def page_facts(self, i):
        po = self.page_objs()[i]
        media = self.nums(self.inherited(po, "MediaBox")) if self.inherited(po, "MediaBox") is not None else None
        if media is not None and len(media) != 4:
            media = None
        if media:
            x0, y0, x1, y1 = media
            media = [min(x0, x1), min(y0, y1), max(x0, x1), max(y0, y1)]
        rot = self.inherited(po, "Rotate")
        rot = rot if isinstance(rot, (int, float)) else 0
        rot = int(rot) // 90 * 90 % 360
        crop = self.inherited(po, "CropBox")
        crop = self.nums(crop) if crop is not None else None
        if crop is not None and len(crop) == 4 and media:
            x0, y0, x1, y1 = crop
            c = [max(min(x0, x1), media[0]), max(min(y0, y1), media[1]),
                 min(max(x0, x1), media[2]), min(max(y0, y1), media[3])]
            crop = c if c[2] > c[0] and c[3] > c[1] else media
        else:
            crop = media
        return {"index": i, "media": media, "crop": crop, "rotate": rot}

    def label_for(self, i):
        lab = self.j["pages"][i].get("label")
        ranges = self.j["pagelabels"]
        if not ranges:
            return str(i + 1)
        cur = None
        for r in ranges:
            if r["index"] <= i:
                cur = r
        if cur is None:
            return str(i + 1)
        d = cur["label"]
        n = d.get("/St", 1) + (i - cur["index"])
        style = d.get("/S")
        prefix = d.get("/P", "")
        prefix = prefix[2:] if isinstance(prefix, str) and prefix.startswith("u:") else prefix
        return prefix + render_number(n, style)

    def outline_stats(self):
        root = self.root()
        ol = self.get(root, "Outlines")
        if not isinstance(ol, dict):
            return None
        total, top, titles = 0, 0, []
        seen = set()

        def walk(parent, depth):
            nonlocal total, top
            node = self.deref_ref(parent.get("/First")) if depth >= 0 else None
            while node is not None:
                ref, d = node
                if ref in seen or not isinstance(d, dict) or depth > 60:
                    return
                seen.add(ref)
                total += 1
                if depth == 0:
                    top += 1
                    if len(titles) < 3:
                        titles.append(self.text(self.deref(d.get("/Title"))) or "")
                walk(d, depth + 1)
                node = self.deref_ref(d.get("/Next"))

        walk(ol, 0)
        return {"total": total, "top": top, "titles": titles}

    def deref_ref(self, v):
        if isinstance(v, str) and v.endswith(" R"):
            d = self.deref(v)
            return (v, d) if d is not None else None
        return None

    def annot_hist(self, i):
        po = self.page_objs()[i]
        arr = self.get(po, "Annots")
        out = {}
        seen = set()
        if isinstance(arr, list):
            for a in arr:
                if isinstance(a, str) and a in seen:
                    continue
                seen.add(a if isinstance(a, str) else id(a))
                d = self.deref(a)
                if isinstance(d, dict):
                    st = d.get("/Subtype", "/?")
                    out[st[1:]] = out.get(st[1:], 0) + 1
        return out

    def form_facts(self):
        af = self.j["acroform"]
        if not af.get("hasacroform"):
            return None
        fields = af["fields"]
        names = [f["fullname"] for f in fields]
        objs = sorted(set(names))
        types = {}
        for f in fields:
            t = f["fieldtype"] or "?"
            if f["ischeckbox"]:
                t = "Checkbox"
            elif f["isradiobutton"]:
                t = "Radio"
            elif t == "/Btn":
                t = "PushButton"
            elif t == "/Tx":
                t = "Text"
            elif t == "/Ch":
                t = "Combo" if f["fieldflags"] & (1 << 17) else "List"
            elif t == "/Sig":
                t = "Signature"
            types[t] = types.get(t, 0) + 1
        first = []
        for f in fields:
            # qpdf lists widgets in object-number order, so sample by name, not by position;
            # names that occur once keep the widget/page unambiguous.
            if names.count(f["fullname"]) != 1 or f["fieldtype"] == "/Sig":
                continue
            v = f["value"]
            v = v[2:] if isinstance(v, str) and v.startswith("u:") else v
            first.append({"name": f["fullname"], "flags": f["fieldflags"], "value": v if isinstance(v, str) else None,
                          "page": f.get("pageposfrom1")})
            if len(first) == 3:
                break
        acro = self.get(self.root(), "AcroForm")
        return {
            "widgets": len(fields),
            "fields": len(objs),
            "need_appearances": bool(af.get("needappearances")),
            "types": types,
            "first": first,
            "has_xfa": isinstance(acro, dict) and "/XFA" in acro,
            "sig_flags": acro.get("/SigFlags", 0) if isinstance(acro, dict) else 0,
        }

    def signed_fields(self):
        n = 0
        af = self.j["acroform"]
        seen = set()
        for f in af.get("fields", []):
            if f["fieldtype"] == "/Sig" and f["object"] not in seen:
                seen.add(f["object"])
                v = self.get(f["object"], "V")
                if isinstance(v, dict) and ("/Contents" in v or "/ByteRange" in v):
                    n += 1
        return n

    def doc_js_count(self):
        names = self.get(self.get(self.root(), "Names"), "JavaScript")
        if not isinstance(names, dict):
            return 0
        return self.tree_count(names, "Names")

    def tree_count(self, node, key, depth=0, seen=None):
        seen = seen if seen is not None else set()
        d = self.deref(node)
        if not isinstance(d, dict) or depth > 30:
            return 0
        n = len(d.get("/" + key, [])) // 2
        for k in d.get("/Kids", []):
            if isinstance(k, str):
                if k in seen:
                    continue
                seen.add(k)
            n += self.tree_count(k, key, depth + 1, seen)
        return n

    def info(self):
        info = self.deref(self.trailer.get("/Info"))
        if not isinstance(info, dict):
            return {}
        out = {}
        for k in ("Title", "Author", "Subject", "Creator", "Producer"):
            t = self.text(self.deref(info.get("/" + k)))
            if t is not None:
                out[k.lower()] = t
        cd = self.text(self.deref(info.get("/CreationDate")))
        if cd:
            out["creation_date_raw"] = cd
        return out

    def encrypt(self):
        e = self.j["encrypt"]
        if not e["encrypted"]:
            return None
        p = e["parameters"]
        c = e["capabilities"]
        return {"r": p["R"], "v": p["V"], "method": e["parameters"].get("method", ""),
                "caps": c, "bits": p.get("P")}

    def catalog_flags(self):
        r = self.root()
        names = self.get(r, "Names")
        return {
            "has_outlines": isinstance(self.get(r, "Outlines"), dict) and self.get(self.get(r, "Outlines"), "First") is not None,
            "has_page_labels": bool(self.j["pagelabels"]),
            "has_acroform": self.get(r, "AcroForm") is not None,
            "has_embedded_files": isinstance(names, dict) and "/EmbeddedFiles" in names,
            "has_metadata": "/Metadata" in r,
            "lang": self.text(self.deref(r.get("/Lang"))),
            "catalog_version": (r.get("/Version") or "")[1:] or None,
            "marked": bool((self.get(r, "MarkInfo") or {}).get("/Marked")) if isinstance(self.get(r, "MarkInfo"), dict) else False,
        }


def render_number(n, style):
    if style is None:
        return ""
    if style == "/D":
        return str(n)
    if style in ("/R", "/r"):
        t = [(1000, "M"), (900, "CM"), (500, "D"), (400, "CD"), (100, "C"), (90, "XC"), (50, "L"),
             (40, "XL"), (10, "X"), (9, "IX"), (5, "V"), (4, "IV"), (1, "I")]
        s = ""
        x = n
        for v, r in t:
            while x >= v:
                s += r
                x -= v
        return s if style == "/R" else s.lower()
    if style in ("/A", "/a"):
        if n <= 0:
            return ""
        c = chr(ord("A") + (n - 1) % 26) * ((n - 1) // 26 + 1)
        return c if style == "/A" else c.lower()
    return ""


if __name__ == "__main__":
    d = Doc(sys.argv[1], sys.argv[2] if len(sys.argv) > 2 else None)
    print(json.dumps({
        "pages": len(d.page_objs()),
        "page0": d.page_facts(0),
        "label0": d.label_for(0),
        "outline": d.outline_stats(),
        "form": d.form_facts(),
        "info": d.info(),
        "encrypt": d.encrypt(),
        "catalog": d.catalog_flags(),
        "js": d.doc_js_count(),
    }, indent=1, default=str))
