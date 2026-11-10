#include "toplevel.h"

#include <stdlib.h>
#include <string.h>

#include "ext-foreign-toplevel-list-v1-client-protocol.h"

static void set_str(char **dst, const char *src) {
	free(*dst);
	*dst = strdup(src ? src : "");
}

static void toplevel_free(struct toplevel *tl) {
	if (tl->handle)
		ext_foreign_toplevel_handle_v1_destroy(tl->handle);
	wl_list_remove(&tl->link);
	free(tl->title);
	free(tl->app_id);
	free(tl->identifier);
	free(tl);
}

static void handle_closed(void *data, struct ext_foreign_toplevel_handle_v1 *h) {
	(void)h;
	struct toplevel *tl = data;
	tl->closed = true;
	if (tl->list->on_closed)
		tl->list->on_closed(tl->list->data, tl);
	toplevel_free(tl);
}

static void handle_done(void *data, struct ext_foreign_toplevel_handle_v1 *h) {
	(void)h;
	((struct toplevel *)data)->done = true;
}

static void handle_title(void *data, struct ext_foreign_toplevel_handle_v1 *h,
		const char *title) {
	(void)h;
	set_str(&((struct toplevel *)data)->title, title);
}

static void handle_app_id(void *data, struct ext_foreign_toplevel_handle_v1 *h,
		const char *app_id) {
	(void)h;
	set_str(&((struct toplevel *)data)->app_id, app_id);
}

static void handle_identifier(void *data, struct ext_foreign_toplevel_handle_v1 *h,
		const char *identifier) {
	(void)h;
	set_str(&((struct toplevel *)data)->identifier, identifier);
}

static const struct ext_foreign_toplevel_handle_v1_listener handle_listener = {
	.closed = handle_closed,
	.done = handle_done,
	.title = handle_title,
	.app_id = handle_app_id,
	.identifier = handle_identifier,
};

static void list_toplevel(void *data, struct ext_foreign_toplevel_list_v1 *list,
		struct ext_foreign_toplevel_handle_v1 *handle) {
	(void)list;
	struct toplevel_list *l = data;
	struct toplevel *tl = calloc(1, sizeof(*tl));
	if (!tl)
		abort();
	tl->list = l;
	tl->handle = handle;
	ext_foreign_toplevel_handle_v1_add_listener(handle, &handle_listener, tl);
	wl_list_insert(l->toplevels.prev, &tl->link);
}

static void list_finished(void *data, struct ext_foreign_toplevel_list_v1 *list) {
	(void)data;
	(void)list;
}

static const struct ext_foreign_toplevel_list_v1_listener list_listener = {
	.toplevel = list_toplevel,
	.finished = list_finished,
};

void toplevel_list_init(struct toplevel_list *l, struct ext_foreign_toplevel_list_v1 *list) {
	l->list = list;
	wl_list_init(&l->toplevels);
	ext_foreign_toplevel_list_v1_add_listener(list, &list_listener, l);
}

void toplevel_list_finish(struct toplevel_list *l) {
	struct toplevel *tl, *tmp;
	wl_list_for_each_safe(tl, tmp, &l->toplevels, link)
		toplevel_free(tl);
	if (l->list) {
		ext_foreign_toplevel_list_v1_stop(l->list);
		ext_foreign_toplevel_list_v1_destroy(l->list);
		l->list = NULL;
	}
}

static bool is_selected(struct toplevel **out, size_t count, const struct toplevel *tl) {
	for (size_t i = 0; i < count; i++)
		if (out[i] == tl)
			return true;
	return false;
}

size_t toplevel_select(struct wl_list *toplevels, char *const *ids, size_t n_ids,
		struct toplevel **out, FILE *warn) {
	size_t count = 0;
	struct toplevel *tl;

	if (n_ids == 0) {
		wl_list_for_each(tl, toplevels, link)
			if (!tl->closed)
				out[count++] = tl;
		return count;
	}

	for (size_t i = 0; i < n_ids; i++) {
		struct toplevel *match = NULL;
		wl_list_for_each(tl, toplevels, link) {
			if (!tl->closed && tl->identifier && strcmp(tl->identifier, ids[i]) == 0) {
				match = tl;
				break;
			}
		}
		if (!match) {
			if (warn)
				fprintf(warn, "wl-expose: no window with ID '%s'\n", ids[i]);
			continue;
		}
		if (!is_selected(out, count, match))
			out[count++] = match;
	}
	return count;
}

// Tabs and newlines would break the line/column format.
static void print_field(FILE *out, const char *s) {
	for (; s && *s; s++)
		fputc(*s == '\t' || *s == '\n' || *s == '\r' ? ' ' : *s, out);
}

void toplevel_print_list(struct wl_list *toplevels, FILE *out) {
	struct toplevel *tl;
	wl_list_for_each(tl, toplevels, link) {
		if (tl->closed)
			continue;
		print_field(out, tl->identifier);
		fputc('\t', out);
		print_field(out, tl->app_id);
		fputc('\t', out);
		print_field(out, tl->title);
		fputc('\n', out);
	}
}

