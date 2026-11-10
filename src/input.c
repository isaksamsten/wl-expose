// Seat handling: keyboard (xkbcommon, with key repeat) and pointer.

#include <linux/input-event-codes.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>

#include "app.h"
#include "cursor-shape-v1-client-protocol.h"

// ---------------------------------------------------------------------------
// actions

static void select_index(struct app *app, size_t i) {
	if (i < app->n_windows)
		app_select(app, app->windows[i]);
}

static void move(struct app *app, enum layout_dir dir) {
	ptrdiff_t cur = app_window_index(app, app->selected);
	if (cur < 0) {
		select_index(app, 0);
		return;
	}
	struct layout_rect *rects = calloc(app->n_windows, sizeof(*rects));
	if (!rects)
		abort();
	for (size_t i = 0; i < app->n_windows; i++) {
		struct window *w = app->windows[i];
		rects[i] = (struct layout_rect){w->x, w->y, w->w, w->h};
	}
	int next = layout_neighbor(rects, app->n_windows, (size_t)cur, dir);
	free(rects);
	if (next >= 0)
		select_index(app, (size_t)next);
}

static void cycle(struct app *app, int step) {
	size_t n = app->n_windows;
	if (n == 0)
		return;
	ptrdiff_t cur = app_window_index(app, app->selected);
	if (cur < 0) {
		select_index(app, step > 0 ? 0 : n - 1);
		return;
	}
	select_index(app, ((size_t)cur + n + (size_t)(ptrdiff_t)step) % n);
}

// Returns true if the key should auto-repeat.
static bool handle_key(struct app *app, xkb_keysym_t sym) {
	struct input *in = &app->input;
	bool shift = in->xkb_state && xkb_state_mod_name_is_active(in->xkb_state,
		XKB_MOD_NAME_SHIFT, XKB_STATE_MODS_EFFECTIVE) > 0;

	switch (sym) {
	case XKB_KEY_Escape:
		app_cancel(app);
		return false;
	case XKB_KEY_Return:
	case XKB_KEY_KP_Enter:
		if (app->selected)
			app_pick(app, app->selected);
		return false;
	case XKB_KEY_Left:
	case XKB_KEY_h:
		move(app, LAYOUT_LEFT);
		return true;
	case XKB_KEY_Right:
	case XKB_KEY_l:
		move(app, LAYOUT_RIGHT);
		return true;
	case XKB_KEY_Up:
	case XKB_KEY_k:
		move(app, LAYOUT_UP);
		return true;
	case XKB_KEY_Down:
	case XKB_KEY_j:
		move(app, LAYOUT_DOWN);
		return true;
	case XKB_KEY_Tab:
		cycle(app, shift ? -1 : 1);
		return true;
	case XKB_KEY_ISO_Left_Tab:
		cycle(app, -1);
		return true;
	default:
		if (sym >= XKB_KEY_1 && sym <= XKB_KEY_9) {
			size_t i = sym - XKB_KEY_1;
			if (i < app->n_windows)
				app_pick(app, app->windows[i]);
		}
		return false;
	}
}

// ---------------------------------------------------------------------------
// keyboard

static void kb_keymap(void *data, struct wl_keyboard *kb, uint32_t format,
		int32_t fd, uint32_t size) {
	(void)kb;
	struct input *in = &((struct app *)data)->input;
	if (format != WL_KEYBOARD_KEYMAP_FORMAT_XKB_V1) {
		close(fd);
		return;
	}
	char *map = mmap(NULL, size, PROT_READ, MAP_PRIVATE, fd, 0);
	close(fd);
	if (map == MAP_FAILED)
		return;
	struct xkb_keymap *keymap = xkb_keymap_new_from_buffer(in->xkb, map,
		strnlen(map, size), XKB_KEYMAP_FORMAT_TEXT_V1, XKB_KEYMAP_COMPILE_NO_FLAGS);
	munmap(map, size);
	if (!keymap) {
		fprintf(stderr, "wl-expose: failed to compile keymap\n");
		return;
	}
	struct xkb_state *state = xkb_state_new(keymap);
	if (!state) {
		xkb_keymap_unref(keymap);
		return;
	}
	xkb_state_unref(in->xkb_state);
	xkb_keymap_unref(in->keymap);
	in->keymap = keymap;
	in->xkb_state = state;
}

static void kb_enter(void *data, struct wl_keyboard *kb, uint32_t serial,
		struct wl_surface *surface, struct wl_array *keys) {
	(void)data;
	(void)kb;
	(void)serial;
	(void)surface;
	(void)keys;
}

