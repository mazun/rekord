import { mount } from "svelte";
import App from "./App.svelte";

// Mounted straight into <body> (no wrapper element) so the DOM tree --
// and therefore styles.css's html/body/.app layout rules -- stays exactly
// as it was before the Svelte migration.
mount(App, { target: document.body });
