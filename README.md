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
  -t, --timeout MS        max wait for window sizes before showing (500)
      --config PATH       config file (default: see below); implies --no-daemon
      --daemon            stay running and show the overlay for later calls
      --no-daemon         show the overlay from this process even if a daemon runs
```

## Daemon

`wl-expose --daemon` keeps the Wayland connection, window list, config and
decoded background image ready, so invocations skip all of that. Start it with
the session, e.g. in niri:

```kdl
spawn-at-startup "wl-expose" "--daemon"
```

Plain `wl-expose [OPTIONS] [ID...]` then hands its request to the daemon over
`$XDG_RUNTIME_DIR/wl-expose-$WAYLAND_DISPLAY.sock` and prints the result and exits
exactly as before, so wrappers don't change. Without a daemon it shows the
overlay itself. Window captures still start fresh on every show.

- Calling wl-expose while the overlay is open closes it (both calls exit 1),
  so a single key binding toggles it.
- Killing the waiting client cancels its overlay.
- The daemon reloads the config and image when their files change (checked
  on each show). An invalid config is reported to the client, and the
  previous one is kept.
- It exits when the compositor goes away.

## Configuration

Appearance is read from `$XDG_CONFIG_HOME/wl-expose/config.toml` (usually
`~/.config/wl-expose/config.toml`). A missing file means the defaults; every
key is optional, and unknown keys are an error. Colors are `RRGGBB[AA]`, with
or without a leading `#`.

```toml
background = "000000b3"     # overlay color; tints the image if one is set
# background-image = "~/Pictures/wall.png"  # PNG, JPEG or WebP
placeholder = "3a3a3aff"    # color of windows without a thumbnail
gap = 24                    # minimum space between thumbnails, px
margin = 48                 # space around the layout, px; negative extends past the card
zoom = 1.08                 # zoom factor of the selected thumbnail; 1 disables
zoom-duration = 150         # zoom animation, ms; 0 is instant
max-scale = 1.0             # max thumbnail scale
shadow-color = "00000077"   # alpha 0 disables shadows
shadow-softness = 30        # shadow blur radius, px
shadow-spread = 5           # px, may be negative
shadow-offset = [0, 5]      # px
open-duration = 400         # ms
card-color = "303030ff"     #
card-scale = 0.9
background-blur = 0
```

`background-image` is scaled to cover the output (centered, cropped to keep its
aspect ratio) by the compositor. `~/` is expanded; relative paths are relative
to the config file. Use a fully transparent `background` (`00000000`) to show the
image untinted. If the image cannot be loaded, a warning is printed and the plain
color is used.

Shadows match niri's (CSS `box-shadow` semantics, same defaults) and are not
drawn behind thumbnails, so translucent windows show the background.

## Protocols

- `ext_foreign_toplevel_list_v1`: window list (identifier, app_id, title)
- `ext_foreign_toplevel_image_capture_source_manager_v1` +
  `ext_image_copy_capture_manager_v1`: live thumbnails, into dmabufs
  (`zwp_linux_dmabuf_v1` v3 + GBM) when possible, otherwise `wl_shm` buffers
- `zwlr_layer_shell_v1`: the overlay surface
- `wl_subcompositor` + `wp_viewporter`: one subsurface per thumbnail, scaled
  by the compositor
- `wp_single_pixel_buffer_manager_v1` (optional, falls back to 1x1 shm
  buffers): background, placeholders; the background image and shadows are
  plain `wl_shm` buffers
- `wl_seat` + xkbcommon: input; `wp_cursor_shape_manager_v1` (optional)

## Build

Written in Rust on [smithay-client-toolkit](https://github.com/Smithay/client-toolkit)
(SCTK), with protocol bindings from `wayland-protocols`.

```sh
cargo build --release
cargo test
```

Requires Rust 1.88 or newer, `libxkbcommon` and `libgbm`.
