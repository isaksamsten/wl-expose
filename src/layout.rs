//! Pure layout code: no Wayland, unit tested.

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Item {
    /// Native size; non-positive values are treated as 1.
    pub w: f64,
    pub h: f64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub const fn new(x: f64, y: f64, w: f64, h: f64) -> Self {
        Self { x, y, w, h }
    }

    fn center_x(&self) -> f64 {
        self.x + self.w / 2.0
    }

    fn center_y(&self) -> f64 {
        self.y + self.h / 2.0
    }

    fn center(&self, horizontal: bool) -> f64 {
        if horizontal {
            self.center_x()
        } else {
            self.center_y()
        }
    }

    /// A zero-sized rect at the center of this one.
    fn collapsed(&self) -> Rect {
        Rect::new(self.center_x(), self.center_y(), 0.0, 0.0)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Params {
    /// Minimum thumbnail gap in final logical pixels (negative: 0).
    pub spacing: f64,
    /// Thumbnails are never scaled above this (<= 0: no cap).
    pub max_scale: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    Left,
    Right,
    Up,
    Down,
}

fn item_w(it: &Item) -> f64 {
    if it.w > 0.0 { it.w } else { 1.0 }
}

fn item_h(it: &Item) -> f64 {
    if it.h > 0.0 { it.h } else { 1.0 }
}

// ---------------------------------------------------------------------------
// rows

/// Greedily split boosted native widths into at most `max_rows` balanced
/// rows. Returns row starts plus a final `widths.len()` sentinel.
fn break_rows(widths: &[f64], max_rows: usize, total: f64) -> Vec<usize> {
    let target = total / max_rows as f64;
    let mut starts = vec![0];
    let mut acc = 0.0;
    for (i, &w) in widths.iter().enumerate() {
        let rows = starts.len();
        // Start a new row when this item's midpoint lies past the row's end.
        if i > starts[rows - 1] && rows < max_rows && acc + w / 2.0 > target * rows as f64 {
            starts.push(i);
        }
        acc += w;
    }
    starts.push(widths.len());
    starts
}

/// Fit boosted native sizes with one shared scale. Spacing stays unscaled.
fn rows_scale(widths: &[f64], heights: &[f64], starts: &[usize], area: Rect, spacing: f64) -> f64 {
    let mut scale = f64::INFINITY;
    let mut total_h = 0.0;
    for row in starts.windows(2) {
        let (a, b) = (row[0], row[1]);
        let row_w: f64 = widths[a..b].iter().sum();
        let row_h = heights[a..b].iter().fold(0.0, |m: f64, &h| m.max(h));
        let avail = area.w - spacing * (b - a - 1) as f64;
        if avail <= 0.0 {
            return -1.0;
        }
        scale = scale.min(avail / row_w);
        total_h += row_h;
    }
    let rows = starts.len() - 1;
    scale.min((area.h - spacing * (rows - 1) as f64) / total_h)
}

fn capped_scale(it: &Item, preferred_h: f64, scale: f64, max_scale: f64) -> f64 {
    let s = scale * preferred_h / item_h(it);
    if max_scale > 0.0 { s.min(max_scale) } else { s }
}

/// Native-relative rows, with a small-window boost based on `area.h`, fitted
/// with a shared scale. Items stay in input order.
#[cfg_attr(not(test), allow(dead_code))]
pub fn rows(items: &[Item], area: Rect, spacing: f64, max_scale: f64) -> Vec<Rect> {
    let n = items.len();
    if n == 0 {
        return Vec::new();
    }
    let mut spacing = spacing.max(0.0);

    // GNOME-style boost: small windows get up to 1.5x the shared scale.
    // Use usable height as the reference and clamp oversized windows.
    let boosts: Vec<f64> = items
        .iter()
        .map(|it| {
            let ratio = if area.h > 0.0 {
                (item_h(it) / area.h).min(1.0)
            } else {
                1.0
            };
            1.5 - 0.5 * ratio
        })
        .collect();
    let widths: Vec<f64> = items
        .iter()
        .zip(&boosts)
        .map(|(it, b)| item_w(it) * b)
        .collect();
    let heights: Vec<f64> = items
        .iter()
        .zip(&boosts)
        .map(|(it, b)| item_h(it) * b)
        .collect();
    let total: f64 = widths.iter().sum();

    // Try every row count; keep the one showing the most thumbnail area. If
    // nothing fits (spacing alone exceeds the area), shrink the spacing.
    let mut best: Option<(f64, Vec<usize>)> = None;
    let mut best_score = 0.0;
    loop {
        for max_rows in 1..=n {
            let starts = break_rows(&widths, max_rows, total);
            if starts.len() - 1 < max_rows {
                continue; // same split as an earlier row count
            }
            let scale = rows_scale(&widths, &heights, &starts, area, spacing);
            if scale <= 0.0 {
                continue;
            }
            let score: f64 = items
                .iter()
                .zip(&heights)
                .map(|(it, &h)| {
                    let s = capped_scale(it, h, scale, max_scale);
                    item_w(it) * item_h(it) * s * s
                })
                .sum();
            if score > best_score * (1.0 + 1e-9) {
                best_score = score;
                best = Some((scale, starts));
            }
        }
        if best.is_some() || spacing == 0.0 {
            break;
        }
        spacing = if spacing < 1.0 { 0.0 } else { spacing / 2.0 };
    }

    // Empty area: collapse everything to its center.
    let Some((scale, starts)) = best else {
        return vec![area.collapsed(); n];
    };
    let rows = starts.len() - 1;
    let size = |i: usize| {
        let s = capped_scale(&items[i], heights[i], scale, max_scale);
        (item_w(&items[i]) * s, item_h(&items[i]) * s)
    };

    // Row heights after capping, to center the block vertically.
    let row_heights: Vec<f64> = starts
        .windows(2)
        .map(|r| (r[0]..r[1]).fold(0.0, |m: f64, i| m.max(size(i).1)))
        .collect();
    let block_h = spacing * (rows - 1) as f64 + row_heights.iter().sum::<f64>();

    let mut out = vec![Rect::default(); n];
    let mut y = area.y + (area.h - block_h) / 2.0;
    for (row, &row_h) in starts.windows(2).zip(&row_heights) {
        let (a, b) = (row[0], row[1]);
        let mut row_w = spacing * (b - a - 1) as f64;
        for (i, r) in out.iter_mut().enumerate().take(b).skip(a) {
            (r.w, r.h) = size(i);
            row_w += r.w;
        }
        let mut x = area.x + (area.w - row_w) / 2.0;
        for r in &mut out[a..b] {
            r.x = x;
            // Center a single row; use a common baseline for multiple rows.
            r.y = y + (row_h - r.h) / if rows == 1 { 2.0 } else { 1.0 };
            x += r.w + spacing;
        }
        y += row_h + spacing;
    }
    out
}

// ---------------------------------------------------------------------------
// Natural placement adapted from native_win.js, GPL-2.0-or-later.
// SPDX-FileCopyrightText: 2011 Giovanni Campagna <gcampagna@src.gnome.org>
// SPDX-FileCopyrightText: 2011 Stefano Facchini <stefano.facchini@gmail.com>
// SPDX-FileCopyrightText: 2011 Wepmaschda <wepmaschda@gmx.de>
// SPDX-FileCopyrightText: 2015 Florian Müllner <fmuellner@gnome.org>
// SPDX-FileCopyrightText: 2024 Marko Bakšić <markonius@pm.me>
// SPDX-License-Identifier: GPL-2.0-or-later

fn bounds(rects: &[Rect]) -> Rect {
    let (mut x0, mut y0) = (f64::INFINITY, f64::INFINITY);
    let (mut x1, mut y1) = (f64::NEG_INFINITY, f64::NEG_INFINITY);
    for r in rects {
        x0 = x0.min(r.x);
        y0 = y0.min(r.y);
        x1 = x1.max(r.x + r.w);
        y1 = y1.max(r.y + r.h);
    }
    Rect::new(x0, y0, x1 - x0, y1 - y0)
}

/// The half-gap threshold and shortest signed dodge match Rect.getDodgeTarget.
/// The target gap is deliberately larger than the stopping clearance; the
/// whole arrangement, including its gaps, is scaled only after separation.
fn dodge_target(a: Rect, b: Rect, aspect_force: f64, gap: f64) -> Option<(f64, f64)> {
    let left = a.x + a.w - b.x + gap;
    let right = b.x + b.w - a.x + gap;
    let up = a.y + a.h - b.y + gap;
    let down = b.y + b.h - a.y + gap;
    if left <= gap / 2.0 || right <= gap / 2.0 || up <= gap / 2.0 || down <= gap / 2.0 {
        return None;
    }
    let horizontal = if left < right { -left } else { right };
    let vertical = if up < down { -up } else { down };
    if (horizontal / aspect_force).abs() < vertical.abs() {
        Some((horizontal, 0.0))
    } else {
        Some((0.0, vertical))
    }
}

/// Damped pairwise separation. Returns false if it hit the pass limit.
fn arrange(rects: &mut [Rect], aspect: f64, gap: f64) -> bool {
    for _ in 0..500 {
        let b = bounds(rects);
        let aspect_force = (aspect / (b.w / b.h)).powi(2);
        let mut any_overlap = false;
        for i in 0..rects.len() {
            // Deliberately visit both (i,j) and (j,i), as in the JS.
            for j in 0..rects.len() {
                if i == j {
                    continue;
                }
                let Some((dx, dy)) = dodge_target(rects[i], rects[j], aspect_force, gap) else {
                    continue;
                };
                any_overlap = true;
                rects[i].x += dx * 0.2;
                rects[j].x -= dx * 0.2;
                rects[i].y += dy * 0.2;
                rects[j].y -= dy * 0.2;
            }
        }
        if !any_overlap {
            return true;
        }
    }
    false
}

/// Indices ordered by center along one axis (stable insertion sort).
#[allow(clippy::neg_cmp_op_on_partial_ord)]
fn center_order(rects: &[Rect], horizontal: bool) -> Vec<usize> {
    let mut order: Vec<usize> = Vec::with_capacity(rects.len());
    for (i, r) in rects.iter().enumerate() {
        let center = r.center(horizontal);
        // Shift past every previous center that is not <= this one, so NaN
        // centers sort exactly as in the original insertion sort.
        let mut k = order.len();
        while k > 0 && !(rects[order[k - 1]].center(horizontal) <= center) {
            k -= 1;
        }
        order.insert(k, i);
    }
    order
}

/// The damped solver can cycle until its iteration limit. Resolve any remaining
/// collisions with a monotone sweep before fitting, rather than displaying them.
/// Earlier rectangles never move, so later placements cannot undo separation.
fn finish_separation(rects: &mut [Rect], aspect: f64, gap: f64) {
    let n = rects.len();
    let collision =
        (0..n).any(|i| (0..i).any(|j| dodge_target(rects[i], rects[j], 1.0, gap).is_some()));
    if !collision {
        return;
    }
    let b = bounds(rects);
    let horizontal = b.w / b.h < aspect;
    let order = center_order(rects, horizontal);
    // Match the natural solver's stopping clearance, with a numerical cushion
    // for zero-gap layouts and the subsequent shared scaling operation.
    let clearance = gap / 2.0 + 1e-9 * b.w.max(b.h);
    for i in 0..n {
        let mut r = rects[order[i]];
        for &j in &order[..i] {
            let prev = rects[j];
            if horizontal {
                if r.y < prev.y + prev.h + clearance && prev.y < r.y + r.h + clearance {
                    r.x = r.x.max(prev.x + prev.w + clearance);
                }
            } else if r.x < prev.x + prev.w + clearance && prev.x < r.x + r.w + clearance {
                r.y = r.y.max(prev.y + prev.h + clearance);
            }
        }
        rects[order[i]] = r;
    }
}

/// Keep the natural layout's pairwise ordering. Each pair contributes one
/// horizontal or vertical separation constraint; center order makes each axis
/// acyclic, so a forward sweep satisfies all its constraints without relaxation.
fn horizontal_pair(a: Rect, b: Rect) -> bool {
    let x_clear = a.x + a.w <= b.x || b.x + b.w <= a.x;
    let y_clear = a.y + a.h <= b.y || b.y + b.h <= a.y;
    if !x_clear {
        return false;
    }
    if !y_clear {
        return true;
    }
    (a.center_x() - b.center_x()).abs() / (a.w + b.w)
        >= (a.center_y() - b.center_y()).abs() / (a.h + b.h)
}

struct Seed {
    rects: Vec<Rect>,
    x_order: Vec<usize>,
    y_order: Vec<usize>,
}

impl Seed {
    /// Native-sized rects in their natural, separated arrangement.
    fn natural(items: &[Item], aspect: f64) -> Self {
        let n = items.len();
        let mut rects: Vec<Rect> = items
            .iter()
            .map(|it| Rect::new(0.0, 0.0, item_w(it), item_h(it)))
            .collect();
        let total: f64 = rects.iter().map(|r| r.w * r.h).sum();
        // Unlike GNOME, we have no original desktop coordinates. Start with one
        // compact, deterministic sunflower of native-sized windows instead.
        for (i, r) in rects.iter_mut().enumerate() {
            let angle = i as f64 * 2.399963229728653;
            let radius = if n == 1 {
                0.0
            } else {
                0.35 * (total * (i as f64 + 0.5) / n as f64).sqrt()
            };
            r.x = radius * aspect.sqrt() * angle.cos() - r.w / 2.0;
            r.y = radius / aspect.sqrt() * angle.sin() - r.h / 2.0;
        }
        // The native-space target shapes the natural arrangement, not the minimum
        // screen-space gap. That is reserved separately during fitting.
        if !arrange(&mut rects, aspect, 50.0) {
            eprintln!(
                "wl-expose: natural layout reached 500 separation passes for {n} windows; \
                 finishing separation before fitting"
            );
            finish_separation(&mut rects, aspect, 50.0);
        }
        let x_order = center_order(&rects, true);
        let y_order = center_order(&rects, false);
        Self {
            rects,
            x_order,
            y_order,
        }
    }

    /// Scale the seed, then push rects apart to keep `gap` between every
    /// constrained pair. Writes into `out` and returns its bounds.
    fn place(&self, scale: f64, gap: f64, out: &mut [Rect]) -> Rect {
        let seed = &self.rects;
        let b = bounds(seed);
        for (o, s) in out.iter_mut().zip(seed) {
            *o = Rect::new(
                (s.x - b.x) * scale,
                (s.y - b.y) * scale,
                s.w * scale,
                s.h * scale,
            );
        }
        for i in 0..seed.len() {
            let (x, y) = (self.x_order[i], self.y_order[i]);
            for j in 0..i {
                let (px, py) = (self.x_order[j], self.y_order[j]);
                if horizontal_pair(seed[x], seed[px]) {
                    out[x].x = out[x].x.max(out[px].x + out[px].w + gap);
                }
                if !horizontal_pair(seed[y], seed[py]) {
                    out[y].y = out[y].y.max(out[py].y + out[py].h + gap);
                }
            }
        }
        bounds(out)
    }

    /// Whether zero-sized thumbnails fit `area` with `gap` between them.
    fn gap_fits(&self, gap: f64, area: Rect) -> bool {
        let min = self.place(0.0, gap, &mut vec![Rect::default(); self.rects.len()]);
        min.w < area.w && min.h < area.h
    }
}

/// Gently dilate centers uniformly, preserving the natural composition and
/// thumbnail sizes. Use only part of the available headroom, not edge-to-edge
/// stretching. Dilation can only increase existing separating distances.
fn spread(rects: &mut [Rect], area: Rect) {
    let b = bounds(rects);
    let (cx, cy) = (b.center_x(), b.center_y());
    let (mut sx, mut sy) = (f64::INFINITY, f64::INFINITY);
    for r in rects.iter() {
        let dx = (r.center_x() - cx).abs();
        let dy = (r.center_y() - cy).abs();
        if dx > 1e-9 {
            sx = sx.min((area.w - r.w) / (2.0 * dx));
        }
        if dy > 1e-9 {
            sy = sy.min((area.h - r.h) / (2.0 * dy));
        }
    }
    let available = sx.min(sy);
    let spread = if available.is_finite() {
        1.0 + 0.75 * (available.clamp(1.0, 1.35) - 1.0)
    } else {
        1.0
    };
    for r in rects.iter_mut() {
        let dx = r.center_x() - cx;
        let dy = r.center_y() - cy;
        r.x = area.center_x() + dx * spread - r.w / 2.0;
        r.y = area.center_y() + dy * spread - r.h / 2.0;
    }
}

/// Natural placement adapted from native_win.js with screen-space gap
/// constraints. Preserves native size ratios and synthetic seed ordering, with
/// a shared scale and gentle uniform center spreading instead of edge-to-edge
/// stretching. If the minimum gap cannot fit the ordering, reduces it with a
/// stderr warning. At the iteration limit, logs to stderr and sweeps away
/// remaining collisions. `out[i]` corresponds to `items[i]`.
pub fn compute(items: &[Item], area: Rect, params: &Params) -> Vec<Rect> {
    let n = items.len();
    if n == 0 {
        return Vec::new();
    }
    if area.w <= 0.0 || area.h <= 0.0 {
        return vec![area.collapsed(); n];
    }

    let seed = Seed::natural(items, area.w / area.h);
    let mut trial = vec![Rect::default(); n];

    let mut gap = params.spacing.max(0.0);
    if !seed.gap_fits(gap, area) {
        let minimum = seed.place(0.0, gap, &mut trial);
        // Even zero-sized thumbnails cannot fit this gap with the natural ordering.
        // Keep the result usable, but never silently weaken the requested minimum.
        let fit = (if minimum.w > 0.0 {
            area.w / minimum.w
        } else {
            f64::INFINITY
        })
        .min(if minimum.h > 0.0 {
            area.h / minimum.h
        } else {
            f64::INFINITY
        });
        let requested = gap;
        gap *= fit * 0.95;
        eprintln!(
            "wl-expose: minimum gap {requested:.2} cannot fit the natural ordering; \
             reducing to {gap:.2}"
        );
    }

    let minimum = seed.place(0.0, gap, &mut trial);
    // Reserve a little of the size budget for breathing room. Fixed gap costs
    // are excluded, so this remains feasible even in crowded layouts.
    let fill = if n == 1 { 1.0 } else { 0.9 };
    let fit_w = minimum.w + fill * (area.w - minimum.w);
    let fit_h = minimum.h + fill * (area.h - minimum.h);

    let b = bounds(&seed.rects);
    let mut hi = (area.w / b.w).min(area.h / b.h);
    if params.max_scale > 0.0 {
        hi = hi.min(params.max_scale);
    }
    let mut lo = 0.0;
    // Constraint lengths grow monotonically with scale. Fit within the size
    // budget while keeping the minimum gap in final logical pixels.
    for _ in 0..48 {
        let scale = (lo + hi) / 2.0;
        let b = seed.place(scale, gap, &mut trial);
        if b.w <= fit_w && b.h <= fit_h {
            lo = scale;
        } else {
            hi = scale;
        }
    }
    let mut out = trial;
    seed.place(lo, gap, &mut out);
    spread(&mut out, area);
    out
}

/// Index of the nearest rect from `rects[cur]` in direction `dir`.
pub fn neighbor(rects: &[Rect], cur: usize, dir: Dir) -> Option<usize> {
    let from = rects.get(cur)?;
    let (cx, cy) = (from.center_x(), from.center_y());
    let mut best = None;
    let mut best_score = f64::INFINITY;
    for (i, r) in rects.iter().enumerate() {
        if i == cur {
            continue;
        }
        let dx = r.center_x() - cx;
        let dy = r.center_y() - cy;
        let (along, across) = match dir {
            Dir::Left => (-dx, dy),
            Dir::Right => (dx, dy),
            Dir::Up => (-dy, dx),
            Dir::Down => (dy, dx),
        };
        if along < 0.5 {
            continue;
        }
        // Prefer staying in the same row/column over raw distance.
        let score = along + 2.0 * across.abs();
        if score < best_score {
            best_score = score;
            best = Some(i);
        }
    }
    best
}

#[cfg(test)]
mod tests;