static void kb_leave(void *data, struct wl_keyboard *kb, uint32_t serial,
		struct wl_surface *surface) {
	(void)kb;
	(void)serial;
	(void)surface;
	((struct app *)data)->input.repeat_next = 0;
}

static void kb_key(void *data, struct wl_keyboard *kb, uint32_t serial,
		uint32_t time, uint32_t key, uint32_t state) {
	(void)kb;
	(void)serial;
	(void)time;
	struct app *app = data;
	struct input *in = &app->input;
	if (!in->xkb_state)
		return;

	if (state != WL_KEYBOARD_KEY_STATE_PRESSED) {
		if (key == in->repeat_key)
			in->repeat_next = 0;
		return;
	}

	xkb_keycode_t code = key + 8;
	xkb_keysym_t sym = xkb_state_key_get_one_sym(in->xkb_state, code);
	bool repeats = handle_key(app, sym);
	if (repeats && in->repeat_rate > 0 && xkb_keymap_key_repeats(in->keymap, code)) {
		in->repeat_key = key;
		in->repeat_sym = sym;
		in->repeat_next = now_ms() + in->repeat_delay;
	} else {
		in->repeat_next = 0;
	}
}

static void kb_modifiers(void *data, struct wl_keyboard *kb, uint32_t serial,
		uint32_t depressed, uint32_t latched, uint32_t locked, uint32_t group) {
	(void)kb;
	(void)serial;
	struct input *in = &((struct app *)data)->input;
	if (in->xkb_state)
		xkb_state_update_mask(in->xkb_state, depressed, latched, locked, 0, 0, group);
}

static void kb_repeat_info(void *data, struct wl_keyboard *kb, int32_t rate,
		int32_t delay) {
	(void)kb;
	struct input *in = &((struct app *)data)->input;
	in->repeat_rate = rate;
	in->repeat_delay = delay;
}

static const struct wl_keyboard_listener keyboard_listener = {
	.keymap = kb_keymap,
	.enter = kb_enter,
	.leave = kb_leave,
	.key = kb_key,
	.modifiers = kb_modifiers,
	.repeat_info = kb_repeat_info,
};

int input_repeat_timeout(struct app *app) {
	struct input *in = &app->input;
	if (in->repeat_next == 0)
		return -1;
	double dt = in->repeat_next - now_ms();
	return dt > 0 ? (int)dt + 1 : 0;
}

void input_repeat(struct app *app) {
	struct input *in = &app->input;
	if (in->repeat_next == 0 || now_ms() < in->repeat_next)
		return;
	in->repeat_next += 1000.0 / in->repeat_rate;
	if (in->repeat_next < now_ms())
		in->repeat_next = now_ms(); // don't burst after a stall
	handle_key(app, in->repeat_sym);
}

// ---------------------------------------------------------------------------
// pointer

static void ptr_enter(void *data, struct wl_pointer *ptr, uint32_t serial,
		struct wl_surface *surface, wl_fixed_t x, wl_fixed_t y) {
	(void)ptr;
	(void)surface;
	struct input *in = &((struct app *)data)->input;
	// Don't select on enter: the pointer may just happen to rest on a
	// thumbnail when the overlay appears.
	in->px = wl_fixed_to_double(x);
	in->py = wl_fixed_to_double(y);
	in->pointer_inside = true;
	if (in->cursor_shape)
		wp_cursor_shape_device_v1_set_shape(in->cursor_shape, serial,
			WP_CURSOR_SHAPE_DEVICE_V1_SHAPE_DEFAULT);
}

static void ptr_leave(void *data, struct wl_pointer *ptr, uint32_t serial,
		struct wl_surface *surface) {
	(void)ptr;
	(void)serial;
	(void)surface;
	((struct app *)data)->input.pointer_inside = false;
}

static void ptr_motion(void *data, struct wl_pointer *ptr, uint32_t time,
		wl_fixed_t x, wl_fixed_t y) {
	(void)ptr;
	(void)time;
	struct app *app = data;
	app->input.px = wl_fixed_to_double(x);
	app->input.py = wl_fixed_to_double(y);
	struct window *win = overlay_hit(app, app->input.px, app->input.py);
	if (win && win != app->selected)
		app_select(app, win);
}

static void ptr_button(void *data, struct wl_pointer *ptr, uint32_t serial,
		uint32_t time, uint32_t button, uint32_t state) {
	(void)ptr;
	(void)serial;
	(void)time;
	struct app *app = data;
	if (state != WL_POINTER_BUTTON_STATE_PRESSED || !app->overlay.mapped ||
			!app->input.pointer_inside)
		return;
	struct window *win = overlay_hit(app, app->input.px, app->input.py);
	if (!win)
		app_cancel(app); // any click on the background
	else if (button == BTN_LEFT)
		app_pick(app, win);
}

