# Web monitor UI

The browser-based personal-monitoring surface, embedded into the daemon binary
via `rust-embed` (`#[folder = "web/dist"]`) and served at `/` (the WebSocket
endpoint is `/ws`). See `src/web/` for the server side and the JSON protocol.

## Current form: a single, self-contained file (no build step)

`dist/index.html` is hand-authored vanilla HTML/CSS/JS — **no Node, npm, or
build step**. `cargo build` embeds it as-is, so the Raspberry Pi / single-binary
build needs no JS toolchain. Edit the file directly and rebuild the Rust binary.

The Rust side serves whatever is in `dist/` and is framework-agnostic: it serves
the requested path from the embedded folder (falling back to `index.html`), with
the MIME type guessed from the extension. So a build tool can be introduced later
without touching any Rust code.

## Optional: swapping in a Svelte (or any) build later

If richer UI tooling is wanted, point a bundler's output at `dist/`:

```
# example, not currently wired
npm create vite@latest . -- --template svelte
npm install
# set base: './' and build.outDir: 'dist' in vite.config.js
npm run build        # → dist/  (commit it)
```

Then `cargo build` picks up the new `dist/` unchanged. `node_modules/` is
git-ignored; **`dist/` is committed** so cargo/Pi builds never require Node.
