//! Desktop geometry: which monitors touch, and how pixels along a shared edge map to millimetres.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub fn right(&self) -> i32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> i32 {
        self.y + self.h
    }
    pub fn intersects(&self, o: &Rect) -> bool {
        self.x < o.right() && o.x < self.right() && self.y < o.bottom() && o.y < self.bottom()
    }
}

/// A monitor as far as geometry is concerned: its desktop rectangle and visible panel size.
#[derive(Clone, Copy, Debug)]
pub struct Mon {
    pub rect: Rect,
    pub mm_w: f32,
    pub mm_h: f32,
}

impl Mon {
    pub fn ppmm_x(&self) -> f64 {
        self.rect.w as f64 / self.mm_w as f64
    }
    pub fn ppmm_y(&self) -> f64 {
        self.rect.h as f64 / self.mm_h as f64
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    /// The shared edge is a vertical line: `a` is on the left, `b` on the right, positions run along y.
    Vertical,
    /// The shared edge is a horizontal line: `a` is on top, `b` below, positions run along x.
    Horizontal,
}

#[derive(Clone, Copy, Debug)]
pub struct Edge {
    pub axis: Axis,
    pub a: usize,
    pub b: usize,
    /// Desktop coordinate of the line itself (x for vertical edges, y for horizontal ones).
    pub pos: i32,
    /// Shared part of the line, in desktop pixels along the edge.
    pub s0: i32,
    pub s1: i32,
}

impl Edge {
    pub fn other(&self, i: usize) -> usize {
        if i == self.a { self.b } else { self.a }
    }
    /// Desktop coordinate where the monitor's side begins along this edge.
    pub fn start(&self, m: &Mon) -> f64 {
        match self.axis {
            Axis::Vertical => m.rect.y as f64,
            Axis::Horizontal => m.rect.x as f64,
        }
    }
    pub fn ppmm(&self, m: &Mon) -> f64 {
        match self.axis {
            Axis::Vertical => m.ppmm_y(),
            Axis::Horizontal => m.ppmm_x(),
        }
    }
    pub fn len_mm(&self, m: &Mon) -> f64 {
        match self.axis {
            Axis::Vertical => m.mm_h as f64,
            Axis::Horizontal => m.mm_w as f64,
        }
    }
    pub fn along(&self, p: (i32, i32)) -> i32 {
        match self.axis {
            Axis::Vertical => p.1,
            Axis::Horizontal => p.0,
        }
    }
}

pub fn find_edges(mons: &[Mon]) -> Vec<Edge> {
    let mut edges = Vec::new();
    for (i, a) in mons.iter().enumerate() {
        for (j, b) in mons.iter().enumerate() {
            if i == j {
                continue;
            }
            let (a, b) = (a.rect, b.rect);
            if a.right() == b.x {
                let (s0, s1) = (a.y.max(b.y), a.bottom().min(b.bottom()));
                if s1 > s0 {
                    edges.push(Edge { axis: Axis::Vertical, a: i, b: j, pos: b.x, s0, s1 });
                }
            }
            if a.bottom() == b.y {
                let (s0, s1) = (a.x.max(b.x), a.right().min(b.right()));
                if s1 > s0 {
                    edges.push(Edge { axis: Axis::Horizontal, a: i, b: j, pos: b.y, s0, s1 });
                }
            }
        }
    }
    edges
}

/// Where the zero of the rulers sits on a shared edge. At that point the cursor crosses
/// the edge without a physical jump; elsewhere the error grows if pixel densities differ.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Anchor {
    Start,
    #[default]
    Center,
    End,
    Cursor,
}

pub fn anchor_t(e: &Edge, anchor: Anchor, cursor: Option<(i32, i32)>) -> f64 {
    let (s0, s1) = (e.s0 as f64, e.s1 as f64);
    match anchor {
        Anchor::Start => s0,
        Anchor::End => s1,
        Anchor::Center => (s0 + s1) / 2.0,
        Anchor::Cursor => match cursor {
            Some(c) => (e.along(c) as f64).clamp(s0, s1),
            None => (s0 + s1) / 2.0,
        },
    }
}

