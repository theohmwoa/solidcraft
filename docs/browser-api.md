# Browser command API

After the browser app starts, `window.solidcraft` controls the design open in that tab.
Calls use the same command ids, parameters, control handlers and undo history as the desktop
control channel. No separate engine or server is started.

| Call | Result |
|---|---|
| `solidcraft.request(method, params?)` | Any [control method](control-protocol.md), such as `ui.inspect`, `document.inspect` or `engine.batch` |
| `solidcraft.execute(command, params?)` | Run an engine command without opening its interactive dialog |
| `solidcraft.commands()` | Discover commands, their parameters and current enablement |
| `solidcraft.inspect(params?)` | Inspect the active document, optionally with `{measure: true}` |
| `solidcraft.readFile(path)` | The bytes of a file in the browser's virtual file system, as a `Uint8Array` |
| `solidcraft.writeFile(path, bytes)` | Put a `Uint8Array`, which may come from another window, into the browser's virtual file system at `path` |

Every call returns a promise. It resolves to the control reply's `result`, or rejects with an
`Error`. Omitted or `null` parameters become `{}`. Parameters must be JSON serializable and
at most 16 MiB of JSON. At most 256 requests may be pending; excess requests reject without
running. Requests run on the UI thread between frames. A hidden or suspended tab can delay
completion; keep it active while driving the app. A reload ends the session and its pending work.

In the browser's developer console, on an empty design:

```js
await solidcraft.commands();
await solidcraft.request("ui.home", {open: false});
await solidcraft.execute("solid.box", {
  length: 30, width: 20, height: 10, body_name: "Browser API box"
});
await solidcraft.inspect({measure: true});
await solidcraft.execute("edit.undo");
await solidcraft.execute("edit.redo");
await solidcraft.request("ui.inspect");
```

A page that embeds the app passes files in and out with `readFile` and `writeFile`: write a
file, open it with `doc.open` or `file.insert_step`, and read back what `file.save` or
`file.export` wrote to a path. Both take memory paths only, such as `/host/part.step`. Paths
under `/opfs/`, `/disk/` and `/downloads/` are refused, because the app stores or downloads only
what its commands write there. Memory files last until the page reloads.

```js
await solidcraft.writeFile("/host/part.step", new Uint8Array(await response.arrayBuffer()));
await solidcraft.execute("doc.open", {path: "/host/part.step"});
await solidcraft.execute("file.export", {path: "/host/part-edited.step"});
const bytes = await solidcraft.readFile("/host/part-edited.step");
```

An agent with JavaScript evaluation access to this tab can make the same calls. The API does
not register WebMCP tools or expose a network listener. Scripts executing in the page can use
the API with the page's browser permissions. File paths refer to the browser's virtual file
system, not arbitrary files on the user's computer. Native-only operations remain subject to
the browser host's limitations; this API does not grant desktop filesystem or window access.

## Verify a browser build

Build the app with `trunk build` in `apps/solidcraft-web`, then run from the repository root:

```sh
cargo xtask web-api-test --site dist/web
```

The test starts a local server and headless Chrome, discovers commands, creates a box in the
live document, checks undo and redo, and verifies error handling and the pending-request limit.
It also checks that Ctrl+Tab and Ctrl+Shift+Tab are left to the browser, which switches tabs with them.
Set `CHROME` to a Chrome or Chromium executable if it is not installed in a usual location.
