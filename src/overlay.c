// Full-screen layer surface with one subsurface per window thumbnail.
//
// The background, highlight and placeholders are single-pixel buffers (or
// 1x1 shm buffers if the compositor lacks wp_single_pixel_buffer_manager_v1)
// scaled with viewports; thumbnails attach the capture buffers directly and
// let the compositor scale them. All subsurfaces have an empty input region,
// so pointer events arrive on the main surface and are hit-tested here.

#include <limits.h>
#include <math.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/mman.h>
#include <unistd.h>

#include "app.h"
#include "single-pixel-buffer-v1-client-protocol.h"
#include "viewporter-client-protocol.h"
#include "wlr-layer-shell-unstable-v1-client-protocol.h"

// Layout size of a window whose capture size is not known yet.
#define DEFAULT_W 800
#define DEFAULT_H 500

// 1x1 ARGB8888 shm buffer, premultiplied.
static struct wl_buffer *shm_color_buffer(struct app *app, double r, double g,
		double b, double a) {
	int fd = memfd_create("wl-expose-color", MFD_CLOEXEC);
	if (fd < 0 || ftruncate(fd, 4) < 0) {
		perror("wl-expose: color buffer");
		abort();
	}
	uint32_t *pixel = mmap(NULL, 4, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
	if (pixel == MAP_FAILED) {
		perror("wl-expose: mmap");
		abort();
	}
	*pixel = (uint32_t)lround(a * 255) << 24 | (uint32_t)lround(r * 255) << 16 |
		(uint32_t)lround(g * 255) << 8 | (uint32_t)lround(b * 255);
	munmap(pixel, 4);

	struct wl_shm_pool *pool = wl_shm_create_pool(app->g.shm, fd, 4);
	struct wl_buffer *buffer = wl_shm_pool_create_buffer(pool, 0, 1, 1, 4,
		WL_SHM_FORMAT_ARGB8888);
	wl_shm_pool_destroy(pool);
	close(fd);
	return buffer;
}

static struct wl_buffer *color_buffer(struct app *app, uint32_t rgba) {
	// Both buffer kinds take premultiplied alpha.
	double a = (rgba & 0xff) / 255.0;
	double r = ((rgba >> 24) & 0xff) / 255.0 * a;
	double g = ((rgba >> 16) & 0xff) / 255.0 * a;
	double b = ((rgba >> 8) & 0xff) / 255.0 * a;
	if (!app->g.single_pixel)
		return shm_color_buffer(app, r, g, b, a);
	return wp_single_pixel_buffer_manager_v1_create_u32_rgba_buffer(
		app->g.single_pixel, (uint32_t)(r * UINT32_MAX), (uint32_t)(g * UINT32_MAX),
		(uint32_t)(b * UINT32_MAX), (uint32_t)(a * UINT32_MAX));
}

static void commit_parent(struct app *app) {
	// Subsurface positions and stacking are applied on the parent commit.
	// Before mapping, the map commit takes care of that.
	if (app->overlay.mapped)
		wl_surface_commit(app->overlay.surface);
}

// ---------------------------------------------------------------------------
// layer surface

static void layer_configure(void *data, struct zwlr_layer_surface_v1 *ls,
		uint32_t serial, uint32_t width, uint32_t height) {
	struct app *app = data;
	struct overlay *o = &app->overlay;
	zwlr_layer_surface_v1_ack_configure(ls, serial);

	bool changed = !o->configured || width != o->width || height != o->height;
	o->configured = true;
	o->width = width;
	o->height = height;
	if (width > 0 && height > 0)
		wp_viewport_set_destination(o->viewport, (int32_t)width, (int32_t)height);

	if (!o->mapped)
		return;
	if (changed)
		overlay_relayout(app);
	else
		wl_surface_commit(o->surface);
}

static void layer_closed(void *data, struct zwlr_layer_surface_v1 *ls) {
	(void)ls;
	app_cancel(data);
}

static const struct zwlr_layer_surface_v1_listener layer_listener = {
	.configure = layer_configure,
	.closed = layer_closed,
};

static void surface_enter(void *data, struct wl_surface *s, struct wl_output *output) {
	(void)data;
	(void)s;
	(void)output;
}

static void surface_leave(void *data, struct wl_surface *s, struct wl_output *output) {
	(void)data;
	(void)s;
	(void)output;
}

static void surface_preferred_scale(void *data, struct wl_surface *s, int32_t scale) {
	(void)s;
	struct app *app = data;
	if (scale < 1 || scale == app->overlay.scale)
		return;
	app->overlay.scale = scale;
	if (app->overlay.mapped)
		overlay_relayout(app);
}

static void surface_preferred_transform(void *data, struct wl_surface *s,
		uint32_t transform) {
	(void)data;
	(void)s;
	(void)transform;
}

static const struct wl_surface_listener surface_listener = {
	.enter = surface_enter,
	.leave = surface_leave,
	.preferred_buffer_scale = surface_preferred_scale,
	.preferred_buffer_transform = surface_preferred_transform,
};

bool overlay_init(struct app *app) {
	struct overlay *o = &app->overlay;
	struct globals *g = &app->g;
	o->scale = 1;

	o->background = color_buffer(app, app->opts.background);
	o->highlight = color_buffer(app, app->opts.highlight);
	o->placeholder = color_buffer(app, app->opts.placeholder);
	o->empty_region = wl_compositor_create_region(g->compositor);

	o->surface = wl_compositor_create_surface(g->compositor);
	wl_surface_add_listener(o->surface, &surface_listener, app);
	o->viewport = wp_viewporter_get_viewport(g->viewporter, o->surface);

	// Output NULL: the compositor picks the focused output.
	o->layer_surface = zwlr_layer_shell_v1_get_layer_surface(g->layer_shell,
		o->surface, NULL, ZWLR_LAYER_SHELL_V1_LAYER_OVERLAY, "wl-expose");
	zwlr_layer_surface_v1_add_listener(o->layer_surface, &layer_listener, app);
	zwlr_layer_surface_v1_set_anchor(o->layer_surface,
		ZWLR_LAYER_SURFACE_V1_ANCHOR_TOP | ZWLR_LAYER_SURFACE_V1_ANCHOR_BOTTOM |
		ZWLR_LAYER_SURFACE_V1_ANCHOR_LEFT | ZWLR_LAYER_SURFACE_V1_ANCHOR_RIGHT);
	zwlr_layer_surface_v1_set_size(o->layer_surface, 0, 0);
	zwlr_layer_surface_v1_set_exclusive_zone(o->layer_surface, -1);
	zwlr_layer_surface_v1_set_keyboard_interactivity(o->layer_surface,
		ZWLR_LAYER_SURFACE_V1_KEYBOARD_INTERACTIVITY_EXCLUSIVE);

	// Highlight: created first so it starts below all thumbnails.
	o->hl_surface = wl_compositor_create_surface(g->compositor);
	o->hl_subsurface = wl_subcompositor_get_subsurface(g->subcompositor,
		o->hl_surface, o->surface);
	wl_subsurface_set_desync(o->hl_subsurface);
	o->hl_viewport = wp_viewporter_get_viewport(g->viewporter, o->hl_surface);
	wl_surface_set_input_region(o->hl_surface, o->empty_region);

	// Initial commit without a buffer; the map happens once frames are in.
	wl_surface_commit(o->surface);
	return true;
}

void overlay_finish(struct app *app) {
	struct overlay *o = &app->overlay;
	if (o->hl_viewport)
		wp_viewport_destroy(o->hl_viewport);
	if (o->hl_subsurface)
		wl_subsurface_destroy(o->hl_subsurface);
	if (o->hl_surface)
		wl_surface_destroy(o->hl_surface);
	if (o->layer_surface)
		zwlr_layer_surface_v1_destroy(o->layer_surface);
	if (o->viewport)
		wp_viewport_destroy(o->viewport);
	if (o->surface)
		wl_surface_destroy(o->surface);
	if (o->empty_region)
		wl_region_destroy(o->empty_region);
	if (o->background)
		wl_buffer_destroy(o->background);
	if (o->highlight)
		wl_buffer_destroy(o->highlight);
	if (o->placeholder)
		wl_buffer_destroy(o->placeholder);
	*o = (struct overlay){0};
}

// ---------------------------------------------------------------------------
// thumbnails

void overlay_window_init(struct window *win) {
	struct app *app = win->app;
	struct globals *g = &app->g;
	win->surface = wl_compositor_create_surface(g->compositor);
	win->subsurface = wl_subcompositor_get_subsurface(g->subcompositor,
		win->surface, app->overlay.surface);
	wl_subsurface_set_desync(win->subsurface);
	win->viewport = wp_viewporter_get_viewport(g->viewporter, win->surface);
	wl_surface_set_input_region(win->surface, app->overlay.empty_region);
}

void overlay_window_finish(struct window *win) {
	if (win->viewport)
		wp_viewport_destroy(win->viewport);
	if (win->subsurface)
		wl_subsurface_destroy(win->subsurface);
	if (win->surface)
		wl_surface_destroy(win->surface);
	win->viewport = NULL;
	win->subsurface = NULL;
	win->surface = NULL;
}

static bool transform_swaps(uint32_t transform) {
	return transform & 1; // 90 and 270, flipped or not
}

// Logical size of a window's content, for the layout.
static void window_size(struct window *win, double *w, double *h) {
	uint32_t bw = 0, bh = 0, transform = WL_OUTPUT_TRANSFORM_NORMAL;
	if (win->shows_frame) {
		bw = win->frame_w;
		bh = win->frame_h;
		transform = win->frame_transform;
	} else if (win->cap.width && win->cap.height) {
		bw = win->cap.width;
		bh = win->cap.height;
	}
	if (!bw || !bh) {
		*w = DEFAULT_W;
		*h = DEFAULT_H;
		return;
	}
	if (transform_swaps(transform)) {
		uint32_t tmp = bw;
		bw = bh;
		bh = tmp;
	}
	// Capture sizes are in buffer pixels; never show windows larger than
	// their real size on a scaled output.
	double scale = win->app->overlay.scale;
	*w = bw / scale;
	*h = bh / scale;
}

void overlay_relayout(struct app *app) {
	struct overlay *o = &app->overlay;
	size_t n = app->n_windows;
	if (!o->configured || o->width == 0 || o->height == 0 || n == 0) {
		commit_parent(app);
		return;
	}

	struct layout_item *items = calloc(n, sizeof(*items));
	struct layout_rect *rects = calloc(n, sizeof(*rects));
	if (!items || !rects)
		abort();
	for (size_t i = 0; i < n; i++) {
		window_size(app->windows[i], &items[i].w, &items[i].h);
	}

	double m = app->opts.margin;
	struct layout_rect area = {m, m, o->width - 2 * m, o->height - 2 * m};
	if (area.w <= 0 || area.h <= 0)
		area = (struct layout_rect){0, 0, o->width, o->height};
	struct layout_params params = {
		.spacing = app->opts.gap,
		.max_scale = app->opts.max_scale,
	};
	layout_compute(items, n, area, &params, rects);

	for (size_t i = 0; i < n; i++) {
		struct window *win = app->windows[i];
		// Round edges rather than sizes so neighbors never overlap.
		int x0 = (int)lround(rects[i].x), y0 = (int)lround(rects[i].y);
		int x1 = (int)lround(rects[i].x + rects[i].w);
		int y1 = (int)lround(rects[i].y + rects[i].h);
		win->x = x0;
		win->y = y0;
		win->w = x1 > x0 ? x1 - x0 : 1;
		win->h = y1 > y0 ? y1 - y0 : 1;

		wl_subsurface_set_position(win->subsurface, win->x, win->y);
		wp_viewport_set_destination(win->viewport, win->w, win->h);
		if (!win->shows_frame) {
			wl_surface_attach(win->surface, o->placeholder, 0, 0);
			wl_surface_damage_buffer(win->surface, 0, 0, INT32_MAX, INT32_MAX);
		}
		wl_surface_commit(win->surface);
	}
	free(items);
	free(rects);

	overlay_update_highlight(app); // commits the parent
}

void overlay_window_frame(struct window *win, struct shm_buffer *buf, uint32_t transform) {
	struct app *app = win->app;
	bool resized = !win->shows_frame || buf->width != win->frame_w ||
		buf->height != win->frame_h ||
		transform_swaps(transform) != transform_swaps(win->frame_transform);
	win->shows_frame = true;
	win->frame_w = buf->width;
	win->frame_h = buf->height;
	win->frame_transform = transform;

	wl_surface_set_buffer_transform(win->surface, (int32_t)transform);
	wl_surface_attach(win->surface, buf->buffer, 0, 0);
	wl_surface_damage_buffer(win->surface, 0, 0, INT32_MAX, INT32_MAX);
	buf->busy = true;

	if (resized && app->overlay.configured)
		overlay_relayout(app); // commits every thumbnail
	else
		wl_surface_commit(win->surface);
}

void overlay_window_failed(struct window *win) {
	struct app *app = win->app;
	if (win->shows_frame || !app->overlay.configured)
		return; // the placeholder is attached on the next relayout
	wl_surface_attach(win->surface, app->overlay.placeholder, 0, 0);
	wl_surface_damage_buffer(win->surface, 0, 0, INT32_MAX, INT32_MAX);
	wl_surface_commit(win->surface);
}

void overlay_update_highlight(struct app *app) {
	struct overlay *o = &app->overlay;
	struct window *sel = app->selected;
	int b = app->opts.border;
	if (sel && sel->w > 0 && sel->h > 0 && b > 0) {
		wl_subsurface_set_position(o->hl_subsurface, sel->x - b, sel->y - b);
		wl_subsurface_place_below(o->hl_subsurface, sel->surface);
		wp_viewport_set_destination(o->hl_viewport, sel->w + 2 * b, sel->h + 2 * b);
		wl_surface_attach(o->hl_surface, o->highlight, 0, 0);
		wl_surface_damage_buffer(o->hl_surface, 0, 0, INT32_MAX, INT32_MAX);
	} else {
		wl_surface_attach(o->hl_surface, NULL, 0, 0);
	}
	wl_surface_commit(o->hl_surface);
	commit_parent(app);
}

void overlay_maybe_map(struct app *app) {
	struct overlay *o = &app->overlay;
	if (o->mapped || !o->configured)
		return;
	bool settled = true;
	for (size_t i = 0; i < app->n_windows && settled; i++)
		settled = capture_settled(&app->windows[i]->cap);
	if (!settled && now_ms() < app->map_deadline)
		return;

	o->mapped = true;
	wl_surface_attach(o->surface, o->background, 0, 0);
	wl_surface_damage_buffer(o->surface, 0, 0, INT32_MAX, INT32_MAX);
	overlay_relayout(app); // commits the parent, mapping it
}

struct window *overlay_hit(struct app *app, double x, double y) {
	if (!app->overlay.mapped)
		return NULL;
	for (size_t i = 0; i < app->n_windows; i++) {
		struct window *win = app->windows[i];
		if (x >= win->x && x < win->x + win->w && y >= win->y && y < win->y + win->h)
			return win;
	}
	return NULL;
}

