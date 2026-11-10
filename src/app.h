#pragma once

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <wayland-client.h>
#include <xkbcommon/xkbcommon.h>

#include "layout.h"
#include "toplevel.h"
#include "wl.h"

#define MAX_SHM_FORMATS 32

struct options {
	char **ids;
	size_t n_ids;
	const char *select_id;
	bool list;
	bool live;     // keep capturing after the first frame
	bool cursors;  // paint cursors into captures
	int timeout_ms; // max wait for first frames before showing the overlay
	uint32_t background, highlight, placeholder; // 0xRRGGBBAA
	int gap, margin, border;
	double max_scale;
};

struct capture;

struct shm_buffer {
	struct capture *cap;
	struct wl_buffer *buffer;
	void *data;
	size_t size;
	uint32_t width, height, stride, format;
	bool busy; // attached to a surface and not yet released
};

struct capture {
	struct window *win;
	struct ext_image_capture_source_v1 *source;
	struct ext_image_copy_capture_session_v1 *session;
	struct ext_image_copy_capture_frame_v1 *frame; // in flight

	// constraints, collected per batch
	uint32_t width, height;
	uint32_t shm_formats[MAX_SHM_FORMATS];
	size_t n_shm_formats;
	bool constraints_reset;
	bool have_constraints;
	bool realloc_pending; // new constraints arrived while a frame was in flight

	struct shm_buffer bufs[2];
	int back; // buffer the next frame is captured into
	uint32_t transform;
	int failures; // consecutive failed frames

	bool have_frame; // at least one frame was shown
	bool done;       // no further frames will be requested
	bool failed;     // gave up without any frame: show a placeholder
};

struct window {
	struct app *app;
	struct toplevel *tl;
	struct capture cap;

	struct wl_surface *surface;
	struct wl_subsurface *subsurface;
	struct wp_viewport *viewport;
	int x, y, w, h; // layout rect, surface-local

	bool shows_frame; // a capture buffer is attached (else the placeholder)
	uint32_t frame_w, frame_h, frame_transform;
};

struct overlay {
	struct wl_surface *surface;
	struct zwlr_layer_surface_v1 *layer_surface;
	struct wp_viewport *viewport;
	struct wl_buffer *background, *highlight, *placeholder;
	struct wl_region *empty_region;

	struct wl_surface *hl_surface;
	struct wl_subsurface *hl_subsurface;
	struct wp_viewport *hl_viewport;

	uint32_t width, height;
	int32_t scale;
	bool configured, mapped;
};

struct input {
	struct wl_keyboard *keyboard;
	struct wl_pointer *pointer;
	struct wp_cursor_shape_device_v1 *cursor_shape;

	struct xkb_context *xkb;
	struct xkb_keymap *keymap;
	struct xkb_state *xkb_state;

	int32_t repeat_rate, repeat_delay;
	uint32_t repeat_key;
	xkb_keysym_t repeat_sym;
	double repeat_next; // 0: not repeating

	double px, py; // last pointer position on the overlay surface
	bool pointer_inside;
};

struct app {
	struct globals g;
	struct options opts;
	struct toplevel_list toplevels;

	struct window **windows; // layout order
	size_t n_windows;
	struct window *selected;

	struct overlay overlay;
	struct input input;

	double map_deadline;
	bool running;
	int exit_code;
	char *picked; // identifier of the picked window
};

// main.c
void app_select(struct app *app, struct window *win);
void app_pick(struct app *app, struct window *win);
void app_cancel(struct app *app);
ptrdiff_t app_window_index(struct app *app, struct window *win);

// capture.c
void capture_start(struct window *win);
void capture_finish(struct capture *cap);
// The first frame arrived or capturing gave up.
bool capture_settled(const struct capture *cap);

// overlay.c
bool overlay_init(struct app *app);
void overlay_finish(struct app *app);
void overlay_window_init(struct window *win);
void overlay_window_finish(struct window *win);
void overlay_window_frame(struct window *win, struct shm_buffer *buf, uint32_t transform);
void overlay_window_failed(struct window *win);
void overlay_maybe_map(struct app *app);
void overlay_relayout(struct app *app);
void overlay_update_highlight(struct app *app);
struct window *overlay_hit(struct app *app, double x, double y);

// input.c
void input_init(struct app *app);
void input_finish(struct app *app);
int input_repeat_timeout(struct app *app); // ms until the next repeat, -1: none
void input_repeat(struct app *app);

