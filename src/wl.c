#include "wl.h"

#include <errno.h>
#include <poll.h>
#include <stdio.h>
#include <string.h>
#include <time.h>

#include "cursor-shape-v1-client-protocol.h"
#include "ext-foreign-toplevel-list-v1-client-protocol.h"
#include "ext-image-capture-source-v1-client-protocol.h"
#include "ext-image-copy-capture-v1-client-protocol.h"
#include "single-pixel-buffer-v1-client-protocol.h"
#include "viewporter-client-protocol.h"
#include "wlr-layer-shell-unstable-v1-client-protocol.h"

#define MIN(a, b) ((a) < (b) ? (a) : (b))

double now_ms(void) {
	struct timespec ts;
	clock_gettime(CLOCK_MONOTONIC, &ts);
	return ts.tv_sec * 1e3 + ts.tv_nsec / 1e6;
}

int wl_dispatch_timeout(struct wl_display *display, int timeout_ms) {
	while (wl_display_prepare_read(display) != 0) {
		if (wl_display_dispatch_pending(display) < 0)
			return -1;
	}
	while (wl_display_flush(display) < 0) {
		if (errno != EAGAIN) {
			wl_display_cancel_read(display);
			return -1;
		}
		struct pollfd out = {.fd = wl_display_get_fd(display), .events = POLLOUT};
		poll(&out, 1, -1);
	}

	struct pollfd pfd = {.fd = wl_display_get_fd(display), .events = POLLIN};
	int ret = poll(&pfd, 1, timeout_ms);
	if (ret <= 0) {
		wl_display_cancel_read(display);
		return ret < 0 && errno != EINTR ? -1 : 0;
	}
	if (wl_display_read_events(display) < 0)
		return -1;
	return wl_display_dispatch_pending(display) < 0 ? -1 : 1;
}

static void registry_global(void *data, struct wl_registry *registry,
		uint32_t name, const char *interface, uint32_t version) {
	struct globals *g = data;
	if (strcmp(interface, wl_compositor_interface.name) == 0) {
		g->compositor = wl_registry_bind(registry, name, &wl_compositor_interface,
			MIN(version, 6));
	} else if (strcmp(interface, wl_subcompositor_interface.name) == 0) {
		g->subcompositor = wl_registry_bind(registry, name,
			&wl_subcompositor_interface, 1);
	} else if (strcmp(interface, wl_shm_interface.name) == 0) {
		g->shm = wl_registry_bind(registry, name, &wl_shm_interface, 1);
	} else if (strcmp(interface, wl_seat_interface.name) == 0) {
		// Bound by input_init(), once a listener is ready.
		if (!g->seat_name) {
			g->seat_name = name;
			g->seat_version = MIN(version, 7);
		}
	} else if (strcmp(interface, wp_viewporter_interface.name) == 0) {
		g->viewporter = wl_registry_bind(registry, name, &wp_viewporter_interface, 1);
	} else if (strcmp(interface, wp_single_pixel_buffer_manager_v1_interface.name) == 0) {
		g->single_pixel = wl_registry_bind(registry, name,
			&wp_single_pixel_buffer_manager_v1_interface, 1);
	} else if (strcmp(interface, wp_cursor_shape_manager_v1_interface.name) == 0) {
		g->cursor_shape = wl_registry_bind(registry, name,
			&wp_cursor_shape_manager_v1_interface, 1);
	} else if (strcmp(interface, zwlr_layer_shell_v1_interface.name) == 0) {
		g->layer_shell = wl_registry_bind(registry, name,
			&zwlr_layer_shell_v1_interface, MIN(version, 4));
	} else if (strcmp(interface, ext_foreign_toplevel_list_v1_interface.name) == 0) {
		g->toplevel_list = wl_registry_bind(registry, name,
			&ext_foreign_toplevel_list_v1_interface, 1);
	} else if (strcmp(interface,
			ext_foreign_toplevel_image_capture_source_manager_v1_interface.name) == 0) {
		g->source_manager = wl_registry_bind(registry, name,
			&ext_foreign_toplevel_image_capture_source_manager_v1_interface, 1);
	} else if (strcmp(interface, ext_image_copy_capture_manager_v1_interface.name) == 0) {
		g->copy_manager = wl_registry_bind(registry, name,
			&ext_image_copy_capture_manager_v1_interface, 1);
	}
}

static void registry_global_remove(void *data, struct wl_registry *registry,
		uint32_t name) {
	(void)data;
	(void)registry;
	(void)name;
}

static const struct wl_registry_listener registry_listener = {
	.global = registry_global,
	.global_remove = registry_global_remove,
};

bool globals_init(struct globals *g) {
	g->display = wl_display_connect(NULL);
	if (!g->display) {
		fprintf(stderr, "wl-expose: failed to connect to wayland display\n");
		return false;
	}
	g->registry = wl_display_get_registry(g->display);
	wl_registry_add_listener(g->registry, &registry_listener, g);
	wl_display_roundtrip(g->display);
	return true;
}

#define REQUIRE(field, name) \
	do { \
		if (!g->field) { \
			fprintf(stderr, "wl-expose: missing global: %s\n", name); \
			ok = false; \
		} \
	} while (0)

bool globals_check(const struct globals *g, bool overlay) {
	bool ok = true;
	REQUIRE(toplevel_list, "ext_foreign_toplevel_list_v1");
	if (!overlay)
		return ok;
	REQUIRE(compositor, "wl_compositor");
	REQUIRE(subcompositor, "wl_subcompositor");
	REQUIRE(shm, "wl_shm");
	REQUIRE(viewporter, "wp_viewporter");
	REQUIRE(layer_shell, "zwlr_layer_shell_v1");
	REQUIRE(source_manager, "ext_foreign_toplevel_image_capture_source_manager_v1");
	REQUIRE(copy_manager, "ext_image_copy_capture_manager_v1");
	if (g->compositor && wl_compositor_get_version(g->compositor) < 4) {
		fprintf(stderr, "wl-expose: wl_compositor version 4 or newer required\n");
		ok = false;
	}
	return ok;
}

void globals_finish(struct globals *g) {
	if (g->copy_manager)
		ext_image_copy_capture_manager_v1_destroy(g->copy_manager);
	if (g->source_manager)
		ext_foreign_toplevel_image_capture_source_manager_v1_destroy(g->source_manager);
	if (g->layer_shell) {
		if (zwlr_layer_shell_v1_get_version(g->layer_shell) >=
				ZWLR_LAYER_SHELL_V1_DESTROY_SINCE_VERSION)
			zwlr_layer_shell_v1_destroy(g->layer_shell);
		else
			wl_proxy_destroy((struct wl_proxy *)g->layer_shell);
	}
	if (g->cursor_shape)
		wp_cursor_shape_manager_v1_destroy(g->cursor_shape);
	if (g->single_pixel)
		wp_single_pixel_buffer_manager_v1_destroy(g->single_pixel);
	if (g->viewporter)
		wp_viewporter_destroy(g->viewporter);
	if (g->seat) {
		if (wl_seat_get_version(g->seat) >= WL_SEAT_RELEASE_SINCE_VERSION)
			wl_seat_release(g->seat);
		else
			wl_seat_destroy(g->seat);
	}
	if (g->shm)
		wl_shm_destroy(g->shm);
	if (g->subcompositor)
		wl_subcompositor_destroy(g->subcompositor);
	if (g->compositor)
		wl_compositor_destroy(g->compositor);
	if (g->registry)
		wl_registry_destroy(g->registry);
	if (g->display)
		wl_display_disconnect(g->display);
}

