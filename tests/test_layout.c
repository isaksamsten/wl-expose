#include <math.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>

#include "layout.h"

#define EPS 1e-6
#define MAX_ITEMS 40

static int failures;

#define CHECK(cond, ...) \
	do { \
		if (!(cond)) { \
			fprintf(stderr, "%s:%d: check failed: %s: ", __FILE__, __LINE__, #cond); \
			fprintf(stderr, __VA_ARGS__); \
			fputc('\n', stderr); \
			failures++; \
		} \
	} while (0)

static bool overlaps(struct layout_rect a, struct layout_rect b, double gap) {
	return a.x < b.x + b.w + gap - EPS && b.x < a.x + a.w + gap - EPS &&
		a.y < b.y + b.h + gap - EPS && b.y < a.y + a.h + gap - EPS;
}

// gap: minimum distance between thumbnails.
static void check_invariants(const char *name, const struct layout_item *items,
		size_t n, struct layout_rect area, double max_scale, double gap,
		const struct layout_rect *out) {
	for (size_t i = 0; i < n; i++) {
		struct layout_rect r = out[i];
		CHECK(r.w > 0 && r.h > 0, "%s: item %zu has empty rect", name, i);
		CHECK(r.x >= area.x - EPS && r.y >= area.y - EPS &&
			r.x + r.w <= area.x + area.w + EPS && r.y + r.h <= area.y + area.h + EPS,
			"%s: item %zu outside area (%.2f,%.2f %.2fx%.2f)", name, i, r.x, r.y, r.w, r.h);
		double want = items[i].w / items[i].h;
		CHECK(fabs(r.w / r.h - want) < 1e-6 * want,
			"%s: item %zu aspect %.6f, want %.6f", name, i, r.w / r.h, want);
		if (max_scale > 0)
			CHECK(r.w <= items[i].w * max_scale + EPS,
				"%s: item %zu upscaled (%.2f > %.2f)", name, i, r.w, items[i].w);
		for (size_t j = i + 1; j < n; j++) {
			CHECK(!overlaps(r, out[j], gap), "%s: items %zu and %zu too close", name, i, j);
		}
	}
}

static double rnd(double lo, double hi) {
	return lo + (hi - lo) * (rand() / (double)RAND_MAX);
}

static double thumbnail_area(const struct layout_rect *rects, size_t n) {
	double area = 0;
	for (size_t i = 0; i < n; i++)
		area += rects[i].w * rects[i].h;
	return area;
}

static void test_random(void) {
	srand(42);
	for (int iter = 0; iter < 2000; iter++) {
		size_t n = 1 + (size_t)(rand() % MAX_ITEMS);
		struct layout_item items[MAX_ITEMS];
		for (size_t i = 0; i < n; i++) {
			items[i] = (struct layout_item){rnd(50, 3000), rnd(50, 2000)};
		}
		struct layout_rect area = {rnd(0, 100), rnd(0, 100), rnd(400, 3840), rnd(300, 2160)};
		struct layout_params p = {
			.spacing = rnd(0, 40),
			.max_scale = rand() % 4 == 0 ? 0 : rnd(0.2, 2),
		};
		struct layout_rect out[MAX_ITEMS], rows[MAX_ITEMS], again[MAX_ITEMS];
		char name[32];
		snprintf(name, sizeof(name), "random#%d", iter);
		// Random sizes include cramped cases where spacing has to shrink,
		// so only check that nothing overlaps.
		layout_compute(items, n, area, &p, out);
		check_invariants(name, items, n, area, p.max_scale, 0, out);

		layout_rows(items, n, area, p.spacing, p.max_scale, rows);
		check_invariants(name, items, n, area, p.max_scale, 0, rows);
		CHECK(thumbnail_area(out, n) + EPS >= thumbnail_area(rows, n),
			"%s: free-form layout shrank the row fallback", name);
		// A feasible requested gap must never be lost by free-form packing.
		bool full_gap = true;
		for (size_t i = 0; i < n; i++)
			for (size_t j = i + 1; j < n; j++)
				if (overlaps(rows[i], rows[j], p.spacing))
					full_gap = false;
		if (full_gap)
			check_invariants(name, items, n, area, p.max_scale, p.spacing, out);
		if (iter < 50) {
			layout_compute(items, n, area, &p, again);
			for (size_t i = 0; i < n; i++)
				CHECK(out[i].x == again[i].x && out[i].y == again[i].y &&
					out[i].w == again[i].w && out[i].h == again[i].h,
					"%s: layout is not deterministic", name);
		}
		if (failures > 20)
			return;
	}
}

static void test_realistic(void) {
	// Typical desktop: thumbnail gaps are kept exactly.
	struct layout_item items[9] = {
		{1920, 1080}, {1920, 1080}, {960, 1080},
		{1280, 800},
		{800, 600}, {800, 600}, {1600, 1000},
		{3840, 2160},
		{500, 900},
	};
	struct layout_rect area = {48, 48, 1824, 984}, out[9];
	struct layout_params p = {.spacing = 24, .max_scale = 1};
	for (size_t n = 1; n <= 9; n++) {
		char name[32];
		snprintf(name, sizeof(name), "realistic/%zu", n);
		layout_compute(items, n, area, &p, out);
		check_invariants(name, items, n, area, 1, p.spacing, out);
	}
}

static void test_single_small(void) {
	// A small window is centered at its native size, never upscaled.
	struct layout_item it = {400, 300};
	struct layout_rect area = {0, 0, 1920, 1080}, out;
	struct layout_params p = {.spacing = 20, .max_scale = 1};
	layout_compute(&it, 1, area, &p, &out);
	CHECK(fabs(out.w - 400) < EPS && fabs(out.h - 300) < EPS,
		"size %.2fx%.2f", out.w, out.h);
	CHECK(fabs(out.x - 760) < EPS && fabs(out.y - 390) < EPS,
		"pos %.2f,%.2f", out.x, out.y);
}

static void test_single_large(void) {
	// A large window fills the area along its limiting axis.
	struct layout_item it = {3840, 2160};
	struct layout_rect area = {0, 0, 1920, 1200}, out;
	struct layout_params p = {.spacing = 20, .max_scale = 1};
	layout_compute(&it, 1, area, &p, &out);
	CHECK(fabs(out.w - 1920) < EPS, "width %.2f", out.w);
	CHECK(fabs(out.y - (1200 - 1080) / 2.0) < EPS, "y %.2f", out.y);
}

static void test_rows(void) {
	// Four equal windows in a 16:9 area: a 2x2 grid beats one row.
	struct layout_item items[4];
	for (int i = 0; i < 4; i++)
		items[i] = (struct layout_item){1920, 1080};
	struct layout_rect area = {0, 0, 1920, 1080}, out[4];
	layout_rows(items, 4, area, 0, 1, out);
	CHECK(fabs(out[0].w - 960) < EPS, "width %.2f", out[0].w);
	CHECK(fabs(out[1].y - out[0].y) < EPS && out[2].y > out[0].y, "not 2x2");
	// Order is kept: row-major.
	CHECK(out[1].x > out[0].x && out[3].x > out[2].x, "order not kept");
}

static void test_relative_sizes(void) {
	// Same aspect ratio, different native sizes: no equal-height normalization.
	struct layout_item items[] = {{400, 300}, {1600, 1200}};
	struct layout_rect area = {20, 30, 1000, 600}, out[2];
	layout_rows(items, 2, area, 24, 1, out);
	check_invariants("relative sizes", items, 2, area, 1, 24, out);
	// Boosts are 1.25 and 1.0, so the height ratio is 0.3125.
	CHECK(fabs(out[0].h / out[1].h - 0.3125) < EPS,
		"relative height %.6f", out[0].h / out[1].h);
	CHECK(out[0].h > out[1].h * 0.25,
		"small window did not receive a boost");
	CHECK(fabs(out[0].y + out[0].h / 2 - out[1].y - out[1].h / 2) < EPS,
		"single row is not vertically centered");
}

static void test_mixed_rows(void) {
	struct layout_item items[] = {{1600, 1200}, {400, 300},
		{1600, 1200}, {400, 300}};
	struct layout_rect area = {0, 0, 1000, 1000}, out[4];
	layout_rows(items, 4, area, 20, 1, out);
	check_invariants("mixed rows", items, 4, area, 1, 20, out);
	CHECK(out[2].y > out[0].y, "expected two rows");
	CHECK(fabs(out[0].y + out[0].h - out[1].y - out[1].h) < EPS &&
		fabs(out[2].y + out[2].h - out[3].y - out[3].h) < EPS,
		"windows do not share row baselines");
}

static void test_scale_caps(void) {
	struct layout_item items[] = {{100, 100}, {800, 600}};
	struct layout_rect area = {0, 0, 2000, 1500}, out[2];
	layout_rows(items, 2, area, 24, 0.75, out);
	check_invariants("caps", items, 2, area, 0.75, 24, out);
	CHECK(fabs(out[0].w - 75) < EPS && fabs(out[1].w - 600) < EPS,
		"boost bypassed cap or capped windows shrank unnecessarily");
}

static void test_freeform(void) {
	// A full-width row is a local trap: independent placement should find
	// room for the utilities below the two large windows, not beside them.
	struct layout_item items[] = {{1600, 1200}, {400, 300},
		{1600, 1200}, {400, 300}};
	struct layout_rect area = {48, 48, 1824, 984}, out[4], rows[4], shifted[4];
	struct layout_params p = {.spacing = 24, .max_scale = 1};
	layout_rows(items, 4, area, p.spacing, p.max_scale, rows);
	layout_compute(items, 4, area, &p, out);
	check_invariants("freeform", items, 4, area, 1, p.spacing, out);
	CHECK(thumbnail_area(out, 4) > thumbnail_area(rows, 4) * 1.4,
		"free-form packing did not escape the row constraint");
	CHECK(fabs(out[0].y + out[0].h - out[1].y - out[1].h) > 1,
		"previews still share the old row baseline");

	// Translation must not alter the sizes or relative composition.
	area.x += 123;
	area.y -= 57;
	layout_compute(items, 4, area, &p, shifted);
	for (size_t i = 0; i < 4; i++)
		CHECK(fabs(shifted[i].x - out[i].x - 123) < EPS &&
			fabs(shifted[i].y - out[i].y + 57) < EPS &&
			fabs(shifted[i].w - out[i].w) < EPS &&
			fabs(shifted[i].h - out[i].h) < EPS,
			"translation changed item %zu", i);
}

static void test_freeform_limits(void) {
	struct layout_item items[] = {{400, 300}, {600, 500}, {350, 700},
		{500, 400}, {800, 600}};
	struct layout_rect area = {48, 48, 1824, 984}, out[5];
	struct layout_params p = {.spacing = 24, .max_scale = 1};
	layout_compute(items, 5, area, &p, out);
	check_invariants("freeform capped", items, 5, area, 1, 24, out);
	for (size_t i = 0; i < 5; i++)
		CHECK(fabs(out[i].w - items[i].w) < EPS,
			"utility %zu did not grow to its cap", i);

	p.spacing = -10;
	p.max_scale = 0;
	layout_compute(items, 5, area, &p, out);
	check_invariants("freeform uncapped", items, 5, area, 0, 0, out);

	// An impossible requested gap uses the valid reduced-gap row fallback.
	p.spacing = 500;
	area = (struct layout_rect){5, 10, 30, 20};
	layout_compute(items, 5, area, &p, out);
	check_invariants("freeform cramped", items, 5, area, 0, 0, out);
	area.w = 0;
	layout_compute(items, 5, area, &p, out);
	for (size_t i = 0; i < 5; i++)
		CHECK(out[i].w == 0 && out[i].h == 0 &&
			out[i].x == 5 && out[i].y == 20, "empty area did not collapse");
}

static void test_neighbor(void) {
	// 3 on top, 2 below (centered)
	struct layout_rect r[5] = {
		{0, 0, 100, 100}, {120, 0, 100, 100}, {240, 0, 100, 100},
		{60, 120, 100, 100}, {180, 120, 100, 100},
	};
	CHECK(layout_neighbor(r, 5, 0, LAYOUT_RIGHT) == 1, "0 right");
	CHECK(layout_neighbor(r, 5, 1, LAYOUT_LEFT) == 0, "1 left");
	CHECK(layout_neighbor(r, 5, 0, LAYOUT_LEFT) == -1, "0 left");
	CHECK(layout_neighbor(r, 5, 0, LAYOUT_UP) == -1, "0 up");
	CHECK(layout_neighbor(r, 5, 0, LAYOUT_DOWN) == 3, "0 down");
	CHECK(layout_neighbor(r, 5, 2, LAYOUT_DOWN) == 4, "2 down");
	CHECK(layout_neighbor(r, 5, 4, LAYOUT_UP) == 1 || layout_neighbor(r, 5, 4, LAYOUT_UP) == 2,
		"4 up");
	CHECK(layout_neighbor(r, 5, 3, LAYOUT_RIGHT) == 4, "3 right");
	CHECK(layout_neighbor(r, 5, 4, LAYOUT_RIGHT) == -1 ||
		layout_neighbor(r, 5, 4, LAYOUT_RIGHT) == 2, "4 right");
}

static void test_degenerate(void) {
	// Zero sizes are treated as 1x1, a tiny area doesn't crash.
	struct layout_item items[2] = {{0, 0}, {-5, 10}};
	struct layout_rect area = {0, 0, 10, 10}, out[2];
	struct layout_params p = {.spacing = 50, .max_scale = 1};
	layout_compute(items, 2, area, &p, out);
	layout_compute(items, 0, area, &p, out);
}

int main(void) {
	test_single_small();
	test_single_large();
	test_rows();
	test_relative_sizes();
	test_mixed_rows();
	test_scale_caps();
	test_freeform();
	test_freeform_limits();
	test_neighbor();
	test_degenerate();
	test_realistic();
	test_random();
	if (failures) {
		fprintf(stderr, "%d failure(s)\n", failures);
		return 1;
	}
	printf("ok\n");
	return 0;
}

