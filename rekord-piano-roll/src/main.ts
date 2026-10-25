import { mount } from "svelte";
import App from "./App.svelte";
import { settings } from "./settings.svelte";
import { applyTheme } from "./theme";
import { checkForUpdate } from "./updater.svelte";

// Before the first paint: the page's colors are all custom properties set from the theme.
applyTheme(settings.theme);

// Mounted straight into <body> (no wrapper element) so the DOM tree --
// and therefore styles.css's html/body/.app layout rules -- stays exactly
// as it was before the Svelte migration.
mount(App, { target: document.body });

// A unicode-range face only loads on first use, but Pixi rasterizes chord labels once at creation,
// so fetch the accidentals up front or the first label with a ♭ would be drawn in the fallback font.
void document.fonts.load('600 13px "Accidentals"', "♯♭♮");

void checkForUpdate();
