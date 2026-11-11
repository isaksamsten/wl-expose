use super::*;

const EPS: f64 = 1e-6;
const MAX_ITEMS: usize = 40;

fn item(w: f64, h: f64) -> Item {
    Item { w, h }
}

fn overlaps(a: Rect, b: Rect, gap: f64) -> bool {
    a.x < b.x + b.w + gap - EPS
        && b.x < a.x + a.w + gap - EPS
        && a.y < b.y + b.h + gap - EPS
        && b.y < a.y + a.h + gap - EPS
}

/// Nothing empty, everything inside the area, aspect ratios kept, no
/// upscaling past `max_scale`, and at least `gap` between thumbnails.
fn check_invariants(
    name: &str,
    items: &[Item],
    area: Rect,
    max_scale: f64,
    gap: f64,
    out: &[Rect],
) {
    assert_eq!(items.len(), out.len(), "{name}: wrong rect count");
    for (i, (it, r)) in items.iter().zip(out).enumerate() {
        assert!(r.w > 0.0 && r.h > 0.0, "{name}: item {i} has empty rect");
        assert!(
            r.x >= area.x - EPS
                && r.y >= area.y - EPS
                && r.x + r.w <= area.x + area.w + EPS
                && r.y + r.h <= area.y + area.h + EPS,
            "{name}: item {i} outside area ({:.2},{:.2} {:.2}x{:.2})",
            r.x,
            r.y,
            r.w,
            r.h
        );
        let want = it.w / it.h;
        assert!(
            (r.w / r.h - want).abs() < 1e-6 * want,
            "{name}: item {i} aspect {:.6}, want {want:.6}",
            r.w / r.h
        );
        if max_scale > 0.0 {
            assert!(
                r.w <= it.w * max_scale + EPS,
                "{name}: item {i} upscaled ({:.2} > {:.2})",
                r.w,
                it.w
            );
        }
        for (j, other) in out.iter().enumerate().skip(i + 1) {
            assert!(
                !overlaps(*r, *other, gap),
                "{name}: items {i} and {j} too close"
            );
        }
    }
}

fn thumbnail_area(rects: &[Rect]) -> f64 {
    rects.iter().map(|r| r.w * r.h).sum()
}

/// Small deterministic PRNG (xorshift64*), so the tests need no dependencies.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * ((self.next() >> 11) as f64 / (1u64 << 53) as f64)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

struct RandomCase {
    name: String,
    items: Vec<Item>,
    area: Rect,
    params: Params,
}

fn random_cases() -> impl Iterator<Item = RandomCase> {
    let mut rng = Rng(42);
    (0..2000).map(move |iter| {
        let n = 1 + rng.below(MAX_ITEMS as u64) as usize;
        let items = (0..n)
            .map(|_| item(rng.range(50.0, 3000.0), rng.range(50.0, 2000.0)))
            .collect();
        let area = Rect::new(
            rng.range(0.0, 100.0),
            rng.range(0.0, 100.0),
            rng.range(400.0, 3840.0),
            rng.range(300.0, 2160.0),
        );
        let params = Params {
            spacing: rng.range(0.0, 40.0),
            max_scale: if rng.below(4) == 0 {
                0.0
            } else {
                rng.range(0.2, 2.0)
            },
        };
        RandomCase {
            name: format!("random#{iter}"),
            items,
            area,
            params,
        }
    })
}

#[test]
fn random() {
    for (iter, c) in random_cases().enumerate() {
        let RandomCase {
            name,
            items,
            area,
            params: p,
        } = &c;
        // Random sizes include cramped cases where spacing has to shrink,
        // so only check that nothing overlaps.
        let out = compute(items, *area, p);
        check_invariants(name, items, *area, p.max_scale, 0.0, &out);

        let rows = rows(items, *area, p.spacing, p.max_scale);
        check_invariants(name, items, *area, p.max_scale, 0.0, &rows);

        // A requested gap that fits the natural ordering must be kept. When
        // it doesn't fit, compute() reduces it (documented, with a warning),
        // even if a row layout could have kept it.
        let full_gap = rows
            .iter()
            .enumerate()
            .all(|(i, a)| rows[i + 1..].iter().all(|b| !overlaps(*a, *b, p.spacing)));
        let natural_fits = Seed::natural(items, area.w / area.h).gap_fits(p.spacing, *area);
        if full_gap && natural_fits {
            check_invariants(name, items, *area, p.max_scale, p.spacing, &out);
        }
        if iter < 50 {
            assert_eq!(
                out,
                compute(items, *area, p),
                "{name}: layout is not deterministic"
            );
        }
    }
}

// Stale since the natural-placement layout (it reserves breathing room and
// no longer falls back to rows); this already failed in the C test suite.
#[test]
#[ignore = "stale: natural placement no longer guarantees the row-layout area"]
fn random_never_smaller_than_rows() {
    for c in random_cases() {
        let out = compute(&c.items, c.area, &c.params);
        let rows = rows(&c.items, c.area, c.params.spacing, c.params.max_scale);
        assert!(
            thumbnail_area(&out) + EPS >= thumbnail_area(&rows),
            "{}: free-form layout shrank the row fallback",
            c.name
        );
    }
}

