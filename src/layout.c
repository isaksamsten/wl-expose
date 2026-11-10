#include "layout.h"

#include <math.h>
#include <stdbool.h>
#include <stdlib.h>

static double item_w(const struct layout_item *it) {
	return it->w > 0 ? it->w : 1;
}

static double item_h(const struct layout_item *it) {
	return it->h > 0 ? it->h : 1;
}

// Greedily split boosted native widths into at most max_rows balanced rows.
// starts[k] is the first item of row k. Returns the non-empty row count.
static size_t break_rows(const double *widths, size_t n, size_t max_rows,
		double total, size_t *starts) {
	double target = total / (double)max_rows;
	size_t rows = 1;
	double acc = 0;
	starts[0] = 0;
	for (size_t i = 0; i < n; i++) {
		// Start a new row when this item's midpoint lies past the row's end.
		if (i > starts[rows - 1] && rows < max_rows &&
				acc + widths[i] / 2 > target * (double)rows)
			starts[rows++] = i;
		acc += widths[i];
	}
	starts[rows] = n;
	return rows;
}

// Fit boosted native sizes with one shared scale. Spacing stays unscaled.
static double rows_scale(const double *widths, const double *heights,
		const size_t *starts, size_t rows, struct layout_rect area, double spacing) {
	double scale = INFINITY, total_h = 0;
	for (size_t r = 0; r < rows; r++) {
		double row_w = 0, row_h = 0;
		for (size_t i = starts[r]; i < starts[r + 1]; i++) {
			row_w += widths[i];
			row_h = fmax(row_h, heights[i]);
		}
		size_t count = starts[r + 1] - starts[r];
		double avail = area.w - spacing * (double)(count - 1);
		if (avail <= 0)
			return -1;
		scale = fmin(scale, avail / row_w);
		total_h += row_h;
	}
	return fmin(scale, (area.h - spacing * (double)(rows - 1)) / total_h);
}

static double capped_scale(const struct layout_item *it, double preferred_h,
		double scale, double max_scale) {
	double s = scale * preferred_h / item_h(it);
	return max_scale > 0 ? fmin(s, max_scale) : s;
}

void layout_rows(const struct layout_item *items, size_t n,
		struct layout_rect area, double spacing, double max_scale,
		struct layout_rect *out) {
	if (n == 0)
		return;
	if (spacing < 0)
		spacing = 0;

	double *widths = malloc(n * sizeof(*widths));
	double *heights = malloc(n * sizeof(*heights));
	size_t *starts = malloc((n + 1) * sizeof(*starts));
	size_t *best_starts = malloc((n + 1) * sizeof(*best_starts));
	if (!widths || !heights || !starts || !best_starts)
		abort();

	double total = 0;
	for (size_t i = 0; i < n; i++) {
		// GNOME-style boost: small windows get up to 1.5x the shared scale.
		// Use usable height as the reference and clamp oversized windows.
		double ratio = area.h > 0 ? fmin(item_h(&items[i]) / area.h, 1) : 1;
		double boost = 1.5 - 0.5 * ratio;
		widths[i] = item_w(&items[i]) * boost;
		heights[i] = item_h(&items[i]) * boost;
		total += widths[i];
	}

	// Try every row count; keep the one showing the most thumbnail area. If
	// nothing fits (spacing alone exceeds the area), shrink the spacing.
	size_t best_rows = 0;
	double best_score = 0, best_scale = 0;
	for (;;) {
		for (size_t max_rows = 1; max_rows <= n; max_rows++) {
			size_t rows = break_rows(widths, n, max_rows, total, starts);
			if (rows < max_rows)
				continue; // same split as an earlier row count
			double scale = rows_scale(widths, heights, starts, rows, area, spacing);
			if (scale <= 0)
				continue;
			double score = 0;
			for (size_t i = 0; i < n; i++) {
				double s = capped_scale(&items[i], heights[i], scale, max_scale);
				score += item_w(&items[i]) * item_h(&items[i]) * s * s;
			}
			if (score > best_score * (1 + 1e-9)) {
				best_score = score;
				best_scale = scale;
				best_rows = rows;
				for (size_t r = 0; r <= rows; r++)
					best_starts[r] = starts[r];
			}
		}
		if (best_rows > 0 || spacing == 0)
			break;
		spacing = spacing < 1 ? 0 : spacing / 2;
	}

	if (best_rows == 0) {
		// Empty area: collapse everything to its center.
		for (size_t i = 0; i < n; i++)
			out[i] = (struct layout_rect){
				area.x + area.w / 2, area.y + area.h / 2, 0, 0};
		goto done;
	}

	// Row heights after capping, to center the block vertically.
	double block_h = spacing * (double)(best_rows - 1);
	for (size_t r = 0; r < best_rows; r++) {
		double row_h = 0;
		for (size_t i = best_starts[r]; i < best_starts[r + 1]; i++) {
			double s = capped_scale(&items[i], heights[i], best_scale, max_scale);
			row_h = fmax(row_h, item_h(&items[i]) * s);
		}
		block_h += row_h;
	}

	double y = area.y + (area.h - block_h) / 2;
	for (size_t r = 0; r < best_rows; r++) {
		double row_w = 0, row_h = 0;
		for (size_t i = best_starts[r]; i < best_starts[r + 1]; i++) {
			double s = capped_scale(&items[i], heights[i], best_scale, max_scale);
			out[i].h = item_h(&items[i]) * s;
			out[i].w = item_w(&items[i]) * s;
			row_w += out[i].w;
			row_h = fmax(row_h, out[i].h);
		}
		row_w += spacing * (double)(best_starts[r + 1] - best_starts[r] - 1);

		double x = area.x + (area.w - row_w) / 2;
		for (size_t i = best_starts[r]; i < best_starts[r + 1]; i++) {
			out[i].x = x;
			// Center a single row; use a common baseline for multiple rows.
			out[i].y = y + (row_h - out[i].h) / (best_rows == 1 ? 2 : 1);
			x += out[i].w + spacing;
		}
		y += row_h + spacing;
	}

done:
	free(widths);
	free(heights);
	free(starts);
	free(best_starts);
}

void layout_compute(const struct layout_item *items, size_t n,
		struct layout_rect area, const struct layout_params *params,
		struct layout_rect *out) {
	layout_rows(items, n, area, params->spacing, params->max_scale, out);
}

int layout_neighbor(const struct layout_rect *rects, size_t n, size_t cur,
		enum layout_dir dir) {
	if (cur >= n)
		return -1;
	double cx = rects[cur].x + rects[cur].w / 2;
	double cy = rects[cur].y + rects[cur].h / 2;

	int best = -1;
	double best_score = INFINITY;
	for (size_t i = 0; i < n; i++) {
		if (i == cur)
			continue;
		double dx = rects[i].x + rects[i].w / 2 - cx;
		double dy = rects[i].y + rects[i].h / 2 - cy;
		double along, across;
		switch (dir) {
		case LAYOUT_LEFT: along = -dx; across = dy; break;
		case LAYOUT_RIGHT: along = dx; across = dy; break;
		case LAYOUT_UP: along = -dy; across = dx; break;
		default: along = dy; across = dx; break;
		}
		if (along < 0.5)
			continue;
		// Prefer staying in the same row/column over raw distance.
		double score = along + 2 * fabs(across);
		if (score < best_score) {
			best_score = score;
			best = (int)i;
		}
	}
	return best;
}

