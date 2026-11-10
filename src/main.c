// wl-expose: pick a window from an exposé-style overlay and print its ID.

#include <errno.h>
#include <getopt.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "app.h"

static const char usage_text[] =
	"usage: wl-expose [OPTIONS] [ID...]\n"
	"\n"
	"Show the given windows (ext toplevel identifiers; default: all) side by\n"
	"side, let the user pick one, print its ID, and exit 0. Escape or a click\n"
	"on the background exits 1.\n"
	"\n"
	"  -l, --list              print identifier<TAB>app_id<TAB>title per window\n"
	"  -s, --select ID         start with this window selected\n"
	"      --static            capture one frame per window (default: live)\n"
	"  -c, --cursors           paint cursors into thumbnails\n"
	"  -t, --timeout MS        max wait for first frames before showing (500)\n"
	"      --background COLOR  overlay color, RRGGBB[AA] (000000b3)\n"
	"      --highlight COLOR   selection border color (4c8dffff)\n"
	"      --placeholder COLOR color of windows without a thumbnail (3a3a3aff)\n"
	"      --gap PX            space between thumbnails (24)\n"
	"      --margin PX         space around the layout (48)\n"
	"      --border PX         selection border width (4)\n"
	"      --max-scale F       max thumbnail scale (1.0)\n"
	"  -h, --help              show this help\n"
	"  -V, --version           show the version\n";

enum {
	OPT_STATIC = 0x100,
	OPT_BACKGROUND,
	OPT_HIGHLIGHT,
	OPT_PLACEHOLDER,
	OPT_GAP,
	OPT_MARGIN,
	OPT_BORDER,
	OPT_MAX_SCALE,
};

static const struct option long_options[] = {
	{"list", no_argument, NULL, 'l'},
	{"select", required_argument, NULL, 's'},
	{"static", no_argument, NULL, OPT_STATIC},
	{"cursors", no_argument, NULL, 'c'},
	{"timeout", required_argument, NULL, 't'},
	{"background", required_argument, NULL, OPT_BACKGROUND},
	{"highlight", required_argument, NULL, OPT_HIGHLIGHT},
	{"placeholder", required_argument, NULL, OPT_PLACEHOLDER},
	{"gap", required_argument, NULL, OPT_GAP},
	{"margin", required_argument, NULL, OPT_MARGIN},
	{"border", required_argument, NULL, OPT_BORDER},
	{"max-scale", required_argument, NULL, OPT_MAX_SCALE},
	{"help", no_argument, NULL, 'h'},
	{"version", no_argument, NULL, 'V'},
	{0},
};

static bool parse_color(const char *s, uint32_t *out) {
	if (*s == '#')
		s++;
	size_t len = strlen(s);
	if (len != 6 && len != 8)
		return false;
	char *end;
	errno = 0;
	unsigned long v = strtoul(s, &end, 16);
	if (errno || *end)
		return false;
	*out = len == 6 ? (uint32_t)(v << 8 | 0xff) : (uint32_t)v;
	return true;
}

static bool parse_int(const char *s, int *out) {
	char *end;
	errno = 0;
	long v = strtol(s, &end, 10);
	if (errno || *end || end == s || v < 0 || v > 100000)
		return false;
	*out = (int)v;
	return true;
}

static bool parse_options(int argc, char **argv, struct options *opts, int *exit_code) {
	int opt;
	while ((opt = getopt_long(argc, argv, "ls:ct:hV", long_options, NULL)) != -1) {
		bool ok = true;
		switch (opt) {
		case 'l': opts->list = true; break;
		case 's': opts->select_id = optarg; break;
		case 'c': opts->cursors = true; break;
		case 't': ok = parse_int(optarg, &opts->timeout_ms); break;
		case OPT_STATIC: opts->live = false; break;
		case OPT_BACKGROUND: ok = parse_color(optarg, &opts->background); break;
		case OPT_HIGHLIGHT: ok = parse_color(optarg, &opts->highlight); break;
		case OPT_PLACEHOLDER: ok = parse_color(optarg, &opts->placeholder); break;
		case OPT_GAP: ok = parse_int(optarg, &opts->gap); break;
		case OPT_MARGIN: ok = parse_int(optarg, &opts->margin); break;
		case OPT_BORDER: ok = parse_int(optarg, &opts->border); break;
		case OPT_MAX_SCALE: {
			char *end;
			opts->max_scale = strtod(optarg, &end);
			ok = *end == '\0' && end != optarg && opts->max_scale > 0;
			break;
		}
		case 'h':
			fputs(usage_text, stdout);
			*exit_code = 0;
			return false;
		case 'V':
			puts("wl-expose " WL_EXPOSE_VERSION);
			*exit_code = 0;
			return false;
		default:
			fputs(usage_text, stderr);
			*exit_code = 1;
			return false;
		}
		if (!ok) {
			fprintf(stderr, "wl-expose: invalid value for option: '%s'\n", optarg);
			*exit_code = 1;
			return false;
		}
	}
	opts->ids = argv + optind;
	opts->n_ids = (size_t)(argc - optind);
	return true;
}

// ---------------------------------------------------------------------------
// selection

ptrdiff_t app_window_index(struct app *app, struct window *win) {
	for (size_t i = 0; i < app->n_windows; i++)
		if (app->windows[i] == win)
			return (ptrdiff_t)i;
	return -1;
}

void app_select(struct app *app, struct window *win) {
	if (app->selected == win)
		return;
	app->selected = win;
	overlay_update_highlight(app);
}

