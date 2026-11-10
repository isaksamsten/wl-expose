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
	double spacing;   // gap between thumbnails
	double max_scale; // thumbnails are never scaled above this (<= 0: no cap)
};

// Native-relative rows, with a small-window boost based on area.h, fitted with
// a shared scale. Items stay in input order; out has n entries.
void layout_rows(const struct layout_item *items, size_t n,
	struct layout_rect area, double spacing, double max_scale,
	struct layout_rect *out);

// Lay out all items in input order using native-relative rows.
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

