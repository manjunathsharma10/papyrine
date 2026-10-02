#!/usr/bin/env python3
"""Rebuild corpus/manifest.toml from pinned upstream trees (maintainer tool).

Needs: gh (authenticated, to list git trees), qpdf (classification only, test-time
tool), network. Downloads into corpus/cache/files, hashes, classifies with
`qpdf --check` and a byte scan, and rewrites the manifest.

    python3 tools/gen-corpus/scripts/build_manifest.py [--pins pins.json]

Upstream refs below are fixed commit SHAs, so reruns reproduce the same manifest. To
move a pin, replace the SHA, rerun and review the diff. IRS forms are pinned through
Wayback Machine raw ("id_") snapshots, resolved once and written into the URL.
"""
import concurrent.futures as cf
import hashlib
import json
import os
import re
import subprocess
import sys
import threading
import time
import urllib.parse
import urllib.request

REPO = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", "..", ".."))
CACHE = os.path.join(REPO, "corpus", "cache", "files")
OUT = os.path.join(REPO, "corpus", "manifest.toml")
MAXSIZE = 3_000_000

def ssl_context():
    """Default context; python.org macOS builds ship without CA certs, so fall back to system bundles."""
    import ssl

    ctx = ssl.create_default_context()
    if ctx.get_ca_certs():
        return ctx
    for cafile in ("/etc/ssl/cert.pem", "/private/etc/ssl/cert.pem", "/opt/homebrew/etc/ca-certificates/cert.pem",
                   "/etc/ssl/certs/ca-certificates.crt", "/etc/pki/tls/certs/ca-bundle.crt"):
        if os.path.exists(cafile):
            return ssl.create_default_context(cafile=cafile)
    return ctx


SSL_CTX = ssl_context()

# prefix, github repo, ref, path filter(regex on path), license, source tag, extra tags
SOURCES = [
    ("pdfjs", "mozilla/pdf.js", "c33c32aed46637ec6010e9f6c031c72e1c529d31", r"^test/pdfs/.*\.pdf$", "Apache-2.0", ["pdfjs"]),
    ("qpdf", "qpdf/qpdf", "4eba95899886e851cc41d76886483b347612f2a8", r"\.pdf$", "Apache-2.0", ["qpdf"]),
    ("pdfium", "chromium/pdfium", "a84323421e94f484faca52dd9d027934eba42ab8", r"^testing/resources/.*\.pdf$", "BSD-3-Clause", ["pdfium"]),
    ("pdfbox", "apache/pdfbox", "c4d556abc9d5f0cbc486d459c84682321dd38ff4", r"\.pdf$", "Apache-2.0", ["pdfbox"]),
    ("pypdf", "py-pdf/pypdf", "36047f40f9ee27b690d6c006e115ea8bcb4cb75d", r"^resources/.*\.pdf$", "BSD-3-Clause", ["pypdf"]),
    ("pdfminer", "pdfminer/pdfminer.six", "a18de2a9c479b4c847538500017b449ddaec177e", r"^samples/.*\.pdf$", "MIT", ["pdfminer"]),
    ("pdfplumber", "jsvine/pdfplumber", "4c64b92d5caccd71c645e98e0fabb0c4dba7ff45", r"\.pdf$", "MIT", ["pdfplumber"]),
    ("pikepdf", "pikepdf/pikepdf", "f11f1380edc04d9709a28235683aa05d1b5d0afc", r"\.pdf$", "MPL-2.0", ["pikepdf"]),
    ("lopdf", "J-F-Liu/lopdf", "c8dd1790ef6f89c4ea28da40653e627e57355f74", r"\.pdf$", "MIT", ["lopdf"]),
    ("pdfdiff", "pdf-association/pdf-differences", "907fe96e52b73e491489eee545c47b119bf9989b", r"\.pdf$", "Apache-2.0", ["pdfdiff"]),
]
VERAPDF = ("verapdf", "veraPDF/veraPDF-corpus", "bb75f4f0073d9350dfd058c0162a367e6fadf25e")
VERA_STRIDE = 14
VERA_MAX = 300_000


def gh(path):
    return json.loads(subprocess.run(["gh", "api", path], check=True, capture_output=True, text=True).stdout)


def tree(repo, ref):
    sha = gh("repos/%s/commits/%s" % (repo, ref))["sha"]
    t = gh("repos/%s/git/trees/%s?recursive=1" % (repo, sha))
    if t.get("truncated"):
        sys.exit("truncated tree for " + repo)
    return sha, [(e["path"], e["size"]) for e in t["tree"] if e["type"] == "blob"]


