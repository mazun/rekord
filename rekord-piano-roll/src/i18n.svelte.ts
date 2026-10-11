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
  "toolbar.repeat": "Repeat the selected range",
  "toolbar.settings": "Settings",
  "debugExport.export": "Export range",
  "debugExport.busy": "Exporting...",
  "debugExport.done": "Exported (path copied)",
  "debugExport.failed": "Export failed",
  "pianoRollView.zoom": "Time zoom (scroll speed)",
  "playbackVolume.title": "Playback volume",
  "playbackVolume.original": "Recorded audio",
  "playbackVolume.notes": "Notes (synth)",
  "playbackVolume.mute": "Mute",
  "playbackVolume.unmute": "Unmute",
  "wave.sine": "Sine wave",
  "wave.triangle": "Triangle wave",
  "wave.square": "Square wave",
  "wave.saw": "Sawtooth wave",
  "settings.title": "Settings",
  "settings.close": "Close",
  "settings.language": "Language",
  "settings.noteModel": "Note model",
  "settings.noteModelAuto": "Auto (by this PC's hardware)",
  "settings.noteModelLarge": "Okarina L (most accurate, needs a discrete GPU)",
  "settings.noteModelMedium": "Okarina M (GPU or a fast CPU)",
  "settings.noteModelSmall": "Okarina S (lightest, for a slow CPU)",
  "settings.noteModelLastRun": "Last session",
  "settings.overBudgetNote": "Okarina L may be too slow without a discrete GPU, and the notes may fall behind the audio.",
  "settings.lookAhead": "Look-ahead",
  "settings.lookAhead8": "0.26 s (less delay)",
  "settings.lookAhead16": "0.51 s (slightly more accurate)",
  "settings.cpuFallbackNote": "DirectML is unavailable, so the model runs on the CPU, at a size that suits its speed.",
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
  "toolbar.repeat": "選択範囲をリピート",
  "toolbar.settings": "設定",
  "debugExport.export": "範囲を書き出し",
  "debugExport.busy": "書き出し中...",
  "debugExport.done": "書き出しました(パスをコピー)",
  "debugExport.failed": "書き出しに失敗",
  "pianoRollView.zoom": "横方向のズーム(スクロール速度)",
  "playbackVolume.title": "再生音量",
  "playbackVolume.original": "録音した音",
  "playbackVolume.notes": "音符(合成音)",
  "playbackVolume.mute": "ミュート",
  "playbackVolume.unmute": "ミュート解除",
  "wave.sine": "サイン波",
  "wave.triangle": "三角波",
  "wave.square": "矩形波",
  "wave.saw": "のこぎり波",
  "settings.title": "設定",
  "settings.close": "閉じる",
  "settings.language": "言語",
  "settings.noteModel": "音符検出のモデル",
  "settings.noteModelAuto": "自動(この PC のハードウェアに合わせる)",
  "settings.noteModelLarge": "Okarina L(最も正確、dGPU が必要)",
  "settings.noteModelMedium": "Okarina M(GPU か速い CPU 向け)",
  "settings.noteModelSmall": "Okarina S(最軽量、遅い CPU 向け)",
  "settings.noteModelLastRun": "直前のセッション",
  "settings.overBudgetNote": "Okarina L は dGPU が無いと遅すぎて、音符が音に遅れることがあります。",
  "settings.lookAhead": "先読み",
  "settings.lookAhead8": "0.26 秒(低遅延)",
  "settings.lookAhead16": "0.51 秒(高精度)",
  "settings.cpuFallbackNote": "DirectML が使えないため、CPU の速さに合わせたサイズで CPU 上で動かしています。",
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
