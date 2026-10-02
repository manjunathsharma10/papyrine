#!/usr/bin/env python3
"""Regenerate tests/curated.toml from `qpdf --json=2` (see qpdf_facts.py).

    python3 tests/tools/gen_curated.py > tests/curated.toml

Needs corpus/cache (corpus/fetch, tools/gen-corpus) and the qpdf CLI. The Rust test
(tests/curated.rs) loads the result and compares the model's values to these.
"""
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from qpdf_facts import Doc  # noqa: E402

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "../../../../corpus/cache"))
PBX = "files/pdfbox/pdfbox/src/test/resources/org/apache/pdfbox"

CURATED = [
    # forms, XFA, outlines
    "files/irs/fw9.pdf",
    "files/js-forms/cms-cms855a.pdf",
    "files/pdfminer/samples/nonfree/f1040nr.pdf",
    PBX + "/pdmodel/interactive/form/AcroFormsBasicFields.pdf",
    "files/qpdf/qpdf/qtest/qpdf/fields-two-pages.pdf",
    "files/js-forms/govuk-dla-for-children-claim-form-0903.pdf",
    "files/js-forms/opm-sf75.pdf",
    "files/js-forms/opm-opm1496.pdf",
    "files/js-forms/irs-f1040sb.pdf",
    "files/js-forms/cra-t1135-fill-23e.pdf",
    # page labels
    "files/pdfium/testing/resources/page_labels.pdf",
    PBX + "/pdmodel/test_pagelabels.pdf",
    "files/qpdf/qpdf/qtest/qpdf/11-pages-with-labels.pdf",
    "files/pdfjs/test/pdfs/labelled_pages.pdf",
    # annotations
    "files/pdfjs/test/pdfs/issue14438.pdf",
    "files/pdfjs/test/pdfs/issue13447.pdf",
    PBX + "/pdmodel/interactive/annotation/Annotations.pdf",
    "files/pdfplumber/tests/pdfs/annotations-rotated-180.pdf",
    # signatures
    "files/pdfium/testing/resources/two_signatures.pdf",
    "files/pdfjs/test/pdfs/issue17069.pdf",
    # rotation, crop box
    PBX + "/multipdf/rot90.pdf",
    "files/pdfium/testing/resources/bug_713197.pdf",
    "files/js-forms/pdfjs-bug1844576.pdf",
    # encryption (password from generated/passwords.tsv, "" otherwise)
    "generated/enc-rc4-40-user.pdf",
    "generated/enc-rc4-128-owner-only.pdf",
    "generated/enc-aes-128-user.pdf",
    "generated/enc-aes-256-r6-user.pdf",
    "generated/enc-aes-256-r6-unicode-password.pdf",
    "generated/enc-aes-256-r5-owner-only.pdf",
    "files/js-forms/opm-sf113a.pdf",
]


def passwords():
    out = {}
    with open(os.path.join(ROOT, "generated/passwords.tsv"), encoding="utf-8") as f:
        next(f)
        for line in f:
            c = line.rstrip("\n").split("\t")
            out["generated/" + c[0]] = c[1]
    return out


def q(s):
    out = []
    for ch in s:
        if ch == "\\":
            out.append("\\\\")
        elif ch == '"':
            out.append('\\"')
        elif ord(ch) < 0x20 or ord(ch) == 0x7F:
            out.append("\\u%04X" % ord(ch))
        else:
            out.append(ch)
    return '"' + "".join(out) + '"'


def arr(xs):
    return "[" + ", ".join(f"{x:g}" if isinstance(x, float) else str(x) for x in xs) + "]"


def header_version(path):
    with open(path, "rb") as f:
        head = f.read(1024)
    i = head.find(b"%PDF-")
    return head[i + 5:i + 8].decode() if i >= 0 else ""