def safe_id(prefix, path, used):
    s = re.sub(r"[^A-Za-z0-9._+/-]", "_", path)
    i, n = prefix + "/" + s, 1
    while i.lower() in used:
        n += 1
        i = prefix + "/" + s + "~%d" % n
    used.add(i.lower())
    return i


def fetch_bytes(url):
    for _ in range(4):
        try:
            with urllib.request.urlopen(urllib.request.Request(url, headers={"User-Agent": "papyrine-manifest"}), timeout=60, context=SSL_CTX) as r:
                return r.read()
        except Exception as ex:  # noqa: BLE001
            err = ex
    raise err


IRS = """f1040 f1040es f1040sa f1040sb f1040sc f1040sd f1040se f1040sse f1040s1 f1040s2 f1040s3 f1040x f1040nr f1040v
f1041 f1065 f1120 f1120s f1120w f2441 f2106 f3800 f4562 f4868 f4506 f4506t f8283 f8332 f8582 f8606 f8812 f8863 f8889
f8949 fw4 fw9 fw8ben fw8bene fw8imy fw8eci f940 f941 f943 f944 f945 f990 f990ez f990pf f1098 f1099int f1099div
fss4 f2848 f8821 f8822 f656 f433a f433f f12153 f14039 f5329 f5695 f6251 f8960 f8995 f8995a f1116 f2210 f4797 f6765
f7004 f8300 f8379 f8801 f1128 f1310 f3520 f5471 f8038 f1099g f1099r f1099k f4684 f4952 f8615 f8396 f8582cr f8829
f1040sr f4137 f4136 f2555 f2439 f1139 f4852 f8867 f3911 f13844 f9465 f8857 f8888 f4506c fw7 fw8exp""".split()
WAYBACK = "https://web.archive.org/web/2025id_/https://www.irs.gov/pub/irs-pdf/%s.pdf"


def irs_item(name):
    return dict(id="irs/%s.pdf" % name, wayback=WAYBACK % name, license="LicenseRef-PublicDomain-USGov",
                tags=["irs", "forms"], repo="www.irs.gov", sha="wayback", path="", size=0)


def fetch_resolved(url):
    last = None
    for attempt in range(8):
        try:
            with ARCHIVE.acquire_ctx():
                time.sleep(0.7)
                with urllib.request.urlopen(urllib.request.Request(url, headers={"User-Agent": "papyrine-manifest"}), timeout=90, context=SSL_CTX) as r:
                    return r.read(), r.geturl()
        except Exception as ex:  # noqa: BLE001
            last = ex
            time.sleep(3 * (attempt + 1))
    raise last


class _Gate:
    def __init__(self, n):
        self.sem = threading.Semaphore(n)

    def acquire_ctx(self):
        return self.sem


ARCHIVE = _Gate(2)


def classify(path):
    tags = []
    def q(*args, timeout=60):
        try:
            return subprocess.run(["qpdf", *args], capture_output=True, timeout=timeout)
        except subprocess.TimeoutExpired:
            return None
    enc = q("--is-encrypted", path)
    encrypted = enc is not None and enc.returncode == 0
    if encrypted:
        tags.append("encrypted")
        rp = q("--requires-password", path)
        if rp is not None and rp.returncode == 0:
            tags.append("password")
            return tags
    chk = q("--check", path)
    if chk is None:
        tags += ["malformed", "slow"]
        return tags
    if chk.returncode == 2:
        tags += ["malformed", "unreadable"]
        return tags
    if chk.returncode == 3:
        tags.append("malformed")
    out = q("--object-streams=disable", "--stream-data=preserve", path, "-", timeout=120)
    if out is not None and out.stdout:
        d = out.stdout
        if b"/AcroForm" in d:
            tags.append("forms")
        if b"/XFA" in d:
            tags.append("xfa")
        if b"/JavaScript" in d or re.search(rb"/JS[\s(</]", d):
            tags.append("js")
        if b"/ByteRange" in d:
            tags.append("signed")
    return tags


def toml_str(s):
    return json.dumps(s, ensure_ascii=True)


