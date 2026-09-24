// UI language. A handful of strings total, so a plain nested dictionary
// plus a lookup function is all this needs -- no runtime, no message
// compiler, no per-locale bundle splitting.

const LOCALE_STORAGE_KEY = "chord.locale";

export type Locale = "en" | "ja";

const messages = {
  en: {
    toolbar: {
      record: "Record",
      stop: "Stop",
      settings: "Settings",
    },
    settings: {
      title: "Settings",
      close: "Close",
      language: "Language",
    },
    keyPicker: {
      autoDetect: "Auto-detect key",
      semitoneUp: "Raise a semitone",
      semitoneDown: "Lower a semitone",
    },
    pianorollToggle: {
      toggle: "Toggle piano roll visibility",
    },
    pianoRollView: {
      rolesToggle: "Click to toggle melody/bass coloring",
    },
  },
  ja: {
    toolbar: {
      record: "録音開始",
      stop: "停止",
      settings: "設定",
    },
    settings: {
      title: "設定",
      close: "閉じる",
      language: "言語",
    },
    keyPicker: {
      autoDetect: "キーの自動検出",
      semitoneUp: "半音上げる",
      semitoneDown: "半音下げる",
    },
    pianorollToggle: {
      toggle: "ピアノロールの表示切替",
    },
    pianoRollView: {
      rolesToggle: "クリックでメロディ/ベースの色分けを切り替え",
    },
  },
} as const satisfies Record<Locale, unknown>;

function detectLocale(): Locale {
  const stored = localStorage.getItem(LOCALE_STORAGE_KEY);
  if (stored === "en" || stored === "ja") return stored;
  return navigator.language.toLowerCase().startsWith("ja") ? "ja" : "en";
}

export const i18n = $state({ locale: detectLocale() });

export function setLocale(locale: Locale): void {
  i18n.locale = locale;
  localStorage.setItem(LOCALE_STORAGE_KEY, locale);
}

// Dotted-path lookup (e.g. "toolbar.record") instead of nested t().t() calls
// at each use site -- every call site here wants a single leaf string.
export function t(path: string): string {
  let node: unknown = messages[i18n.locale];
  for (const part of path.split(".")) node = (node as Record<string, unknown>)[part];
  return node as string;
}
