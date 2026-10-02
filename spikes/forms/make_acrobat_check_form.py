#!/usr/bin/env python3
"""Generate a small fillable PDF whose fields carry the AF format actions on which
PDFium and our implementation disagree (docs/spikes/0.4-form-scripts.md, section 5).

Open the result in Adobe Acrobat Reader and compare each field's displayed text with the
"expected" columns printed beside it. An OpenAction re-assigns every field's value so
the Format actions run on open. Invalid dates raise an alert (expected for two rows).

usage: make_acrobat_check_form.py [OUT.pdf]    (default corpus/cache/generated/acrobat-check.pdf)
The PDF is generated, never committed.
"""
import os, sys

CASES = [
    # (script, initial value, expected by Papyrine, PDFium output / note)
    ('AFNumber_Format(2, 4, 0, 0, "", false);', "1234567.891", "1'234'567.89", "1,234,567.89 (style 4 -> 0)"),
    ('AFNumber_Format(2, 0, 1, 0, "", false);', "-1234.5", "-1,234.50 (red)", "1,234.50 (red, no sign)"),
    ('AFNumber_Format(0, 1, 0, 0, "", false);', "1234.5", "1234 (half-even)", "unverified tie rounding"),
    ('AFNumber_Format(2, 0, 0, 0, "", false);', "2.675", "2.68", "2.68"),
    ('AFDate_FormatEx("mm/dd/yyyy");', "02/29/2023", "unchanged + alert", "03/01/2023"),
    ('AFDate_FormatEx("mm/dd/yyyy");', "x", "unchanged + alert", "today's date"),
    ('AFDate_FormatEx("m/d/yy");', "3/7/2023", "3/7/20 (yy reads 2 digits)", "same; Acrobat unverified"),
    ('AFTime_Format(1);', "00:05", "12:05 am", "0:05 am"),
    ('AFTime_Format(1);', "12:00", "12:00 pm", "12:00 am"),
]

def esc(s):
    return s.replace("\\", "\\\\").replace("(", "\\(").replace(")", "\\)")

objs = []  # list of bytes bodies, index = object number - 1
def add(body):
    objs.append(body.encode("latin-1")); return len(objs)

# reserve 1 catalog, 2 pages, 3 page, 4 font, 5 acroform dict, 6 openaction
for _ in range(6): add("")
fields = []
y = 740
content = ["BT /F1 11 Tf 40 770 Td (Papyrine AF check: compare each field with the expected columns) Tj ET"]
for i, (script, val, ours, theirs) in enumerate(CASES):
    js = add("<< /S /JavaScript /JS (%s) >>" % esc(script))
    f = add("<< /Type /Annot /Subtype /Widget /FT /Tx /T (case_%d) /V (%s) /Rect [40 %d 170 %d] /F 4 /DA (/F1 10 Tf 0 g) /MK << /BC [0 0 0] >> /AA << /F %d 0 R >> /P 3 0 R >>" % (i + 1, esc(val), y - 4, y + 12, js))
    fields.append(f)
    content.append("BT /F1 8 Tf 180 %d Td (%s) Tj ET" % (y + 4, esc("in=%s   papyrine=%s   pdfium=%s" % (val, ours, theirs))))
    content.append("BT /F1 6 Tf 40 %d Td (%s) Tj ET" % (y - 12, esc(script)))
    y -= 40
stream = "\n".join(content)
cont = add("<< /Length %d >>\nstream\n%s\nendstream" % (len(stream), stream))
objs[2] = ("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents %d 0 R /Resources << /Font << /F1 4 0 R >> >> /Annots [%s] >>" % (cont, " ".join("%d 0 R" % f for f in fields))).encode()
objs[0] = b"<< /Type /Catalog /Pages 2 0 R /AcroForm 5 0 R /OpenAction 6 0 R >>"
objs[1] = b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>"
objs[3] = b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>"
objs[4] = ("<< /Fields [%s] /NeedAppearances true /DA (/F1 10 Tf 0 g) /DR << /Font << /F1 4 0 R >> >> >>" % " ".join("%d 0 R" % f for f in fields)).encode()
objs[5] = ("<< /S /JavaScript /JS (for (var i = 0; i < this.numFields; i++) { var f = this.getField(this.getNthFieldName(i)); if (f.type == \"text\") { f.value = f.value; } }) >>").encode()

out = bytearray(b"%PDF-1.7\n")
offs = []
for n, body in enumerate(objs, 1):
    offs.append(len(out)); out += b"%d 0 obj\n" % n + body + b"\nendobj\n"
xref = len(out)
out += b"xref\n0 %d\n0000000000 65535 f \n" % (len(objs) + 1)
for o in offs: out += b"%010d 00000 n \n" % o
out += b"trailer\n<< /Size %d /Root 1 0 R >>\nstartxref\n%d\n%%%%EOF\n" % (len(objs) + 1, xref)
path = sys.argv[1] if len(sys.argv) > 1 else os.path.join(os.path.dirname(__file__), "..", "..", "corpus", "cache", "generated", "acrobat-check.pdf")
os.makedirs(os.path.dirname(path), exist_ok=True)
open(path, "wb").write(out)
print(os.path.abspath(path))
