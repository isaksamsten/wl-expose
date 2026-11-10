#pragma once

#include <stdbool.h>
#include <stddef.h>
#include <stdio.h>
#include <wayland-client.h>

struct ext_foreign_toplevel_list_v1;
struct ext_foreign_toplevel_handle_v1;
struct window;

struct toplevel {
	struct wl_list link; // toplevel_list.toplevels, announcement order
	struct toplevel_list *list;
	struct ext_foreign_toplevel_handle_v1 *handle;
	char *title;
	char *app_id;
	char *identifier;
	bool done;   // initial state received
	bool closed;
	struct window *window; // set while shown in the overlay
};

struct toplevel_list {
	struct ext_foreign_toplevel_list_v1 *list;
	struct wl_list toplevels; // struct toplevel.link

	// Called when a toplevel closes, right before it is freed.
	void (*on_closed)(void *data, struct toplevel *tl);
	void *data;
};

void toplevel_list_init(struct toplevel_list *l, struct ext_foreign_toplevel_list_v1 *list);
void toplevel_list_finish(struct toplevel_list *l);

// Pick the toplevels to show. With n_ids == 0 that's every open toplevel in
// announcement order; otherwise the toplevels whose identifier exactly matches
// one of ids, in the order of ids. Unknown IDs print a warning to warn (if
// not NULL); duplicates are shown once. out needs room for one entry per
// toplevel in the list. Returns the number of entries written.
size_t toplevel_select(struct wl_list *toplevels, char *const *ids, size_t n_ids,
	struct toplevel **out, FILE *warn);

// Print identifier<TAB>app_id<TAB>title for every open toplevel.
void toplevel_print_list(struct wl_list *toplevels, FILE *out);

