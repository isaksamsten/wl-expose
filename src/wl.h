#pragma once

#include <stdbool.h>
#include <stdint.h>
#include <wayland-client.h>

struct wp_viewporter;
struct wp_single_pixel_buffer_manager_v1;
struct wp_cursor_shape_manager_v1;
struct zwlr_layer_shell_v1;
struct ext_foreign_toplevel_list_v1;
struct ext_foreign_toplevel_image_capture_source_manager_v1;
struct ext_image_copy_capture_manager_v1;

struct globals {
	struct wl_display *display;
	struct wl_registry *registry;
	struct wl_compositor *compositor;
	struct wl_subcompositor *subcompositor;
	struct wl_shm *shm;
	struct wl_seat *seat; // first seat only, bound by input_init()
	uint32_t seat_name, seat_version;
	struct wp_viewporter *viewporter;
	struct wp_single_pixel_buffer_manager_v1 *single_pixel; // optional
	struct wp_cursor_shape_manager_v1 *cursor_shape; // optional
	struct zwlr_layer_shell_v1 *layer_shell;
	struct ext_foreign_toplevel_list_v1 *toplevel_list;
	struct ext_foreign_toplevel_image_capture_source_manager_v1 *source_manager;
	struct ext_image_copy_capture_manager_v1 *copy_manager;
};

// Connect and bind globals. Returns false if the display is unavailable.
bool globals_init(struct globals *g);
// Print missing globals. The overlay globals are only required if overlay.
bool globals_check(const struct globals *g, bool overlay);
void globals_finish(struct globals *g);

double now_ms(void);

// Flush, then dispatch events, waiting at most timeout_ms (-1: forever).
// Returns >0 if events were read, 0 on timeout, -1 on error.
int wl_dispatch_timeout(struct wl_display *display, int timeout_ms);