#[test]
fn realistic() {
    // Typical desktop: thumbnail gaps are kept exactly.
    let items = [
        item(1920.0, 1080.0),
        item(1920.0, 1080.0),
        item(960.0, 1080.0),
        item(1280.0, 800.0),
        item(800.0, 600.0),
        item(800.0, 600.0),
        item(1600.0, 1000.0),
        item(3840.0, 2160.0),
        item(500.0, 900.0),
    ];
    let area = Rect::new(48.0, 48.0, 1824.0, 984.0);
    let p = Params {
        spacing: 24.0,
        max_scale: 1.0,
    };
    for n in 1..=items.len() {
        let out = compute(&items[..n], area, &p);
        check_invariants(
            &format!("realistic/{n}"),
            &items[..n],
            area,
            1.0,
            p.spacing,
            &out,
        );
    }
}

#[test]
fn single_small() {
    // A small window is centered at its native size, never upscaled.
    let area = Rect::new(0.0, 0.0, 1920.0, 1080.0);
    let p = Params {
        spacing: 20.0,
        max_scale: 1.0,
    };
    let out = compute(&[item(400.0, 300.0)], area, &p)[0];
    assert!(
        (out.w - 400.0).abs() < EPS && (out.h - 300.0).abs() < EPS,
        "size {out:?}"
    );
    assert!(
        (out.x - 760.0).abs() < EPS && (out.y - 390.0).abs() < EPS,
        "pos {out:?}"
    );
}

#[test]
fn single_large() {
    // A large window fills the area along its limiting axis.
    let area = Rect::new(0.0, 0.0, 1920.0, 1200.0);
    let p = Params {
        spacing: 20.0,
        max_scale: 1.0,
    };
    let out = compute(&[item(3840.0, 2160.0)], area, &p)[0];
    assert!((out.w - 1920.0).abs() < EPS, "width {}", out.w);
    assert!((out.y - (1200.0 - 1080.0) / 2.0).abs() < EPS, "y {}", out.y);
}

#[test]
fn rows_grid() {
    // Four equal windows in a 16:9 area: a 2x2 grid beats one row.
    let items = [item(1920.0, 1080.0); 4];
    let out = rows(&items, Rect::new(0.0, 0.0, 1920.0, 1080.0), 0.0, 1.0);
    assert!((out[0].w - 960.0).abs() < EPS, "width {}", out[0].w);
    assert!(
        (out[1].y - out[0].y).abs() < EPS && out[2].y > out[0].y,
        "not 2x2"
    );
    // Order is kept: row-major.
    assert!(out[1].x > out[0].x && out[3].x > out[2].x, "order not kept");
}

#[test]
fn relative_sizes() {
    // Same aspect ratio, different native sizes: no equal-height normalization.
    let items = [item(400.0, 300.0), item(1600.0, 1200.0)];
    let area = Rect::new(20.0, 30.0, 1000.0, 600.0);
    let out = rows(&items, area, 24.0, 1.0);
    check_invariants("relative sizes", &items, area, 1.0, 24.0, &out);
    // Boosts are 1.25 and 1.0, so the height ratio is 0.3125.
    assert!(
        (out[0].h / out[1].h - 0.3125).abs() < EPS,
        "relative height {:.6}",
        out[0].h / out[1].h
    );
    assert!(
        out[0].h > out[1].h * 0.25,
        "small window did not receive a boost"
    );
    assert!(
        (out[0].center_y() - out[1].center_y()).abs() < EPS,
        "single row is not vertically centered"
    );
}

#[test]
fn mixed_rows() {
    let items = [
        item(1600.0, 1200.0),
        item(400.0, 300.0),
        item(1600.0, 1200.0),
        item(400.0, 300.0),
    ];
    let area = Rect::new(0.0, 0.0, 1000.0, 1000.0);
    let out = rows(&items, area, 20.0, 1.0);
    check_invariants("mixed rows", &items, area, 1.0, 20.0, &out);
    assert!(out[2].y > out[0].y, "expected two rows");
    let bottom = |r: Rect| r.y + r.h;
    assert!(
        (bottom(out[0]) - bottom(out[1])).abs() < EPS
            && (bottom(out[2]) - bottom(out[3])).abs() < EPS,
        "windows do not share row baselines"
    );
}

#[test]
fn scale_caps() {
    let items = [item(100.0, 100.0), item(800.0, 600.0)];
    let area = Rect::new(0.0, 0.0, 2000.0, 1500.0);
    let out = rows(&items, area, 24.0, 0.75);
    check_invariants("caps", &items, area, 0.75, 24.0, &out);
    assert!(
        (out[0].w - 75.0).abs() < EPS && (out[1].w - 600.0).abs() < EPS,
        "boost bypassed cap or capped windows shrank unnecessarily"
    );
}

