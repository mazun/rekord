import { mount } from "svelte";
import App from "./App.svelte";
import { checkForUpdate } from "./updater.svelte";

// Mounted straight into <body> (no wrapper element) so the DOM tree --
// and therefore styles.css's html/body/.app layout rules -- stays exactly
// as it was before the Svelte migration.
mount(App, { target: document.body });

// A unicode-range face only loads on first use, but Pixi rasterizes chord labels once at creation,
// so fetch the accidentals up front or the first label with a ♭ would be drawn in the fallback font.
void document.fonts.load('600 13px "Accidentals"', "♯♭♮");

void checkForUpdate();
