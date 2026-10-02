#!/usr/bin/env python3
"""Extract every JavaScript action from PDFs using the qpdf CLI (test-time tool only).

usage: extract_scripts.py OUT.jsonl FILE.pdf...
Each output line: {"form": id, "trigger": "field.K"|"doc.name"|..., "field": name, "script": text}
Files with no scripts still emit {"form": id, "none": true}; unreadable files emit {"form": id, "error": msg}.
"""
import base64, json, os, re, subprocess, sys
from concurrent.futures import ThreadPoolExecutor

REF = re.compile(r"^(\d+) 0 R$")

def decode_bytes(b):
    if b[:2] == b"\xfe\xff": return b[2:].decode("utf-16-be", "replace")
    if b[:2] == b"\xff\xfe": return b[2:].decode("utf-16-le", "replace")
    if b[:3] == b"\xef\xbb\xbf": return b[3:].decode("utf-8", "replace")
    try: return b.decode("utf-8")
    except UnicodeDecodeError: return b.decode("latin-1")

def pdf_string(s):
    if s.startswith("u:"): return s[2:]
    if s.startswith("b:"): return decode_bytes(bytes.fromhex(s[2:]))
    return None

class Doc:
    def __init__(self, qpdf):
        self.objs = qpdf[1]
        self.trailer = self.objs.get("trailer", {}).get("value", {})

    def get(self, v):
        """Resolve a reference; return python value (dict for dict/stream dict)."""
        seen = 0
        while isinstance(v, str) and REF.match(v) and seen < 20:
            o = self.objs.get("obj:" + v)
            if o is None: return None
            if "stream" in o: return {"__stream__": o["stream"]}
            v = o.get("value")
            seen += 1
        return v

    def dictof(self, v):
        v = self.get(v)
        if isinstance(v, dict) and "__stream__" in v: return v["__stream__"].get("dict", {})
        return v if isinstance(v, dict) else None

    def js_text(self, v):
        v = self.get(v)
        if isinstance(v, str):
            s = pdf_string(v)
            return s
        if isinstance(v, dict) and "__stream__" in v:
            data = v["__stream__"].get("data")
            if data is None: return None
            return decode_bytes(base64.b64decode(data))
        return None

def walk_actions(doc, a, trigger, field, out, seen, depth=0):
    """a: action dict or ref (follow /Next)."""
    if depth > 50: return
    if isinstance(a, list):
        for x in a: walk_actions(doc, x, trigger, field, out, seen, depth + 1)
        return
    key = a if isinstance(a, str) and REF.match(a) else None
    if key is not None:
        if key in seen: return
        seen.add(key)
    d = doc.dictof(a)
    if not d: return
    if d.get("/S") == "/JavaScript":
        txt = doc.js_text(d.get("/JS"))
        out.append({"trigger": trigger, "field": field, "script": txt if txt is not None else "", "action": key})
    elif d.get("/S") in ("/SubmitForm", "/ImportData", "/ResetForm", "/Launch", "/URI", "/GoTo", "/GoToR", "/Named", "/Hide", "/SetOCGState", "/Rendition", "/Movie", "/Sound", "/Thread"):
        out.append({"trigger": trigger, "field": field, "nonjs_action": d.get("/S"), "action": key})
    if "/Next" in d: walk_actions(doc, d["/Next"], trigger, field, out, seen, depth + 1)

FIELD_AA = {"/K": "K", "/F": "F", "/V": "V", "/C": "C"}

def extract(path):
    r = subprocess.run(["qpdf", "--json=2", "--json-key=qpdf", "--json-stream-data=inline", "--password=", path],
                       capture_output=True, timeout=180)
    if r.returncode not in (0, 3):  # 3 = warnings
        return [{"error": (r.stderr.decode("utf-8", "replace").strip().splitlines() or ["qpdf failed"])[0][:200]}]
    try:
        j = json.loads(r.stdout)
    except Exception as e:
        return [{"error": "json: %s" % e}]
    doc = Doc(j["qpdf"])
    out, seen = [], set()
    root = doc.dictof(doc.trailer.get("/Root")) or {}
    # document-level JavaScript name tree
    names = doc.dictof(root.get("/Names")) or {}
    def tree(n, depth=0):
        n = doc.dictof(n)
        if not n or depth > 30: return
        arr = doc.get(n.get("/Names"))
        if isinstance(arr, list):
            for i in range(0, len(arr) - 1, 2):
                nm = pdf_string(arr[i]) if isinstance(arr[i], str) else None
                walk_actions(doc, arr[i + 1], "doc.name", nm or "", out, seen)
        for k in doc.get(n.get("/Kids")) or []: tree(k, depth + 1)
    if "/JavaScript" in names: tree(names["/JavaScript"])
    if "/OpenAction" in root:
        oa = root["/OpenAction"]
        if not isinstance(doc.get(oa), list): walk_actions(doc, oa, "doc.OpenAction", "", out, seen)
    for k, v in (doc.dictof(root.get("/AA")) or {}).items(): walk_actions(doc, v, "doc.AA" + k, "", out, seen)

    # everything else: scan all dicts for /A and /AA
    for key, o in doc.objs.items():
        if not key.startswith("obj:"): continue
        d = o.get("value") if "value" in o else o.get("stream", {}).get("dict")
        if not isinstance(d, dict): continue
        typ = d.get("/Type")
        is_field = any(k in d for k in ("/FT", "/T")) or ("/Parent" in d and d.get("/Subtype") == "/Widget")
        fname = pdf_string(d["/T"]) if isinstance(d.get("/T"), str) and d["/T"][:2] in ("u:", "b:") else ""
        if "/A" in d and d.get("/Subtype") in ("/Widget", "/Link", "/Screen", "/Annot", None) and typ != "/Catalog":
            kind = "widget.A" if d.get("/Subtype") == "/Widget" else ("link.A" if d.get("/Subtype") == "/Link" else "annot.A")
            walk_actions(doc, d["/A"], kind, fname, out, seen)
        aa = doc.dictof(d.get("/AA")) if "/AA" in d else None
        if aa:
            for k, v in aa.items():
                if typ == "/Page": trig = "page.AA" + k
                elif typ == "/Catalog": continue
                elif is_field and k in FIELD_AA: trig = "field." + FIELD_AA[k]
                else: trig = ("widget.AA" if d.get("/Subtype") == "/Widget" else "annot.AA") + k
                walk_actions(doc, v, trig, fname, out, seen)
    # any JavaScript action nobody pointed to
    for key, o in doc.objs.items():
        if not key.startswith("obj:") or "value" not in o: continue
        d = o["value"]
        if isinstance(d, dict) and d.get("/S") == "/JavaScript":
            ref = key[4:]
            if ref not in seen:
                seen.add(ref)
                txt = doc.js_text(d.get("/JS"))
                out.append({"trigger": "unattached", "field": "", "script": txt or "", "action": ref})
    return out

def main():
    out_path, files = sys.argv[1], sys.argv[2:]
    def work(f):
        fid = os.path.basename(f)[:-4]
        try: items = extract(f)
        except Exception as e: items = [{"error": "exc: %s" % e}]
        return fid, items
    with ThreadPoolExecutor(8) as ex, open(out_path, "w") as fo:
        for fid, items in ex.map(work, files):
            if not items: fo.write(json.dumps({"form": fid, "none": True}) + "\n")
            for it in items:
                it["form"] = fid
                fo.write(json.dumps(it, ensure_ascii=False) + "\n")

if __name__ == "__main__":
    main()
