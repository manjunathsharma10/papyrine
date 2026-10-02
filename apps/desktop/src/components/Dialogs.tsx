import * as Dialog from "@radix-ui/react-dialog";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { useActiveDoc, useApp } from "../store/app";
import { restoreFocus } from "./focus";

function Shell({ open, onOpenChange, title, children }: { open: boolean; onOpenChange: (o: boolean) => void; title: string; children: React.ReactNode }) {
  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange}>
      <Dialog.Portal>
        <Dialog.Overlay className="dialog-overlay" />
        <Dialog.Content className="dialog" aria-describedby={undefined} onCloseAutoFocus={restoreFocus}>
          <Dialog.Title className="dialog-title">{title}</Dialog.Title>
          {children}
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

/** One lazy chunk for every small modal dialog. */
export default function Dialogs() {
  return (
    <>
      <GoToPage />
      <DocProperties />
      <ConfirmClose />
      <ActionPrompt />
    </>
  );
}

function GoToPage() {
  const { t } = useTranslation();
  const open = useApp((s) => s.goToOpen);
  const setOpen = useApp((s) => s.setGoTo);
  const hasDoc = useApp((s) => s.activeId !== null);
  return (
    <Shell open={open && hasDoc} onOpenChange={setOpen} title={t("goto.title")}>
      <GoToForm />
    </Shell>
  );
}

/** Mounted per open, so the initial value exists before the field takes focus. */
function GoToForm() {
  const { t } = useTranslation();
  const setOpen = useApp((s) => s.setGoTo);
  const goTo = useApp((s) => s.goToPage);
  const doc = useActiveDoc();
  const [value, setValue] = useState(() => String((doc?.currentPage ?? 0) + 1));
  const total = doc?.info.pageCount ?? 1;
  const n = Number(value);
  const valid = Number.isInteger(n) && n >= 1 && n <= total;
  return (
    <form
      onSubmit={(e) => {
        e.preventDefault();
        if (!valid) return;
        goTo(n - 1);
        setOpen(false);
      }}
    >
      <div className="dialog-body">
        <div className="field">
          <label htmlFor="goto-input">{t("goto.label", { total })}</label>
          <input
            id="goto-input"
            className="input"
            inputMode="numeric"
            autoFocus
            value={value}
            aria-invalid={!valid}
            onChange={(e) => setValue(e.target.value)}
            onFocus={(e) => e.currentTarget.select()}
            data-testid="goto-input"
          />
        </div>
      </div>
      <div className="dialog-actions">
        <button type="button" className="btn btn-outline" onClick={() => setOpen(false)}>
          {t("common.cancel")}
        </button>
        <button type="submit" className="btn btn-primary" disabled={!valid}>
          {t("goto.go")}
        </button>
      </div>
    </form>
  );
}

function DocProperties() {
  const { t } = useTranslation();
  const open = useApp((s) => s.propsOpen);
  const setOpen = useApp((s) => s.setProps);
  const hasDoc = useApp((s) => s.activeId !== null);
  return (
    <Shell open={open && hasDoc} onOpenChange={setOpen} title={t("props.title")}>
      <PropsForm />
    </Shell>
  );
}

function PropsForm() {
  const { t } = useTranslation();
  const setOpen = useApp((s) => s.setProps);
  const run = useApp((s) => s.run);
  const doc = useActiveDoc();
  const [f, setF] = useState(() => ({
    title: doc?.info.meta.title ?? "",
    author: doc?.info.meta.author ?? "",
    subject: doc?.info.meta.subject ?? "",
    keywords: doc?.info.meta.keywords ?? "",
  }));
  if (!doc) return null;
  const m = doc.info.meta;
  const field = (key: keyof typeof f, label: string, first = false) => (
    <div className="field">
      <label htmlFor={`prop-${key}`}>{label}</label>
      <input id={`prop-${key}`} className="input" autoFocus={first} value={f[key]} onChange={(e) => setF({ ...f, [key]: e.target.value })} />
    </div>
  );
  return (
    <form
      onSubmit={(e) => {
        e.preventDefault();
        void run({ type: "set-metadata", fields: f });
        setOpen(false);
      }}
    >
      <div className="dialog-body">
        {field("title", t("props.docTitle"), true)}
        {field("author", t("props.author"))}
        {field("subject", t("props.subject"))}
        {field("keywords", t("props.keywords"))}
        <dl>
          <dt>{t("props.pages")}</dt>
          <dd>{doc.info.pageCount}</dd>
          <dt>{t("props.version")}</dt>
          <dd>{m.pdfVersion}</dd>
          <dt>{t("props.producer")}</dt>
          <dd>{m.producer}</dd>
          <dt>{t("props.encrypted")}</dt>
          <dd>{m.encrypted ? t("common.yes") : t("common.no")}</dd>
        </dl>
      </div>
      <div className="dialog-actions">
        <button type="button" className="btn btn-outline" onClick={() => setOpen(false)}>
          {t("common.cancel")}
        </button>
        <button type="submit" className="btn btn-primary">
          {t("common.apply")}
        </button>
      </div>
    </form>
  );
}

function ConfirmClose() {
  const { t } = useTranslation();
  const id = useApp((s) => s.confirmClose);
  const name = useApp((s) => (id ? s.docs[id]?.info.name : ""));
  const closeTab = useApp((s) => s.closeTab);
  const save = useApp((s) => s.save);
  const activate = useApp((s) => s.activate);
  const cancel = () => useApp.setState({ confirmClose: null });
  return (
    <Shell open={!!id} onOpenChange={(o) => !o && cancel()} title={t("close.title")}>
      <div className="dialog-body">{t("close.body", { name: name ?? "" })}</div>
      <div className="dialog-actions">
        <button type="button" className="btn btn-outline" onClick={cancel}>
          {t("common.cancel")}
        </button>
        <button type="button" className="btn btn-outline" onClick={() => id && void closeTab(id, true)}>
          {t("close.discard")}
        </button>
        <button
          type="button"
          className="btn btn-primary"
          autoFocus
          onClick={async () => {
            if (!id) return;
            activate(id);
            await save();
            await closeTab(id, true);
          }}
        >
          {t("close.save")}
        </button>
      </div>
    </Shell>
  );
}

/** Prompts for URI / launch actions found in documents: nothing runs without an explicit yes. */
function ActionPrompt() {
  const { t } = useTranslation();
  const p = useApp((s) => s.prompt);
  const setPrompt = useApp((s) => s.setPrompt);
  return (
    <Shell open={!!p} onOpenChange={(o) => !o && setPrompt(null)} title={t(p?.kind === "launch" ? "prompt.launchTitle" : "prompt.uriTitle")}>
      <div className="dialog-body">
        <p>{t(p?.kind === "launch" ? "prompt.launchBody" : "prompt.uriBody")}</p>
        <p>
          <code>{p?.target}</code>
        </p>
      </div>
      <div className="dialog-actions">
        <button type="button" className="btn btn-outline" autoFocus onClick={() => setPrompt(null)}>
          {t("prompt.block")}
        </button>
      </div>
    </Shell>
  );
}