def sample_pages(n):
    return sorted({0, n // 2, n - 1})


def emit(cid, pw):
    path = os.path.join(ROOT, cid)
    d = Doc(path, pw or None)
    n = len(d.page_objs())
    o = [f"[[case]]", f"id = {q(cid)}"]
    if pw:
        o.append(f"password = {q(pw)}")
    o.append(f"pages = {n}")
    o.append(f"header_version = {q(header_version(path))}")
    cf = d.catalog_flags()
    o.append(f"has_outlines = {str(cf['has_outlines']).lower()}")
    o.append(f"has_page_labels = {str(cf['has_page_labels']).lower()}")
    o.append(f"has_acroform = {str(cf['has_acroform']).lower()}")
    o.append(f"has_embedded_files = {str(cf['has_embedded_files']).lower()}")
    o.append(f"has_xmp = {str(cf['has_metadata']).lower()}")
    o.append(f"is_marked = {str(cf['marked']).lower()}")
    if cf["lang"]:
        o.append(f"lang = {q(cf['lang'])}")
    if cf["catalog_version"]:
        o.append(f"catalog_version = {q(cf['catalog_version'])}")
    o.append(f"doc_scripts = {d.doc_js_count()}")
    o.append(f"signed_fields = {d.signed_fields()}")
    info = d.info()
    if info:
        parts = ", ".join(f"{k} = {q(v)}" for k, v in info.items())
        o.append(f"info = {{ {parts} }}")
    ol = d.outline_stats()
    if ol:
        o.append(f"outline = {{ total = {ol['total']}, top = {ol['top']}, titles = [{', '.join(q(t) for t in ol['titles'])}] }}")
    form = d.form_facts()
    if form:
        types = ", ".join(f"{k} = {v}" for k, v in sorted(form["types"].items()))
        o.append(f"[case.form]")
        o.append(f"widgets = {form['widgets']}")
        o.append(f"fields = {form['fields']}")
        o.append(f"need_appearances = {str(form['need_appearances']).lower()}")
        o.append(f"has_xfa = {str(form['has_xfa']).lower()}")
        o.append(f"sig_flags = {form['sig_flags']}")
        o.append(f"types = {{ {types} }}")
        for f in form["first"]:
            o.append("[[case.form.first]]")
            o.append(f"name = {q(f['name'])}")
            o.append(f"flags = {f['flags']}")
            if f["value"] is not None:
                o.append(f"value = {q(f['value'])}")
            if f["page"]:
                o.append(f"page = {f['page'] - 1}")
    enc = d.encrypt()
    if enc:
        c = enc["caps"]
        o.append("[case.encrypt]")
        o.append(f"r = {enc['r']}")
        o.append(f"v = {enc['v']}")
        o.append(f"method = {q(enc['method'])}")
        o.append(f"p = {enc['bits']}")
        for k in ("accessibility", "extract", "modifyannotations", "modifyassembly", "modifyforms",
                  "modifyother", "printhigh", "printlow"):
            o.append(f"{k} = {str(c[k]).lower()}")
    for i in sample_pages(n):
        pf = d.page_facts(i)
        o.append("[[case.page]]")
        o.append(f"index = {i}")
        if pf["media"]:
            o.append(f"media = {arr(pf['media'])}")
            o.append(f"crop = {arr(pf['crop'])}")
        o.append(f"rotate = {pf['rotate']}")
        o.append(f"label = {q(d.label_for(i))}")
        ah = d.annot_hist(i)
        parts = ", ".join(f"{k} = {v}" for k, v in sorted(ah.items()))
        o.append(f"annots = {{ {parts} }}")
    return "\n".join(o)


if __name__ == "__main__":
    pw = passwords()
    print("# Generated by tests/tools/gen_curated.py from `qpdf --json=2`; do not edit by hand.")
    print("# Ids are relative to corpus/cache/. Page indexes are zero-based.\n")
    for cid in CURATED:
        print(emit(cid, pw.get(cid, "")))
        print()
