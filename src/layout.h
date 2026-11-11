#pragma once

// Pure layout code: no Wayland, unit tested.

#include <stddef.h>

struct layout_item {
	double w, h; // native size; non-positive values are treated as 1
};

struct layout_rect {
	double x, y, w, h;
};

struct layout_params {
	double spacing;   // minimum thumbnail gap in final logical pixels (>= 0)
	double max_scale; // thumbnails are never scaled above this (<= 0: no cap)
};

// Native-relative rows, with a small-window boost based on area.h, fitted with
// a shared scale. Items stay in input order; out has n entries.
void layout_rows(const struct layout_item *items, size_t n,
	struct layout_rect area, double spacing, double max_scale,
	struct layout_rect *out);

// Natural placement adapted from native_win.js with screen-space gap constraints.
// Preserves native size ratios and synthetic seed ordering, with a shared scale
// and gentle uniform center spreading instead of edge-to-edge stretching.
// If the minimum gap cannot fit the ordering, reduces it with a stderr warning.
// At the iteration limit, logs to stderr and sweeps away remaining collisions.
// out[i] corresponds to items[i]; computation has no animation state.
void layout_compute(const struct layout_item *items, size_t n,
	struct layout_rect area, const struct layout_params *params,
	struct layout_rect *out);

enum layout_dir {
	LAYOUT_LEFT,
	LAYOUT_RIGHT,
	LAYOUT_UP,
	LAYOUT_DOWN,
};

// Index of the nearest rect from rects[cur] in direction dir, or -1.
int layout_neighbor(const struct layout_rect *rects, size_t n, size_t cur,
	enum layout_dir dir);