def main():
    irs_only = "--irs-only" in sys.argv  # keep existing entries, (re)try only missing IRS forms
    used, items = set(), []
    existing = []
    if irs_only:
        import tomllib

        with open(OUT, "rb") as f:
            existing = tomllib.load(f)["file"]
    for prefix, repo, ref, rx, lic, stags in SOURCES:
        sha, files = tree(repo, ref)
        for p, size in sorted(files):
            if re.search(rx, p) and 0 < size <= MAXSIZE:
                items.append(dict(id=safe_id(prefix, p, used), repo=repo, sha=sha, path=p, size=size, license=lic, tags=list(stags)))
        print(prefix, sha[:10], file=sys.stderr)
    prefix, repo, ref = VERAPDF
    sha, files = tree(repo, ref)
    pdfs = sorted((p, s) for p, s in files if p.lower().endswith(".pdf") and 0 < s <= VERA_MAX)
    k = 0
    for p, s in pdfs:
        top = p.split("/")[0]
        keep = top in ("Isartor test files", "TWG test files", "ISO 32000-1", "ISO 32000-2", "Undefined")
        if not keep:
            k += 1
            keep = k % VERA_STRIDE == 0
        if keep:
            t = ["verapdf"]
            if top.startswith("Isartor"):
                t.append("isartor")
            if top.startswith("PDF_A") or top.startswith("Isartor"):
                t.append("pdfa")
            if top.startswith("PDF_UA"):
                t.append("pdfua")
            items.append(dict(id=safe_id(prefix, p, used), repo=repo, sha=sha, path=p, size=s, license="CC-BY-4.0", tags=t))
    print("verapdf", sha[:10], file=sys.stderr)

    have = {e["id"] for e in existing}
    items += [irs_item(n) for n in IRS if "irs/%s.pdf" % n not in have]
    if irs_only:
        items = [i for i in items if i.get("wayback")]

    def work(it):
        if it.get("wayback"):
            dest = os.path.join(CACHE, *it["id"].split("/"))
            os.makedirs(os.path.dirname(dest), exist_ok=True)
            try:
                data, final = fetch_resolved(it["wayback"])
            except Exception as ex:  # noqa: BLE001
                print("IRS fetch failed", it["id"], ex, file=sys.stderr)
                return None
            if not data.startswith(b"%PDF") or "id_/" not in final:
                print("IRS not a PDF / not raw", it["id"], file=sys.stderr)
                return None
            with open(dest, "wb") as f:
                f.write(data)
            it.update(url=final, sha256=hashlib.sha256(data).hexdigest(), size=len(data))
            it["repo"] = "web.archive.org snapshot of irs.gov"
            it["tags"] += classify(dest)
            if it["size"] > 1_000_000:
                it["tags"].append("large")
            return it
        url = "https://raw.githubusercontent.com/%s/%s/%s" % (it["repo"], it["sha"], urllib.parse.quote(it["path"]))
        dest = os.path.join(CACHE, *it["id"].split("/"))
        os.makedirs(os.path.dirname(dest), exist_ok=True)
        data = None
        if os.path.exists(dest):
            data = open(dest, "rb").read()
        if data is None or len(data) != it["size"]:
            data = fetch_bytes(url)
            with open(dest, "wb") as f:
                f.write(data)
        it["url"] = url
        it["sha256"] = hashlib.sha256(data).hexdigest()
        it["size"] = len(data)
        it["tags"] += classify(dest)
        if it["size"] > 1_000_000:
            it["tags"].append("large")
        return it

    with cf.ThreadPoolExecutor(max_workers=16) as ex:
        done = 0
        res = []
        for r in ex.map(work, items):
            if r is not None:
                res.append(r)
            done += 1
            if done % 100 == 0:
                print(done, "/", len(items), file=sys.stderr)

    for e in existing:
        res.append(dict(id=e["id"], url=e["url"], sha256=e["sha256"], license=e["license"], src=e["source"], tags=list(e["tags"]), size=e["size"]))
    seen, uniq = {}, []
    for it in res:  # drop byte-identical duplicates, keep first (sources are in priority order)
        if it["sha256"] in seen:
            continue
        seen[it["sha256"]] = it["id"]
        uniq.append(it)
    uniq.sort(key=lambda i: i["id"])
    with open(OUT, "w", encoding="utf-8", newline="\n") as f:
        f.write("# Papyrine test corpus manifest (ADR-017). Schema: corpus/README.md.\n")
        f.write("# Generated by tools/gen-corpus/scripts/build_manifest.py; do not hand-edit the hashes.\n")
        for it in uniq:
            f.write("\n[[file]]\n")
            f.write("id = %s\nurl = %s\nsha256 = %s\nlicense = %s\n" % (toml_str(it["id"]), toml_str(it["url"]), toml_str(it["sha256"]), toml_str(it["license"])))
            src = it.get("src") or (it["repo"] if it["sha"] == "wayback" else "%s@%s" % (it["repo"], it["sha"]))
            f.write("source = %s\n" % toml_str(src))
            f.write("tags = [%s]\nsize = %d\n" % (", ".join(toml_str(t) for t in sorted(set(it["tags"]))), it["size"]))
    print("wrote %d entries (%d duplicates dropped)" % (len(uniq), len(res) - len(uniq)), file=sys.stderr)


if __name__ == "__main__":
    main()
