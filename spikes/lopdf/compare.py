#!/usr/bin/env python3
"""Spike 0.3 runner: qpdf (papyrine-cos) vs lopdf over the corpus.

One child process per (library, file) so peak RSS, crashes and hangs are attributable.
RSS is ru_maxrss from wait4 (bytes on macOS). Page counts from PDFium are an independent
oracle for "did the repair find the right pages".

  compare.py --lopdf PATH --memprobe PATH [--jobs 4] [--timeout 30] [--out results.tsv]
             [--only SUBSTR] [--large-only]

Output TSV columns: file tags lib status pages objects open_ms first_ms wall_ms rss_mb detail
"""
import argparse, os, subprocess, sys, time, glob, re, signal
from concurrent.futures import ThreadPoolExecutor

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
CORPUS = os.path.join(ROOT, "corpus")


def manifest():
    out = []
    for m in ("manifest.toml", "js-forms.toml"):
        p = os.path.join(CORPUS, m)
        if not os.path.exists(p):
            continue
        fid, tags = None, []
        for line in open(p):
            line = line.strip()
            if line == "[[file]]":
                if fid:
                    out.append((fid, tags))
                fid, tags = None, []
            elif line.startswith("id") and "=" in line:
                fid = line.split('"')[1]
            elif line.startswith("tags") and "=" in line:
                tags = line.split('"')[1::2]
        if fid:
            out.append((fid, tags))
    files = []
    for fid, tags in out:
        p = os.path.join(CORPUS, "cache/files", fid)
        if os.path.isfile(p):
            files.append((p, tags))
    for p in sorted(glob.glob(os.path.join(CORPUS, "cache/generated/*.pdf"))):
        n = os.path.basename(p)
        files.append((p, ["generated", "gen-" + n.split("-")[0]]))
    return files


def run_one(cmd, timeout):
    t0 = time.time()
    with open(os.devnull, "rb") as dn, subprocess.Popen(
        cmd, stdin=dn, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL
    ) as p:
        import threading
        buf = []
        th = threading.Thread(target=lambda: buf.append(p.stdout.read()), daemon=True)
        th.start()
        status = None
        ru = None
        while True:
            pid, st, ru = os.wait4(p.pid, os.WNOHANG)
            if pid:
                p.returncode = 0  # reaped here; Popen must not wait again
                break
            if time.time() - t0 > timeout:
                os.kill(p.pid, signal.SIGKILL)
                _, st, ru = os.wait4(p.pid, 0)
                p.returncode = 0
                status = "timeout"
                break
            time.sleep(0.005)
        th.join(2)
    wall = int((time.time() - t0) * 1000)
    rss = ru.ru_maxrss / (1 << 20) if sys.platform == "darwin" else ru.ru_maxrss / 1024
    out = buf[0].decode("utf8", "replace") if buf else ""
    if status is None:
        if os.WIFSIGNALED(st):
            status = "crash:signal%d" % os.WTERMSIG(st)
        elif os.WEXITSTATUS(st) == 101:
            status = "panic"
        elif os.WEXITSTATUS(st) != 0:
            status = "crash:exit%d" % os.WEXITSTATUS(st)
    return status, out, wall, round(rss, 1)


def probe(lib, cmd, path, tags, timeout):
    status, out, wall, rss = run_one(cmd + [path], timeout)
    f = None
    for line in out.splitlines():
        if line.startswith("RESULT\t"):
            f = line.split("\t", 6)
    if f and len(f) == 7:
        st = status if status in ("timeout",) or (status and status.startswith("crash")) else f[1]
        return [path, ",".join(tags), lib, st, f[2], f[3], f[4], f[5], str(wall), str(rss), f[6]]
    return [path, ",".join(tags), lib, status or "no-result", "-", "-", "-", "-", str(wall), str(rss), out[:100].replace("\t", " ")]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--lopdf", required=True)
    ap.add_argument("--memprobe", required=True)
    ap.add_argument("--jobs", type=int, default=4)
    ap.add_argument("--timeout", type=float, default=30)
    ap.add_argument("--out", default="results.tsv")
    ap.add_argument("--only", default=None)
    ap.add_argument("--large-only", action="store_true")
    a = ap.parse_args()
    files = manifest()
    if a.only:
        files = [x for x in files if a.only in x[0]]
    if a.large_only:
        files = [x for x in files if "/generated/large-" in x[0]]
    libs = [
        ("qpdf", [a.memprobe, "cos-open"]),
        ("lopdf", [a.lopdf]),
        ("pdfium", [a.memprobe, "pdfium-open"]),
    ]
    jobs = [(l, c, p, t) for p, t in files for l, c in libs]
    rel = lambda p: os.path.relpath(p, os.path.join(CORPUS, "cache"))
    rows = []
    with ThreadPoolExecutor(a.jobs) as ex:
        for i, r in enumerate(ex.map(lambda j: probe(j[0], j[1], j[2], j[3], a.timeout), jobs)):
            r[0] = rel(r[0])
            rows.append(r)
            if i % 500 == 0:
                print(i, "/", len(jobs), file=sys.stderr)
    with open(a.out, "w") as f:
        f.write("file\ttags\tlib\tstatus\tpages\tobjects\topen_ms\tfirst_ms\twall_ms\trss_mb\tdetail\n")
        for r in rows:
            f.write("\t".join(r) + "\n")
    print("wrote", a.out, len(rows), "rows")


if __name__ == "__main__":
    main()