/// How many millimetres `b`'s side starts further along the edge than `a`'s side
/// (positive: `b` is lower / further right), assuming the cursor crossing at `t0` is seamless.
pub fn implied_offset_mm(e: &Edge, mons: &[Mon], t0: f64) -> f64 {
    let (a, b) = (&mons[e.a], &mons[e.b]);
    (t0 - e.start(a)) / e.ppmm(a) - (t0 - e.start(b)) / e.ppmm(b)
}

/// Physical error of the crossing at desktop coordinate `t`, relative to a seamless crossing at `t0`.
pub fn mismatch_mm(e: &Edge, mons: &[Mon], t0: f64, t: f64) -> f64 {
    let (a, b) = (&mons[e.a], &mons[e.b]);
    (t - t0) * (1.0 / e.ppmm(a) - 1.0 / e.ppmm(b))
}

/// Desktop start coordinate for the moving monitor so that, with the panels physically offset
/// by `d` mm (as in [`implied_offset_mm`]), the cursor crosses seamlessly at the chosen point.
pub fn target_start(e: &Edge, mons: &[Mon], d: f64, anchor: Anchor, move_b: bool) -> i32 {
    let (a, b) = (&mons[e.a], &mons[e.b]);
    let (pa, pb) = (e.ppmm(a), e.ppmm(b));
    // Physical overlap of the two sides, measured from the start of `a`.
    let lo = d.max(0.0);
    let hi = e.len_mm(a).min(d + e.len_mm(b)).max(lo);
    let h = match anchor {
        Anchor::Start => lo,
        Anchor::End => hi,
        Anchor::Center | Anchor::Cursor => (lo + hi) / 2.0,
    };
    let v = if move_b {
        e.start(a) + h * pa - (h - d) * pb
    } else {
        e.start(b) + (h - d) * pb - h * pa
    };
    v.round() as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mon(x: i32, y: i32, w: i32, h: i32, mm_w: f32, mm_h: f32) -> Mon {
        Mon { rect: Rect { x, y, w, h }, mm_w, mm_h }
    }

    #[test]
    fn finds_side_by_side_edge() {
        let mons = [mon(0, 0, 1920, 1080, 531.0, 299.0), mon(1920, 200, 1920, 1080, 531.0, 299.0)];
        let edges = find_edges(&mons);
        assert_eq!(edges.len(), 1);
        let e = edges[0];
        assert_eq!((e.axis, e.a, e.b, e.pos, e.s0, e.s1), (Axis::Vertical, 0, 1, 1920, 200, 1080));
    }

    #[test]
    fn same_density_offset_is_pixels_over_density() {
        let mons = [mon(0, 0, 1920, 1080, 531.0, 299.0), mon(1920, 100, 1920, 1080, 531.0, 299.0)];
        let e = find_edges(&mons)[0];
        let d = implied_offset_mm(&e, &mons, 500.0);
        assert!((d - 100.0 / (1080.0 / 299.0)).abs() < 1e-9);
        assert_eq!(mismatch_mm(&e, &mons, 500.0, 1000.0), 0.0);
    }

    #[test]
    fn target_start_round_trips() {
        // 27" 1440p next to 24" 1080p, physically top-aligned.
        let mut mons = [mon(0, 0, 2560, 1440, 597.0, 336.0), mon(2560, 0, 1920, 1080, 531.0, 299.0)];
        let e = find_edges(&mons)[0];
        let y = target_start(&e, &mons, 0.0, Anchor::Start, true);
        assert_eq!(y, 0);
        let y = target_start(&e, &mons, 18.5, Anchor::Center, true);
        mons[1].rect.y = y;
        let e = find_edges(&mons)[0];
        // Physical overlap is 18.5..317.5 mm of the first panel, its middle is at 168 mm.
        let t0 = 168.0 * mons[0].ppmm_y();
        assert!((implied_offset_mm(&e, &mons, t0) - 18.5).abs() < 0.3);
    }
}
