import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { getHost, type SearchHit } from "../../ipc";
import { useActiveDoc, useApp } from "../../store/app";

/** Find in the current document: streams hits from the host, cancellable. */
export default function SearchPanel() {
  const { t } = useTranslation();
  const doc = useActiveDoc();
  const docId = useApp((s) => s.activeId);
  const setSearch = useApp((s) => s.setSearch);
  const goTo = useApp((s) => s.goToPage);
  const [query, setQuery] = useState("");
  const [caseSensitive, setCase] = useState(false);
  const [wholeWord, setWhole] = useState(false);
  const [hits, setHits] = useState<SearchHit[]>([]);
  const [progress, setProgress] = useState<{ done: number; total: number; finished: boolean } | null>(null);
  const job = useRef<string | null>(null);
  const input = useRef<HTMLInputElement>(null);

  useEffect(() => input.current?.focus(), []);

  useEffect(() => {
    const host = getHost();
    const offHit = host.on("search-hit", (e) => {
      if (e.jobId === job.current) setHits((h) => (h.length < 500 ? [...h, e.hit] : h));
    });
    const offProg = host.on("job-progress", (e) => {
      if (e.jobId === job.current) setProgress({ done: e.done, total: e.total, finished: e.finished });
    });
    return () => {
      offHit();
      offProg();
      if (job.current) void host.cancelJob(job.current);
    };
  }, []);

  // Search as you type, debounced; a new query cancels the previous job.
  useEffect(() => {
    if (!docId) return;
    const host = getHost();
    if (job.current) void host.cancelJob(job.current);
    job.current = null;
    setHits([]);
    setProgress(null);
    if (!query.trim()) {
      setSearch(null);
      return;
    }
    const timer = setTimeout(() => {
      setSearch({ docId, query, caseSensitive, wholeWord });
      void host.search(docId, query, { caseSensitive, wholeWord, diacriticInsensitive: true, includeComments: false, includeFormValues: false }).then((id) => {
        job.current = id;
      });
    }, 180);
    return () => clearTimeout(timer);
  }, [query, caseSensitive, wholeWord, docId, setSearch]);

  // Clear highlights when the panel goes away.
  useEffect(() => () => setSearch(null), [setSearch]);

  if (!doc) return <p className="empty-note">{t("panel.needsDoc")}</p>;

  return (
    <div className="panel" role="search" aria-label={t("search.label")}>
      <div className="field">
        <label htmlFor="find-input">{t("search.find")}</label>
        <input
          id="find-input"
          ref={input}
          className="input"
          type="search"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Escape" && query) {
              e.stopPropagation();
              setQuery("");
            }
          }}
          data-testid="find-input"
        />
      </div>
      <label className="check">
        <input type="checkbox" checked={caseSensitive} onChange={(e) => setCase(e.target.checked)} />
        {t("search.caseSensitive")}
      </label>
      <label className="check">
        <input type="checkbox" checked={wholeWord} onChange={(e) => setWhole(e.target.checked)} />
        {t("search.wholeWord")}
      </label>
      <p role="status" data-testid="find-status">
        {query.trim()
          ? t("search.summary", { count: hits.length, done: progress?.done ?? 0, total: progress?.total ?? doc.info.pageCount })
          : t("search.hint")}
      </p>
      <ul className="results" aria-label={t("search.results")}>
        {hits.map((h, i) => (
          <li key={i}>
            <button type="button" className="btn result" onClick={() => goTo(h.page)} data-testid="find-hit">
              <span className="where">{t("search.hitPage", { page: h.page + 1 })}</span>
              {h.snippet.slice(0, h.matchStart)}
              <mark>{h.snippet.slice(h.matchStart, h.matchStart + h.matchLength)}</mark>
              {h.snippet.slice(h.matchStart + h.matchLength)}
            </button>
          </li>
        ))}
      </ul>
    </div>
  );
}
