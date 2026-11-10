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
		struct layout_rect out[MAX_ITEMS];
		char name[32];
		snprintf(name, sizeof(name), "random#%d", iter);
		// Random sizes include cramped cases where spacing has to shrink,
		// so only check that nothing overlaps.
		layout_compute(items, n, area, &p, out);
		check_invariants(name, items, n, area, p.max_scale, 0, out);

		layout_rows(items, n, area, p.spacing, p.max_scale, out);
		check_invariants(name, items, n, area, p.max_scale, 0, out);
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

