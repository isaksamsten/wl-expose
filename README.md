# wl-expose

A window overview ("exposé") for Wayland compositors, built as a
layer-shell client on top of standard protocols. It aims to work on any
compositor that supports these protocols (tested on umbriel; niri next).

```sh
wl-expose [OPTIONS] [ID...]
```

wl-expose shows the given windows side by side in a full-screen overlay,
lets you pick one, prints its ID to stdout and exits 0. Escape or a click on
the background exits 1. It never focuses windows itself; a wrapper does that
through the compositor's IPC. See [PLAN.md](PLAN.md) for the design.

- `ID...` are ext toplevel identifiers (on umbriel and current upstream niri
  these are also the IPC window IDs). No IDs means every window. Unknown IDs
  are warned about and skipped; if nothing is left, it exits 1.
- `--list` prints `identifier<TAB>app_id<TAB>title` for every window.

Example niri wrapper (current workspace only):

```sh
ws=$(niri msg -j workspaces | jq '.[] | select(.is_focused) | .id')
ids=$(niri msg -j windows | jq -r ".[] | select(.workspace_id == $ws) | .id")
id=$(wl-expose $ids) &&
    niri msg action focus-window --id "$id"
```
Previews use natural window placement adapted from `native_win.js`: native-sized
windows separate using aspect-aware dodges, then fit with a shared scale while
reserving the `--gap` minimum in final logical pixels. Native size ratios are
preserved without small-window boosts or a row fallback. Placement starts from a
deterministic synthetic arrangement because desktop coordinates are unavailable.
Some space is reserved for breathing room; centers spread gently and uniformly,
preserving the composition rather than stretching it to the screen edges.
The minimum gap is a floor, not a target to minimize. If it cannot fit the natural
ordering even with zero-sized thumbnails, it is reduced with a warning to stderr.
The layout is computed before display; there is no continuous physics simulation.
If separation reaches its 500-pass limit, a warning is written to stderr and a
deterministic sweep clears any remaining collisions before fitting.


## Controls

- Pointer: hover selects, left click picks, a click on the background cancels.
- Arrows / `hjkl`: move to the nearest window in that direction.
- `Tab` / `Shift+Tab`: cycle in input order.
- `1`–`9`: pick the n-th window in input order.
- `Enter` picks, `Escape` cancels.

## Options

```
  -l, --list              print identifier<TAB>app_id<TAB>title per window
  -s, --select ID         start with this window selected (default: first ID)
      --static            capture one frame per window (default: live)
  -c, --cursors           paint cursors into thumbnails
  -t, --timeout MS        max wait for first frames before showing (500)
      --background COLOR  overlay color, RRGGBB[AA] (000000b3)
      --highlight COLOR   selection border color (4c8dffff)
      --placeholder COLOR color of windows without a thumbnail (3a3a3aff)
      --gap PX            minimum space between thumbnails (24)
      --margin PX         space around the layout (48)
      --border PX         selection border width (4)
      --max-scale F       max thumbnail scale (1.0)
```

## Protocols

- `ext_foreign_toplevel_list_v1`: window list (identifier, app_id, title)
- `ext_foreign_toplevel_image_capture_source_manager_v1` +
  `ext_image_copy_capture_manager_v1`: live thumbnails into `wl_shm` buffers
- `zwlr_layer_shell_v1` (vendored XML): the overlay surface
- `wl_subcompositor` + `wp_viewporter`: one subsurface per thumbnail, scaled
  by the compositor
- `wp_single_pixel_buffer_manager_v1` (optional, falls back to 1x1 shm
  buffers): background, highlight, placeholders
- `wl_seat` + xkbcommon: input; `wp_cursor_shape_manager_v1` (optional)

## Build

```sh
meson setup build
ninja -C build
meson test -C build
```

Requires `wayland-client`, `wayland-protocols` (>= 1.37), `wayland-scanner`,
`xkbcommon`.