static void ptr_axis(void *data, struct wl_pointer *ptr, uint32_t time,
		uint32_t axis, wl_fixed_t value) {
	(void)data;
	(void)ptr;
	(void)time;
	(void)axis;
	(void)value;
}

static void ptr_frame(void *data, struct wl_pointer *ptr) {
	(void)data;
	(void)ptr;
}

static void ptr_axis_source(void *data, struct wl_pointer *ptr, uint32_t source) {
	(void)data;
	(void)ptr;
	(void)source;
}

static void ptr_axis_stop(void *data, struct wl_pointer *ptr, uint32_t time,
		uint32_t axis) {
	(void)data;
	(void)ptr;
	(void)time;
	(void)axis;
}

static void ptr_axis_discrete(void *data, struct wl_pointer *ptr, uint32_t axis,
		int32_t discrete) {
	(void)data;
	(void)ptr;
	(void)axis;
	(void)discrete;
}

static const struct wl_pointer_listener pointer_listener = {
	.enter = ptr_enter,
	.leave = ptr_leave,
	.motion = ptr_motion,
	.button = ptr_button,
	.axis = ptr_axis,
	.frame = ptr_frame,
	.axis_source = ptr_axis_source,
	.axis_stop = ptr_axis_stop,
	.axis_discrete = ptr_axis_discrete,
};

// ---------------------------------------------------------------------------
// seat

static void release_keyboard(struct input *in) {
	if (!in->keyboard)
		return;
	if (wl_keyboard_get_version(in->keyboard) >= WL_KEYBOARD_RELEASE_SINCE_VERSION)
		wl_keyboard_release(in->keyboard);
	else
		wl_keyboard_destroy(in->keyboard);
	in->keyboard = NULL;
	in->repeat_next = 0;
}

static void release_pointer(struct input *in) {
	if (in->cursor_shape) {
		wp_cursor_shape_device_v1_destroy(in->cursor_shape);
		in->cursor_shape = NULL;
	}
	if (!in->pointer)
		return;
	if (wl_pointer_get_version(in->pointer) >= WL_POINTER_RELEASE_SINCE_VERSION)
		wl_pointer_release(in->pointer);
	else
		wl_pointer_destroy(in->pointer);
	in->pointer = NULL;
	in->pointer_inside = false;
}

static void seat_capabilities(void *data, struct wl_seat *seat, uint32_t caps) {
	struct app *app = data;
	struct input *in = &app->input;

	bool has_kb = caps & WL_SEAT_CAPABILITY_KEYBOARD;
	if (has_kb && !in->keyboard) {
		in->keyboard = wl_seat_get_keyboard(seat);
		wl_keyboard_add_listener(in->keyboard, &keyboard_listener, app);
	} else if (!has_kb) {
		release_keyboard(in);
	}

	bool has_ptr = caps & WL_SEAT_CAPABILITY_POINTER;
	if (has_ptr && !in->pointer) {
		in->pointer = wl_seat_get_pointer(seat);
		wl_pointer_add_listener(in->pointer, &pointer_listener, app);
		if (app->g.cursor_shape)
			in->cursor_shape = wp_cursor_shape_manager_v1_get_pointer(
				app->g.cursor_shape, in->pointer);
	} else if (!has_ptr) {
		release_pointer(in);
	}
}

static void seat_name(void *data, struct wl_seat *seat, const char *name) {
	(void)data;
	(void)seat;
	(void)name;
}

static const struct wl_seat_listener seat_listener = {
	.capabilities = seat_capabilities,
	.name = seat_name,
};

void input_init(struct app *app) {
	struct input *in = &app->input;
	in->repeat_rate = 25;
	in->repeat_delay = 600;
	in->xkb = xkb_context_new(XKB_CONTEXT_NO_FLAGS);
	if (!in->xkb) {
		fprintf(stderr, "wl-expose: failed to create xkb context\n");
		abort();
	}
	// Bound only now, so the initial capabilities event reaches the listener.
	struct globals *g = &app->g;
	if (!g->seat && g->seat_name) {
		g->seat = wl_registry_bind(g->registry, g->seat_name, &wl_seat_interface,
			g->seat_version);
		wl_seat_add_listener(g->seat, &seat_listener, app);
	} else {
		fprintf(stderr, "wl-expose: no seat, input disabled\n");
	}
}

void input_finish(struct app *app) {
	struct input *in = &app->input;
	release_keyboard(in);
	release_pointer(in);
	xkb_state_unref(in->xkb_state);
	xkb_keymap_unref(in->keymap);
	xkb_context_unref(in->xkb);
	*in = (struct input){0};
}
