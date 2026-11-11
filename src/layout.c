#include "layout.h"

#include <math.h>
#include <stdbool.h>
#include <stdio.h>
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
                         const size_t *starts, size_t rows,
                         struct layout_rect area, double spacing) {
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
      out[i] =
          (struct layout_rect){area.x + area.w / 2, area.y + area.h / 2, 0, 0};
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

// Natural placement adapted from native_win.js, GPL-2.0-or-later.
// SPDX-FileCopyrightText: 2011 Giovanni Campagna <gcampagna@src.gnome.org>
// SPDX-FileCopyrightText: 2011 Stefano Facchini <stefano.facchini@gmail.com>
// SPDX-FileCopyrightText: 2011 Wepmaschda <wepmaschda@gmx.de>
// SPDX-FileCopyrightText: 2015 Florian Müllner <fmuellner@gnome.org>
// SPDX-FileCopyrightText: 2024 Marko Bakšić <markonius@pm.me>
// SPDX-License-Identifier: GPL-2.0-or-later
static struct layout_rect layout_bounds(const struct layout_rect *rects,
                                        size_t n) {
  double x0 = INFINITY, y0 = INFINITY, x1 = -INFINITY, y1 = -INFINITY;
  for (size_t i = 0; i < n; i++) {
    x0 = fmin(x0, rects[i].x);
    y0 = fmin(y0, rects[i].y);
    x1 = fmax(x1, rects[i].x + rects[i].w);
    y1 = fmax(y1, rects[i].y + rects[i].h);
  }
  return (struct layout_rect){x0, y0, x1 - x0, y1 - y0};
}

// The half-gap threshold and shortest signed dodge match Rect.getDodgeTarget.
// The target gap is deliberately larger than the stopping clearance; the
// whole arrangement, including its gaps, is scaled only after separation.
static bool dodge_target(struct layout_rect a, struct layout_rect b,
                         double aspect_force, double gap, double *dx,
                         double *dy) {
  double left = a.x + a.w - b.x + gap;
  double right = b.x + b.w - a.x + gap;
  double up = a.y + a.h - b.y + gap;
  double down = b.y + b.h - a.y + gap;
  *dx = *dy = 0;
  if (left <= gap / 2 || right <= gap / 2 || up <= gap / 2 || down <= gap / 2)
    return false;
  double horizontal = left < right ? -left : right;
  double vertical = up < down ? -up : down;
  if (fabs(horizontal / aspect_force) < fabs(vertical))
    *dx = horizontal;
  else
    *dy = vertical;
  return true;
}

static bool arrange_layout(struct layout_rect *rects, size_t n, double aspect,
                           double gap) {
  for (int pass = 0; pass < 500; pass++) {
    struct layout_rect bounds = layout_bounds(rects, n);
    double aspect_force = pow(aspect / (bounds.w / bounds.h), 2);
    bool any_overlap = false;
    for (size_t i = 0; i < n; i++) {
      // Deliberately visit both (i,j) and (j,i), as in the JS.
      for (size_t j = 0; j < n; j++) {
        if (i == j)
          continue;
        double dx, dy;
        if (!dodge_target(rects[i], rects[j], aspect_force, gap, &dx, &dy))
          continue;
        any_overlap = true;
        rects[i].x += dx * 0.2;
        rects[j].x -= dx * 0.2;
        rects[i].y += dy * 0.2;
        rects[j].y -= dy * 0.2;
      }
    }
    if (!any_overlap)
      return true;
  }
  return false;
}

// The damped solver can cycle until its iteration limit. Resolve any remaining
// collisions with a monotone sweep before fitting, rather than displaying them.
// Earlier rectangles never move, so later placements cannot undo separation.
static void finish_separation(struct layout_rect *rects, size_t n,
                              double aspect, double gap) {
  bool collision = false;
  for (size_t i = 0; i < n; i++)
    for (size_t j = 0; j < i; j++) {
      double dx, dy;
      if (dodge_target(rects[i], rects[j], 1, gap, &dx, &dy))
        collision = true;
    }
  if (!collision)
    return;
  struct layout_rect bounds = layout_bounds(rects, n);
  bool horizontal = bounds.w / bounds.h < aspect;
  size_t *order = malloc(n * sizeof(*order));
  if (!order)
    abort();
  for (size_t i = 0; i < n; i++) {
    double center = horizontal ? rects[i].x + rects[i].w / 2
                               : rects[i].y + rects[i].h / 2;
    size_t k = i;
    while (k > 0) {
      struct layout_rect prev = rects[order[k - 1]];
      double prev_center = horizontal ? prev.x + prev.w / 2
                                      : prev.y + prev.h / 2;
      if (prev_center <= center)
        break;
      order[k] = order[k - 1];
      k--;
    }
    order[k] = i;
  }
  // Match the natural solver's stopping clearance, with a numerical cushion
  // for zero-gap layouts and the subsequent shared scaling operation.
  double clearance = gap / 2 + 1e-9 * fmax(bounds.w, bounds.h);
  for (size_t i = 0; i < n; i++) {
    struct layout_rect *r = &rects[order[i]];
    for (size_t j = 0; j < i; j++) {
      struct layout_rect prev = rects[order[j]];
      if (horizontal) {
        if (r->y < prev.y + prev.h + clearance &&
            prev.y < r->y + r->h + clearance)
          r->x = fmax(r->x, prev.x + prev.w + clearance);
      } else {
        if (r->x < prev.x + prev.w + clearance &&
            prev.x < r->x + r->w + clearance)
          r->y = fmax(r->y, prev.y + prev.h + clearance);
      }
    }
  }
  free(order);
}

// Keep the natural layout's pairwise ordering. Each pair contributes one
// horizontal or vertical separation constraint; center order makes each axis
// acyclic, so a forward sweep satisfies all its constraints without relaxation.
static bool horizontal_pair(struct layout_rect a, struct layout_rect b) {
  bool x_clear = a.x + a.w <= b.x || b.x + b.w <= a.x;
  bool y_clear = a.y + a.h <= b.y || b.y + b.h <= a.y;
  if (!x_clear)
    return false;
  if (!y_clear)
    return true;
  return fabs(a.x + a.w / 2 - b.x - b.w / 2) / (a.w + b.w) >=
         fabs(a.y + a.h / 2 - b.y - b.h / 2) / (a.h + b.h);
}

static void center_order(const struct layout_rect *rects, size_t n,
                         bool horizontal, size_t *order) {
  for (size_t i = 0; i < n; i++) {
    double center = horizontal ? rects[i].x + rects[i].w / 2
                               : rects[i].y + rects[i].h / 2;
    size_t k = i;
    while (k > 0) {
      struct layout_rect prev = rects[order[k - 1]];
      double previous = horizontal ? prev.x + prev.w / 2
                                   : prev.y + prev.h / 2;
      if (previous <= center)
        break;
      order[k] = order[k - 1];
      k--;
    }
    order[k] = i;
  }
}

static struct layout_rect place_with_gap(const struct layout_rect *seed,
                                        size_t n, double scale, double gap,
                                        const size_t *x_order,
                                        const size_t *y_order,
                                        struct layout_rect *out) {
  struct layout_rect bounds = layout_bounds(seed, n);
  for (size_t i = 0; i < n; i++)
    out[i] = (struct layout_rect){(seed[i].x - bounds.x) * scale,
                                 (seed[i].y - bounds.y) * scale,
                                 seed[i].w * scale, seed[i].h * scale};
  for (size_t i = 0; i < n; i++) {
    size_t x = x_order[i], y = y_order[i];
    for (size_t j = 0; j < i; j++) {
      size_t px = x_order[j], py = y_order[j];
      if (horizontal_pair(seed[x], seed[px]))
        out[x].x = fmax(out[x].x, out[px].x + out[px].w + gap);
      if (!horizontal_pair(seed[y], seed[py]))
        out[y].y = fmax(out[y].y, out[py].y + out[py].h + gap);
    }
  }
  return layout_bounds(out, n);
}

// Gently dilate centers uniformly, preserving the natural composition and
// thumbnail sizes. Use only part of the available headroom, not edge-to-edge
// stretching. Dilation can only increase existing separating distances.
static void spread_layout(struct layout_rect *rects, size_t n,
                          struct layout_rect area) {
  struct layout_rect bounds = layout_bounds(rects, n);
  double cx = bounds.x + bounds.w / 2, cy = bounds.y + bounds.h / 2;
  double sx = INFINITY, sy = INFINITY;
  for (size_t i = 0; i < n; i++) {
    double dx = fabs(rects[i].x + rects[i].w / 2 - cx);
    double dy = fabs(rects[i].y + rects[i].h / 2 - cy);
    if (dx > 1e-9)
      sx = fmin(sx, (area.w - rects[i].w) / (2 * dx));
    if (dy > 1e-9)
      sy = fmin(sy, (area.h - rects[i].h) / (2 * dy));
  }
  double available = fmin(sx, sy);
  double spread = isfinite(available)
                      ? 1 + 0.75 * (fmin(1.35, fmax(1, available)) - 1)
                      : 1;
  for (size_t i = 0; i < n; i++) {
    double dx = rects[i].x + rects[i].w / 2 - cx;
    double dy = rects[i].y + rects[i].h / 2 - cy;
    rects[i].x = area.x + area.w / 2 + dx * spread - rects[i].w / 2;
    rects[i].y = area.y + area.h / 2 + dy * spread - rects[i].h / 2;
  }
}

void layout_compute(const struct layout_item *items, size_t n,
                    struct layout_rect area, const struct layout_params *params,
                    struct layout_rect *out) {
  if (n == 0)
    return;
  if (area.w <= 0 || area.h <= 0) {
    for (size_t i = 0; i < n; i++)
      out[i] =
          (struct layout_rect){area.x + area.w / 2, area.y + area.h / 2, 0, 0};
    return;
  }

  double total = 0;
  for (size_t i = 0; i < n; i++) {
    out[i] = (struct layout_rect){0, 0, item_w(&items[i]), item_h(&items[i])};
    total += out[i].w * out[i].h;
  }
  double aspect = area.w / area.h;
  // Unlike GNOME, we have no original desktop coordinates. Start with one
  // compact, deterministic sunflower of native-sized windows instead.
  for (size_t i = 0; i < n; i++) {
    double angle = (double)i * 2.399963229728653;
    double radius =
        n == 1 ? 0 : 0.35 * sqrt(total * ((double)i + 0.5) / (double)n);
    out[i].x = radius * sqrt(aspect) * cos(angle) - out[i].w / 2;
    out[i].y = radius / sqrt(aspect) * sin(angle) - out[i].h / 2;
  }
  // The native-space target shapes the natural arrangement, not the minimum
  // screen-space gap. Reserve the latter separately during fitting below.
  if (!arrange_layout(out, n, aspect, 50)) {
    fprintf(stderr,
            "wl-expose: natural layout reached 500 separation passes "
            "for %zu windows; finishing separation before fitting\n",
            n);
    finish_separation(out, n, aspect, 50);
  }

  struct layout_rect *seed = malloc(n * sizeof(*seed));
  struct layout_rect *trial = malloc(n * sizeof(*trial));
  size_t *x_order = malloc(n * sizeof(*x_order));
  size_t *y_order = malloc(n * sizeof(*y_order));
  if (!seed || !trial || !x_order || !y_order)
    abort();
  for (size_t i = 0; i < n; i++)
    seed[i] = out[i];
  center_order(seed, n, true, x_order);
  center_order(seed, n, false, y_order);

  double gap = fmax(0, params->spacing);
  struct layout_rect minimum =
      place_with_gap(seed, n, 0, gap, x_order, y_order, trial);
  if (minimum.w >= area.w || minimum.h >= area.h) {
    // Even zero-sized thumbnails cannot fit this gap with the natural ordering.
    // Keep the result usable, but never silently weaken the requested minimum.
    double fit = fmin(minimum.w > 0 ? area.w / minimum.w : INFINITY,
                      minimum.h > 0 ? area.h / minimum.h : INFINITY);
    double requested = gap;
    gap *= fit * 0.95;
    fprintf(stderr,
            "wl-expose: minimum gap %.2f cannot fit the natural ordering; "
            "reducing to %.2f\n", requested, gap);
  }

  minimum = place_with_gap(seed, n, 0, gap, x_order, y_order, trial);
  // Reserve a little of the size budget for breathing room. Fixed gap costs
  // are excluded, so this remains feasible even in crowded layouts.
  double fill = n == 1 ? 1 : 0.9;
  double fit_w = minimum.w + fill * (area.w - minimum.w);
  double fit_h = minimum.h + fill * (area.h - minimum.h);

  struct layout_rect bounds = layout_bounds(seed, n);
  double hi = fmin(area.w / bounds.w, area.h / bounds.h);
  if (params->max_scale > 0)
    hi = fmin(hi, params->max_scale);
  double lo = 0;
  // Constraint lengths grow monotonically with scale. Fit within the size
  // budget while keeping the minimum gap in final logical pixels.
  for (int step = 0; step < 48; step++) {
    double scale = (lo + hi) / 2;
    bounds = place_with_gap(seed, n, scale, gap, x_order, y_order, trial);
    if (bounds.w <= fit_w && bounds.h <= fit_h)
      lo = scale;
    else
      hi = scale;
  }
  place_with_gap(seed, n, lo, gap, x_order, y_order, out);
  spread_layout(out, n, area);
  free(seed);
  free(trial);
  free(x_order);
  free(y_order);
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
    case LAYOUT_LEFT:
      along = -dx;
      across = dy;
      break;
    case LAYOUT_RIGHT:
      along = dx;
      across = dy;
      break;
    case LAYOUT_UP:
      along = -dy;
      across = dx;
      break;
    default:
      along = dy;
      across = dx;
      break;
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
