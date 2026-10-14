import { invoke } from "@tauri-apps/api/core";
import { debugExport } from "./debugExport.svelte";
import type { NotePart } from "./pianoroll";
import { recording } from "./recording";
import { settings } from "./settings.svelte";

// MIDI out: the backend writes the selected notes (and drum hits) as a .mid file and drags it out of
// the window (onto a DAW's track or a folder). Parts hidden on the roll are left out.

/** Which notes go in the file: one part alone, or the three parts on their own channels plus the
 * drums (General MIDI drum map, channel 10). */
type MidiLayout = "vocal" | "bass" | "other" | "split";

/** Drags the selection's .mid out of the window: the three parts and the drums on their own
 * channels, or just the part of the note the drag was pressed on. Resolves when the drag ends. */
export async function dragSelectionAsMidi(
  part: NotePart | null,
  isPartShown: (part: NotePart) => boolean,
): Promise<void> {
  const range = debugExport.range;
  if (!range) return;
  try {
    // What is on screen goes in: the notes with the keyboard shown, the drums with the drum lane.
    const midi = recording.midiRange(
      range,
      (p) => settings.pianorollVisible && isPartShown(p),
      settings.drumsVisible,
    );
    const layout: MidiLayout = part ?? "split";
    await invoke("drag_notes_as_midi", { range: { ...midi, layout } });
  } catch (err) {
    console.error(err);
  }
}
