import i18next from "i18next";
import ICU from "i18next-icu";
import { initReactI18next } from "react-i18next";
import en from "../locales/en.json";
import { generatePseudo, pseudoAccent, pseudoRtl } from "./pseudo";

export type LocaleId = "en" | "en-XA" | "ar-XB";

export const LOCALES: { id: LocaleId; dir: "ltr" | "rtl"; labelKey: string }[] = [
  { id: "en", dir: "ltr", labelKey: "locale.en" },
  { id: "en-XA", dir: "ltr", labelKey: "locale.pseudo" },
  { id: "ar-XB", dir: "rtl", labelKey: "locale.pseudoRtl" },
];

export const catalog: Record<string, string> = en;

export async function initI18n(locale: LocaleId = "en"): Promise<void> {
  await i18next
    .use(new ICU())
    .use(initReactI18next)
    .init({
      lng: locale,
      fallbackLng: "en",
      keySeparator: false,
      nsSeparator: false,
      resources: {
        en: { translation: en },
        "en-XA": { translation: generatePseudo(en, pseudoAccent) },
        "ar-XB": { translation: generatePseudo(en, pseudoRtl) },
      },
      interpolation: { escapeValue: false },
      returnNull: false,
    });
  applyLocaleToDocument(locale);
}

export function applyLocaleToDocument(locale: LocaleId): void {
  if (typeof document === "undefined") return;
  const info = LOCALES.find((l) => l.id === locale) ?? LOCALES[0];
  document.documentElement.lang = locale;
  document.documentElement.dir = info?.dir ?? "ltr";
}

export async function setLocale(locale: LocaleId): Promise<void> {
  await i18next.changeLanguage(locale);
  applyLocaleToDocument(locale);
}

export const t = (key: string, opts?: Record<string, unknown>): string => i18next.t(key, opts) as string;
export { i18next };