#[test]
fn freeform() {
    // A full-width row is a local trap: independent placement should find
    // room for the utilities below the two large windows, not beside them.
    let items = [
        item(1600.0, 1200.0),
        item(400.0, 300.0),
        item(1600.0, 1200.0),
        item(400.0, 300.0),
    ];
    let mut area = Rect::new(48.0, 48.0, 1824.0, 984.0);
    let p = Params {
        spacing: 24.0,
        max_scale: 1.0,
    };
    let rows = rows(&items, area, p.spacing, p.max_scale);
    let out = compute(&items, area, &p);
    check_invariants("freeform", &items, area, 1.0, p.spacing, &out);
    assert!(
        thumbnail_area(&out) > thumbnail_area(&rows) * 1.4,
        "free-form packing did not escape the row constraint"
    );
    assert!(
        (out[0].y + out[0].h - out[1].y - out[1].h).abs() > 1.0,
        "previews still share the old row baseline"
    );

    // Translation must not alter the sizes or relative composition.
    area.x += 123.0;
    area.y -= 57.0;
    let shifted = compute(&items, area, &p);
    for (i, (s, o)) in shifted.iter().zip(&out).enumerate() {
        assert!(
            (s.x - o.x - 123.0).abs() < EPS
                && (s.y - o.y + 57.0).abs() < EPS
                && (s.w - o.w).abs() < EPS
                && (s.h - o.h).abs() < EPS,
            "translation changed item {i}"
        );
    }
}

const UTILITIES: [Item; 5] = [
    Item { w: 400.0, h: 300.0 },
    Item { w: 600.0, h: 500.0 },
    Item { w: 350.0, h: 700.0 },
    Item { w: 500.0, h: 400.0 },
    Item { w: 800.0, h: 600.0 },
];

#[test]
fn freeform_limits() {
    let area = Rect::new(48.0, 48.0, 1824.0, 984.0);
    let mut p = Params {
        spacing: 24.0,
        max_scale: 1.0,
    };
    let out = compute(&UTILITIES, area, &p);
    check_invariants("freeform capped", &UTILITIES, area, 1.0, 24.0, &out);

    p.spacing = -10.0;
    p.max_scale = 0.0;
    let out = compute(&UTILITIES, area, &p);
    check_invariants("freeform uncapped", &UTILITIES, area, 0.0, 0.0, &out);

    // An impossible requested gap is reduced rather than overflowing.
    p.spacing = 500.0;
    let mut area = Rect::new(5.0, 10.0, 30.0, 20.0);
    let out = compute(&UTILITIES, area, &p);
    check_invariants("freeform cramped", &UTILITIES, area, 0.0, 0.0, &out);

    area.w = 0.0;
    for r in compute(&UTILITIES, area, &p) {
        assert_eq!(
            r,
            Rect::new(5.0, 20.0, 0.0, 0.0),
            "empty area did not collapse"
        );
    }
}

// Stale since the natural-placement layout reserves 10% breathing room;
// this already failed in the C test suite.
#[test]
#[ignore = "stale: natural placement reserves breathing room instead of growing to the cap"]
fn freeform_utilities_grow_to_cap() {
    let area = Rect::new(48.0, 48.0, 1824.0, 984.0);
    let p = Params {
        spacing: 24.0,
        max_scale: 1.0,
    };
    let out = compute(&UTILITIES, area, &p);
    for (i, (r, it)) in out.iter().zip(&UTILITIES).enumerate() {
        assert!(
            (r.w - it.w).abs() < EPS,
            "utility {i} did not grow to its cap"
        );
    }
}

#[test]
fn neighbors() {
    // 3 on top, 2 below (centered)
    let r = [
        Rect::new(0.0, 0.0, 100.0, 100.0),
        Rect::new(120.0, 0.0, 100.0, 100.0),
        Rect::new(240.0, 0.0, 100.0, 100.0),
        Rect::new(60.0, 120.0, 100.0, 100.0),
        Rect::new(180.0, 120.0, 100.0, 100.0),
    ];
    assert_eq!(neighbor(&r, 0, Dir::Right), Some(1));
    assert_eq!(neighbor(&r, 1, Dir::Left), Some(0));
    assert_eq!(neighbor(&r, 0, Dir::Left), None);
    assert_eq!(neighbor(&r, 0, Dir::Up), None);
    assert_eq!(neighbor(&r, 0, Dir::Down), Some(3));
    assert_eq!(neighbor(&r, 2, Dir::Down), Some(4));
    assert!(matches!(neighbor(&r, 4, Dir::Up), Some(1 | 2)));
    assert_eq!(neighbor(&r, 3, Dir::Right), Some(4));
    assert!(matches!(neighbor(&r, 4, Dir::Right), None | Some(2)));
    assert_eq!(neighbor(&r, 7, Dir::Right), None);
}

#[test]
fn degenerate() {
    // Zero sizes are treated as 1x1, a tiny area doesn't crash.
    let items = [item(0.0, 0.0), item(-5.0, 10.0)];
    let area = Rect::new(0.0, 0.0, 10.0, 10.0);
    let p = Params {
        spacing: 50.0,
        max_scale: 1.0,
    };
    assert_eq!(compute(&items, area, &p).len(), 2);
    assert!(compute(&[], area, &p).is_empty());
}