void app_pick(struct app *app, struct window *win) {
	if (!app->running)
		return;
	free(app->picked);
	app->picked = strdup(win->tl->identifier ? win->tl->identifier : "");
	app->exit_code = 0;
	app->running = false;
}

void app_cancel(struct app *app) {
	if (!app->running)
		return;
	app->exit_code = 1;
	app->running = false;
}

static void window_destroy(struct window *win) {
	capture_finish(&win->cap);
	overlay_window_finish(win);
	if (win->tl)
		win->tl->window = NULL;
	free(win);
}

static void on_toplevel_closed(void *data, struct toplevel *tl) {
	struct app *app = data;
	struct window *win = tl->window;
	if (!win)
		return;
	ptrdiff_t idx = app_window_index(app, win);
	if (idx < 0)
		return;

	memmove(&app->windows[idx], &app->windows[idx + 1],
		(app->n_windows - (size_t)idx - 1) * sizeof(*app->windows));
	app->n_windows--;
	if (app->selected == win) {
		size_t next = (size_t)idx < app->n_windows ? (size_t)idx : app->n_windows - 1;
		app->selected = app->n_windows ? app->windows[next] : NULL;
	}
	window_destroy(win);

	if (app->n_windows == 0) {
		fprintf(stderr, "wl-expose: all windows closed\n");
		app_cancel(app);
		return;
	}
	overlay_relayout(app);
}

static bool create_windows(struct app *app) {
	size_t cap = (size_t)wl_list_length(&app->toplevels.toplevels);
	struct toplevel **selected = calloc(cap ? cap : 1, sizeof(*selected));
	if (!selected)
		abort();
	size_t n = toplevel_select(&app->toplevels.toplevels, app->opts.ids,
		app->opts.n_ids, selected, stderr);
	if (n == 0) {
		fprintf(stderr, "wl-expose: no windows to show\n");
		free(selected);
		return false;
	}

	app->windows = calloc(n, sizeof(*app->windows));
	if (!app->windows)
		abort();
	for (size_t i = 0; i < n; i++) {
		struct window *win = calloc(1, sizeof(*win));
		if (!win)
			abort();
		win->app = app;
		win->tl = selected[i];
		selected[i]->window = win;
		app->windows[i] = win;
	}
	app->n_windows = n;
	free(selected);

	// The first window passed in starts selected, unless --select says otherwise.
	app->selected = app->windows[0];
	if (app->opts.select_id) {
		bool found = false;
		for (size_t i = 0; i < n && !found; i++) {
			const char *id = app->windows[i]->tl->identifier;
			if (id && strcmp(id, app->opts.select_id) == 0) {
				app->selected = app->windows[i];
				found = true;
			}
		}
		if (!found)
			fprintf(stderr, "wl-expose: --select: no shown window with ID '%s'\n",
				app->opts.select_id);
	}
	return true;
}

// ---------------------------------------------------------------------------

static int run(struct app *app) {
	app->running = true;
	app->exit_code = 1;

	if (!overlay_init(app))
		return 1;
	for (size_t i = 0; i < app->n_windows; i++) {
		overlay_window_init(app->windows[i]);
		capture_start(app->windows[i]);
	}
	input_init(app);
	app->map_deadline = now_ms() + app->opts.timeout_ms;

	while (app->running) {
		overlay_maybe_map(app);

		int timeout = input_repeat_timeout(app);
		if (!app->overlay.mapped && app->overlay.configured) {
			double dt = app->map_deadline - now_ms();
			int map_timeout = dt > 0 ? (int)dt + 1 : 0;
			if (timeout < 0 || map_timeout < timeout)
				timeout = map_timeout;
		}

		if (wl_dispatch_timeout(app->g.display, timeout) < 0) {
			fprintf(stderr, "wl-expose: wayland connection error: %s\n", strerror(errno));
			app->exit_code = 1;
			break;
		}
		input_repeat(app);
	}

	input_finish(app);
	for (size_t i = 0; i < app->n_windows; i++)
		window_destroy(app->windows[i]);
	free(app->windows);
	app->windows = NULL;
	app->n_windows = 0;
	app->selected = NULL;
	overlay_finish(app);
	// Make sure the overlay is gone before the caller acts on the result.
	wl_display_roundtrip(app->g.display);
	return app->exit_code;
}

int main(int argc, char **argv) {
	struct app app = {
		.opts = {
			.live = true,
			.timeout_ms = 500,
			.background = 0x000000b3,
			.highlight = 0x4c8dffff,
			.placeholder = 0x3a3a3aff,
			.gap = 24,
			.margin = 48,
			.border = 4,
			.max_scale = 1.0,
		},
	};

	int exit_code = 1;
	if (!parse_options(argc, argv, &app.opts, &exit_code))
		return exit_code;

	if (!globals_init(&app.g))
		return 1;
	if (!globals_check(&app.g, !app.opts.list)) {
		globals_finish(&app.g);
		return 1;
	}

	toplevel_list_init(&app.toplevels, app.g.toplevel_list);
	app.g.toplevel_list = NULL; // owned by app.toplevels now
	app.toplevels.on_closed = on_toplevel_closed;
	app.toplevels.data = &app;
	// The first roundtrip delivers the handles, the second their state.
	wl_display_roundtrip(app.g.display);
	wl_display_roundtrip(app.g.display);

	if (app.opts.list) {
		toplevel_print_list(&app.toplevels.toplevels, stdout);
		exit_code = 0;
	} else if (create_windows(&app)) {
		exit_code = run(&app);
	}

	toplevel_list_finish(&app.toplevels);
	globals_finish(&app.g);

	if (exit_code == 0 && app.picked)
		printf("%s\n", app.picked);
	free(app.picked);
	return exit_code;
}

