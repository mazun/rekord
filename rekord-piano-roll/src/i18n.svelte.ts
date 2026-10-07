// UI language. A handful of strings total, so a flat dictionary plus a
// lookup function is all this needs -- no runtime, no message compiler, no
// per-locale bundle splitting. Keys are dotted strings for grouping, but
// they're plain literal keys, so `t()` is checked against them by the
// compiler.

const LOCALE_STORAGE_KEY = "chord.locale";

export type Locale = "en" | "ja";

// `en` is the source of truth for the key set.
const en = {
  "toolbar.record": "Record",
  "toolbar.stop": "Stop",
  "toolbar.play": "Play the recording",
  "toolbar.pause": "Pause",
  "toolbar.settings": "Settings",
  "playbackVolume.title": "Playback volume",
  "playbackVolume.original": "Recorded audio",
  "playbackVolume.sine": "Notes (sine waves)",
  "settings.title": "Settings",
  "settings.close": "Close",
  "settings.language": "Language",
  "settings.noteModel": "Note model",
  "settings.noteModelMix": "Large, 3 parts: vocal / bass / other (for songs)",
  "settings.noteModelLarge": "Large, single part (solo instruments)",
  "settings.noteModelMedium": "Medium, single part",
  "settings.noteModelSmall": "Small, single part (lightest)",
  "settings.lookAhead": "Look-ahead",
  "settings.lookAhead8": "0.26 s (less delay)",
  "settings.lookAhead16": "0.51 s (slightly more accurate)",
  "settings.cpuFallbackNote": "DirectML is unavailable, so the small model is running on the CPU.",
  "settings.accidental": "Sharps / flats",
  "settings.accidentalAuto": "Auto (most natural for piano notation)",
  "settings.accidentalSharp": "Always ♯",
  "settings.accidentalFlat": "Always ♭",
  "keyPicker.auto": "Auto",
  "keyPicker.autoDetect": "Auto-detect key",
  "keyPicker.semitoneUp": "Raise a semitone",
  "keyPicker.semitoneDown": "Lower a semitone",
  "pianorollToggle.toggle": "Toggle piano roll visibility",
  "pianoRollView.rolesToggle": "Click to toggle melody/bass coloring",
  "pianoRollView.melody": "Melody",
  "pianoRollView.bass": "Bass",
  "pianoRollView.partToggle": "Click to show or hide this part's notes",
  "pianoRollView.vocal": "Vocal",
  "pianoRollView.other": "Other",
} as const;

export type MessageKey = keyof typeof en;

// Must have exactly `en`'s keys: a missing or extra key is a compile error.
const ja: Record<MessageKey, string> = {
  "toolbar.record": "録音開始",
  "toolbar.stop": "停止",
  "toolbar.play": "録音を再生",
  "toolbar.pause": "一時停止",
  "toolbar.settings": "設定",
  "playbackVolume.title": "再生音量",
  "playbackVolume.original": "録音した音",
  "playbackVolume.sine": "音符(サイン波)",
  "settings.title": "設定",
  "settings.close": "閉じる",
  "settings.language": "言語",
  "settings.noteModel": "音符検出のモデル",
  "settings.noteModelMix": "large・3 パート(歌/ベース/その他、曲向け)",
  "settings.noteModelLarge": "large・単一パート(単独の楽器向け)",
  "settings.noteModelMedium": "medium・単一パート",
  "settings.noteModelSmall": "small・単一パート(最軽量)",
  "settings.lookAhead": "先読み",
  "settings.lookAhead8": "0.26 秒(低遅延)",
  "settings.lookAhead16": "0.51 秒(高精度)",
  "settings.cpuFallbackNote": "DirectML が使えないため、small のモデルを CPU で動かしています。",
  "settings.accidental": "♯ / ♭ 表記",
  "settings.accidentalAuto": "自動(ピアノ譜で最も自然な表記)",
  "settings.accidentalSharp": "常に ♯",
  "settings.accidentalFlat": "常に ♭",
  "keyPicker.auto": "自動",
  "keyPicker.autoDetect": "キーの自動検出",
  "keyPicker.semitoneUp": "半音上げる",
  "keyPicker.semitoneDown": "半音下げる",
  "pianorollToggle.toggle": "ピアノロールの表示切替",
  "pianoRollView.rolesToggle": "クリックでメロディ/ベースの色分けを切り替え",
  "pianoRollView.melody": "メロディ",
  "pianoRollView.bass": "ベース",
  "pianoRollView.partToggle": "クリックでこのパートの音符を表示/非表示",
  "pianoRollView.vocal": "歌",
  "pianoRollView.other": "その他",
};

const messages: Record<Locale, Record<MessageKey, string>> = { en, ja };

function detectLocale(): Locale {
  const stored = localStorage.getItem(LOCALE_STORAGE_KEY);
  if (stored === "en" || stored === "ja") return stored;
  return navigator.language.toLowerCase().startsWith("ja") ? "ja" : "en";
}

export const i18n = $state({ locale: detectLocale() });

// Keep <html lang> in step with the UI language (screen readers, CJK font
// selection).
document.documentElement.lang = i18n.locale;

export function setLocale(locale: Locale): void {
  i18n.locale = locale;
  document.documentElement.lang = locale;
  localStorage.setItem(LOCALE_STORAGE_KEY, locale);
}

export function t(key: MessageKey): string {
  return messages[i18n.locale][key];
}
