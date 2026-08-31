use super::constraints::{ConstraintKind, DimTarget};
use super::document::Document;
use super::geometry::Point2;
use super::ids::PointId;
use std::collections::HashMap;

// Real constraint solver: damped least-squares (Levenberg-Marquardt) over
// FREE point positions.
//
// Dragged points are eliminated from the system entirely — their cursor
// targets ARE their positions, exactly, by construction. The remaining
// free points are solved with well-conditioned weights, so every live
// drag frame is one small dense solve.
//
// Weighting scheme over free points:
//   - constraint equations (H/V/coincident/locked dims) dominate,
//   - a small soft anchor pulls each free point toward its pre-solve
//     position, which kills nullspace drift on under-constrained systems
//     (a rectangle has 8 DOF but only 4 constraint equations) by always
//     picking the minimal-motion solution.

/// Outcome of a solve pass.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SolveStatus {
    Converged,
    MaxIterations,
}

pub struct Solution {
    pub status: SolveStatus,
    pub positions: Vec<(PointId, Point2)>,
    /// Largest unsatisfied LINEAR residual (doc units) at the solution.
    pub max_lin_residual: f64,
    /// Largest unsatisfied ANGULAR residual (radians) at the solution.
    pub max_angle_residual: f64,
}

const ANCHOR_WEIGHT: f64 = 1.0;
const DIM_WEIGHT: f64 = 1e6;
const EQ_WEIGHT: f64 = 1e6;
const DRAG_WEIGHT: f64 = 1e3;
const MAX_ITER: usize = 60;
const TOL: f64 = 1e-10;
const LAMBDA_INIT: f64 = 1e-3;

// One equation over point positions.
#[derive(Clone, Copy, Debug)]
enum Eq {
    // Residual: a.y - b.y
    Horizontal { a: usize, b: usize },
    // Residual: a.x - b.x
    Vertical { a: usize, b: usize },
    // Residual: |ab| - target
    Distance { a: usize, b: usize, target: f64 },
    // Residual: signed distance of p from line (l1->l2) - target
    PointLineDist { p: usize, l1: usize, l2: usize, target: f64 },
    // Residual: signed distance of b1 from line (a1->a2) - target
    LineDist { a1: usize, a2: usize, b1: usize, target: f64 },
    // Residual: signed angle between (a2-a1) and (b2-b1) - target (radians)
    Angle { a1: usize, a2: usize, b1: usize, b2: usize, target: f64 },
    // Residual: circumradius of the triangle (s,e,c) - target (doc units).
    // Expresses an arc radius constraint through its three defining points.
    ArcRadius { s: usize, e: usize, c: usize, target: f64 },
}

// A residual evaluated at one iterate: value plus sparse gradient over
// FREE variable indices.
struct Residual {
    value: f64,
    // (free variable index, d value / d variable)
    grad: Vec<(usize, f64)>,
    weight: f64,
}

pub struct Solver {
    // All points involved in the system, indexed by slot. Slots that are
    // neither dragged nor auxiliary are FIXED at their document positions.
    slots: Vec<PointId>,
    // Slot -> true when eliminated (a drag target).
    fixed: Vec<bool>,
    // Positions for fixed slots.
    fixed_pos: Vec<Point2>,
    start: Vec<Point2>,
    eqs: Vec<Eq>,
    // Dragged slots and cursor targets.
    drag: Vec<(usize, Point2)>,
    // Auxiliary slots: free followers with a strong anchor toward
    // `aux_anchor` (their gesture-start position).
    aux: Vec<(usize, Point2)>,
    // Map slot index -> free variable index (None when fixed).
    free_of: Vec<Option<usize>>,
    n_free: usize,
    // Soft-anchor weight for free points (strong for dimension applies).
    anchor_weight: f64,
}

impl Solver {
    /// Builds the system from the document. `drag` maps dragged points to
    /// cursor targets (they chase the cursor with dominant weight). `aux`
    /// lists follower points with their gesture-start positions (soft,
    /// strong-anchor). Every other point referenced by a constraint is
    /// HARD-FIXED at its current document position, so constraints can
    /// reject illegal components of the drag instead of transmitting them.
    pub fn build(
        doc: &Document,
        drag: &[(PointId, Point2)],
        aux: &[(PointId, Point2)],
    ) -> Solver {
        let mut slots: Vec<PointId> = Vec::new();
        let mut index: HashMap<PointId, usize> = HashMap::new();

        let mut slot_of = |pid: PointId| -> Option<usize> {
            if doc.point(pid).is_none() {
                return None;
            }
            Some(match index.get(&pid) {
                Some(&i) => i,
                None => {
                    let i = slots.len();
                    slots.push(pid);
                    index.insert(pid, i);
                    i
                }
            })
        };

        let mut eqs = Vec::new();
        for c in &doc.constraints {
            let (Some(a), Some(b)) = (slot_of(c.a), slot_of(c.b)) else {
                continue;
            };
            match c.kind {
                ConstraintKind::Horizontal => eqs.push(Eq::Horizontal { a, b }),
                ConstraintKind::Vertical => eqs.push(Eq::Vertical { a, b }),
                ConstraintKind::Coincident => {
                    eqs.push(Eq::Horizontal { a, b });
                    eqs.push(Eq::Vertical { a, b });
                }
            }
        }
        for d in &doc.dimensions {
            match &d.target {
                DimTarget::Points { a, b } => {
                    let (Some(a), Some(b)) = (slot_of(*a), slot_of(*b)) else {
                        continue;
                    };
                    eqs.push(Eq::Distance { a, b, target: d.value });
                }
                DimTarget::PointLine { p, line } => {
                    let Some(seg) = doc.segment(*line) else { continue };
                    let (Some(pp), Some(l1), Some(l2)) =
                        (slot_of(*p), slot_of(seg.start), slot_of(seg.end))
                    else {
                        continue;
                    };
                    // Preserve whichever side of the line the point is on.
                    let (Some(ppp), Some(l1p), Some(l2p)) = (
                        doc.point(*p),
                        doc.point(seg.start),
                        doc.point(seg.end),
                    ) else {
                        continue;
                    };
                    let dx = l2p.x - l1p.x;
                    let dy = l2p.y - l1p.y;
                    let l = (dx * dx + dy * dy).sqrt().max(1e-9);
                    let signed =
                        (ppp.x - l1p.x) * (-dy / l) + (ppp.y - l1p.y) * (dx / l);
                    eqs.push(Eq::PointLineDist {
                        p: pp,
                        l1,
                        l2,
                        target: signed.signum() * d.value,
                    });
                }
                DimTarget::Lines { a, b } => {
                    let Some(sega) = doc.segment(*a) else { continue };
                    let Some(segb) = doc.segment(*b) else { continue };
                    let (Some(a1), Some(a2)) = (slot_of(sega.start), slot_of(sega.end)) else {
                        continue;
                    };
                    let (Some(b1), _) = (slot_of(segb.start), slot_of(segb.end)) else {
                        continue;
                    };
                    let (Some(a1p), Some(a2p), Some(b1p)) = (
                        doc.point(sega.start),
                        doc.point(sega.end),
                        doc.point(segb.start),
                    ) else {
                        continue;
                    };
                    let dx = a2p.x - a1p.x;
                    let dy = a2p.y - a1p.y;
                    let l = (dx * dx + dy * dy).sqrt().max(1e-9);
                    let signed = (b1p.x - a1p.x) * (-dy / l) + (b1p.y - a1p.y) * (dx / l);
                    eqs.push(Eq::LineDist { a1, a2, b1, target: signed.signum() * d.value });
                }
                DimTarget::Radius { seg } => {
                    // Radius constraint: circumradius of the arc's defining
                    // triangle equals the placed value.
                    let Some(seg_d) = doc.segment(*seg) else { continue };
                    let (Some(s), Some(e)) = (slot_of(seg_d.start), slot_of(seg_d.end)) else {
                        continue;
                    };
                    let Some(cc) = seg_d.ctrl.and_then(|id| slot_of(id)) else {
                        continue;
                    };
                    eqs.push(Eq::ArcRadius { s, e, c: cc, target: d.value });
                }
                DimTarget::Angle { a, b } => {
                    let Some(sega) = doc.segment(*a) else { continue };
                    let Some(segb) = doc.segment(*b) else { continue };
                    let (Some(a1), Some(a2)) = (slot_of(sega.start), slot_of(sega.end)) else {
                        continue;
                    };
                    let (Some(b1), Some(b2)) = (slot_of(segb.start), slot_of(segb.end)) else {
                        continue;
                    };
                    // The placed SIGNED sweep (degrees) is the target — the
                    // rotation from ray A to ray B in the placed direction,
                    // exactly what the arc draws. Label, arc and constraint
                    // can never disagree.
                    eqs.push(Eq::Angle {
                        a1,
                        a2,
                        b1,
                        b2,
                        target: d.sweep.to_radians(),
                    });
                }
            }
        }

        // Constraint-less geometry (rulers, free lines) must still enter
        // the system when dragged, or their targets get dropped.
        for &(pid, _) in drag {
            let _ = slot_of(pid);
        }
        for &(pid, _) in aux {
            let _ = slot_of(pid);
        }

        let mut fixed = vec![true; slots.len()];
        let mut fixed_pos = vec![Point2::new(0., 0.); slots.len()];
        for (i, &pid) in slots.iter().enumerate() {
            fixed_pos[i] = doc.point(pid).unwrap();
        }

        let start: Vec<Point2> = fixed_pos.clone();

        let mut free_of = vec![None; slots.len()];
        let mut n_free = 0;

        let mut drag_idx: Vec<(usize, Point2)> = Vec::new();
        for &(pid, t) in drag {
            if let Some(&i) = index.get(&pid) {
                drag_idx.push((i, t));
                if free_of[i].is_none() {
                    free_of[i] = Some(n_free);
                    n_free += 1;
                    fixed[i] = false;
                }
            }
        }

        let mut aux_idx: Vec<(usize, Point2)> = Vec::new();
        for &(pid, anchor) in aux {
            if let Some(&i) = index.get(&pid) {
                aux_idx.push((i, anchor));
                if free_of[i].is_none() {
                    free_of[i] = Some(n_free);
                    n_free += 1;
                    fixed[i] = false;
                }
            }
        }

        Solver {
            slots,
            fixed,
            fixed_pos,
            start,
            eqs,
            drag: drag_idx,
            aux: aux_idx,
            free_of,
            n_free,
            anchor_weight: ANCHOR_WEIGHT,
        }
    }

    /// Convenience: no auxiliary followers.
    pub fn build_simple(doc: &Document, drag: &[(PointId, Point2)]) -> Solver {
        Self::build(doc, drag, &[])
    }

    /// Variant with a STRONG soft-anchor: used by dimension application,
    /// where the freed component must deform minimally to satisfy the new
    /// equation instead of drifting wherever the solve is easiest.
    pub fn build_with_anchor(
        doc: &Document,
        drag: &[(PointId, Point2)],
        aux: &[(PointId, Point2)],
        anchor_weight: f64,
    ) -> Solver {
        let mut solver = Self::build(doc, drag, aux);
        solver.anchor_weight = anchor_weight;
        solver
    }

    pub fn is_empty(&self) -> bool {
        self.n_free == 0 && self.eqs.is_empty()
    }

    fn has_work(&self) -> bool {
        self.n_free > 0
    }

    /// Current position of a slot: cursor target when eliminated, else the
    /// free iterate.
    fn pos(&self, s: usize, x: &[Point2]) -> Point2 {
        match self.free_of[s] {
            Some(v) => x[v],
            None => self.fixed_pos[s],
        }
    }

    /// Evaluates all weighted residuals (geometric + soft anchors).
    /// Variables are SCALARS: free slot v owns variables (2v, 2v+1).
    fn residuals(&self, x: &[Point2]) -> Vec<Residual> {
        let mut out = Vec::with_capacity(self.eqs.len() * 2 + self.n_free * 2);
        for eq in &self.eqs {
            match *eq {
                Eq::Horizontal { a, b } => {
                    let (pa, pb) = (self.pos(a, x), self.pos(b, x));
                    let mut grad = Vec::new();
                    if let Some(v) = self.free_of[a] {
                        grad.push((v * 2 + 1, 1.0));
                    }
                    if let Some(v) = self.free_of[b] {
                        grad.push((v * 2 + 1, -1.0));
                    }
                    out.push(Residual { value: pa.y - pb.y, grad, weight: EQ_WEIGHT });
                }
                Eq::Vertical { a, b } => {
                    let (pa, pb) = (self.pos(a, x), self.pos(b, x));
                    let mut grad = Vec::new();
                    if let Some(v) = self.free_of[a] {
                        grad.push((v * 2, 1.0));
                    }
                    if let Some(v) = self.free_of[b] {
                        grad.push((v * 2, -1.0));
                    }
                    out.push(Residual { value: pa.x - pb.x, grad, weight: EQ_WEIGHT });
                }
                Eq::Distance { a, b, target } => {
                    let (pa, pb) = (self.pos(a, x), self.pos(b, x));
                    let dx = pb.x - pa.x;
                    let dy = pb.y - pa.y;
                    let len = (dx * dx + dy * dy).sqrt().max(1e-9);
                    let ux = dx / len;
                    let uy = dy / len;
                    let mut grad = Vec::new();
                    if let Some(v) = self.free_of[a] {
                        // d|ab|/da = -(unit ab)
                        grad.push((v * 2, -ux));
                        grad.push((v * 2 + 1, -uy));
                    }
                    if let Some(v) = self.free_of[b] {
                        grad.push((v * 2, ux));
                        grad.push((v * 2 + 1, uy));
                    }
                    out.push(Residual { value: len - target, grad, weight: DIM_WEIGHT });
                }
                Eq::PointLineDist { p, l1, l2, target } => {
                    let (pp, pl1, pl2) = (self.pos(p, x), self.pos(l1, x), self.pos(l2, x));
                    let dx = pl2.x - pl1.x;
                    let dy = pl2.y - pl1.y;
                    let l = (dx * dx + dy * dy).sqrt().max(1e-9);
                    let nx = -dy / l;
                    let ny = dx / l;
                    let mut grad = Vec::new();
                    if let Some(v) = self.free_of[p] {
                        grad.push((v * 2, nx));
                        grad.push((v * 2 + 1, ny));
                    }
                    if let Some(v) = self.free_of[l1] {
                        grad.push((v * 2, -nx));
                        grad.push((v * 2 + 1, -ny));
                    }
                    // l2 only tilts the normal (second order) — no gradient.
                    out.push(Residual {
                        value: (pp.x - pl1.x) * nx + (pp.y - pl1.y) * ny - target,
                        grad,
                        weight: DIM_WEIGHT,
                    });
                }
                Eq::LineDist { a1, a2, b1, target } => {
                    let (pa1, pa2, pb1) =
                        (self.pos(a1, x), self.pos(a2, x), self.pos(b1, x));
                    let dx = pa2.x - pa1.x;
                    let dy = pa2.y - pa1.y;
                    let l = (dx * dx + dy * dy).sqrt().max(1e-9);
                    let nx = -dy / l;
                    let ny = dx / l;
                    let mut grad = Vec::new();
                    if let Some(v) = self.free_of[a1] {
                        grad.push((v * 2, -nx));
                        grad.push((v * 2 + 1, -ny));
                    }
                    if let Some(v) = self.free_of[b1] {
                        grad.push((v * 2, nx));
                        grad.push((v * 2 + 1, ny));
                    }
                    out.push(Residual {
                        value: (pb1.x - pa1.x) * nx + (pb1.y - pa1.y) * ny - target,
                        grad,
                        weight: DIM_WEIGHT,
                    });
                }
                Eq::Angle { a1, a2, b1, b2, target } => {
                    let (pa1, pa2, pb1, pb2) = (
                        self.pos(a1, x),
                        self.pos(a2, x),
                        self.pos(b1, x),
                        self.pos(b2, x),
                    );
                    let ax = pa2.x - pa1.x;
                    let ay = pa2.y - pa1.y;
                    let bx = pb2.x - pb1.x;
                    let by = pb2.y - pb1.y;
                    let c = ax * by - ay * bx;
                    let d = ax * bx + ay * by;
                    let denom = (c * c + d * d).max(1e-9);
                    // dTheta/dA and dTheta/dB of atan2(cross, dot).
                    let gax = (by * d - c * bx) / denom;
                    let gay = (-bx * d - c * by) / denom;
                    let gbx = (-ay * d - c * ax) / denom;
                    let gby = (ax * d - c * ay) / denom;
                    let mut grad = Vec::new();
                    if let Some(v) = self.free_of[a1] {
                        grad.push((v * 2, -gax));
                        grad.push((v * 2 + 1, -gay));
                    }
                    if let Some(v) = self.free_of[a2] {
                        grad.push((v * 2, gax));
                        grad.push((v * 2 + 1, gay));
                    }
                    if let Some(v) = self.free_of[b1] {
                        grad.push((v * 2, -gbx));
                        grad.push((v * 2 + 1, -gby));
                    }
                    if let Some(v) = self.free_of[b2] {
                        grad.push((v * 2, gbx));
                        grad.push((v * 2 + 1, gby));
                    }
                    out.push(Residual {
                        // Wrapped into (-PI, PI]: correct for ordinary AND
                        // reflex (|sweep| > 180 deg) targets.
                        value: {
                            let mut value = c.atan2(d) - target;
                            while value > std::f64::consts::PI {
                                value -= std::f64::consts::TAU;
                            }
                            while value <= -std::f64::consts::PI {
                                value += std::f64::consts::TAU;
                            }
                            value
                        },
                        grad,
                        weight: DIM_WEIGHT,
                    });
                }
                Eq::ArcRadius { s, e, c, target } => {
                    let (ps, pe, pc) = (self.pos(s, x), self.pos(e, x), self.pos(c, x));
                    // Chord geometry: half-length m, bend height h (signed
                    // perpendicular of the bend off the chord). Circumradius
                    // R = (m^2 + h^2) / (2h).
                    let wx = pe.x - ps.x;
                    let wy = pe.y - ps.y;
                    let l = (wx * wx + wy * wy).sqrt().max(1e-9);
                    let ux = wx / l;
                    let uy = wy / l;
                    let m = l / 2.;
                    // SIGNED bend height off the chord (either side is a
                    // valid arc); R uses |h| — clamping the sign away made
                    // half of all arcs unsatisfiable.
                    let h = (pc.x - ps.x) * (-uy) + (pc.y - ps.y) * ux;
                    let h_abs = h.abs().max(1e-6);
                    let value = (m * m + h_abs * h_abs) / (2. * h_abs) - target;
                    // dR/dh carries the sign of h (R is symmetric in +/-h),
                    // dR/dm = m / |h|.
                    let dr_dh = h.signum() * (h_abs * h_abs - m * m) / (2. * h_abs * h_abs);
                    let dr_dm = m / h;
                    let mut grad = Vec::new();
                    if let Some(v) = self.free_of[c] {
                        // dh/dc = perp(u)
                        grad.push((v * 2, -uy * dr_dh));
                        grad.push((v * 2 + 1, ux * dr_dh));
                    }
                    if let Some(v) = self.free_of[s] {
                        // dh/ds = -perp(u); dm/ds = -u/2
                        grad.push((v * 2, uy * dr_dh - ux / 2. * dr_dm));
                        grad.push((v * 2 + 1, -ux * dr_dh - uy / 2. * dr_dm));
                    }
                    if let Some(v) = self.free_of[e] {
                        // dh/de = 0 (first order); dm/de = +u/2
                        grad.push((v * 2, ux / 2. * dr_dm));
                        grad.push((v * 2 + 1, uy / 2. * dr_dm));
                    }
                    out.push(Residual { value, grad, weight: DIM_WEIGHT });
                }
            }
        }
        // Drag targets dominate; auxiliary followers anchor strongly to
        // their gesture-start positions.
        for &(i, t) in &self.drag {
            if let Some(v) = self.free_of[i] {
                out.push(Residual {
                    value: x[v].x - t.x,
                    grad: vec![(v * 2, 1.0)],
                    weight: DRAG_WEIGHT,
                });
                out.push(Residual {
                    value: x[v].y - t.y,
                    grad: vec![(v * 2 + 1, 1.0)],
                    weight: DRAG_WEIGHT,
                });
            }
        }
        for &(i, anchor) in &self.aux {
            if let Some(v) = self.free_of[i] {
                out.push(Residual {
                    value: x[v].x - anchor.x,
                    grad: vec![(v * 2, 1.0)],
                    weight: self.anchor_weight,
                });
                out.push(Residual {
                    value: x[v].y - anchor.y,
                    grad: vec![(v * 2 + 1, 1.0)],
                    weight: self.anchor_weight,
                });
            }
        }
        out
    }

    fn cost(residuals: &[Residual]) -> f64 {
        residuals.iter().map(|r| r.weight * r.value * r.value).sum()
    }

    /// Runs LM from current geometry. Returns new positions for all slots
    /// plus a status and the largest unsatisfied equation residuals
    /// (linear in doc units, angular in radians) — large values mean the
    /// system is over-constrained / infeasible.
    pub fn solve(&self) -> Solution {
        if !self.has_work() {
            return Solution {
                status: SolveStatus::Converged,
                positions: Vec::new(),
                max_lin_residual: 0.,
                max_angle_residual: 0.,
            };
        }

        let n = self.n_free * 2;
        // Free iterate in slot order.
        let mut x: Vec<Point2> =
            (0..self.slots.len()).filter(|&s| self.free_of[s].is_some()).map(|s| self.start[s]).collect();

        let mut lambda = LAMBDA_INIT;
        let mut status = SolveStatus::MaxIterations;
        let init_cost = Self::cost(&self.residuals(&x));
        let mut cost = init_cost;

        for _iter in 0..MAX_ITER {
            if cost < TOL {
                status = SolveStatus::Converged;
                break;
            }

            let mut jtj = vec![vec![0.0; n]; n];
            let mut jtr = vec![0.0; n];
            for r in &self.residuals(&x) {
                let w = r.weight;
                for &(vi, gi) in &r.grad {
                    for &(vj, gj) in &r.grad {
                        jtj[vi][vj] += w * gi * gj;
                    }
                    jtr[vi] -= w * gi * r.value;
                }
            }
            for i in 0..n {
                jtj[i][i] += lambda + 1e-12;
            }

            match gauss_solve(&jtj, &jtr) {
                Some(dx) => {
                    let trial: Vec<Point2> = x
                        .iter()
                        .zip(dx.chunks_exact(2))
                        .map(|(p, d)| {
                            let cap = 100.0;
                            Point2::new(p.x + d[0].clamp(-cap, cap), p.y + d[1].clamp(-cap, cap))
                                .clamped()
                        })
                        .collect();
                    let new_cost = Self::cost(&self.residuals(&trial));
                    if new_cost < cost {
                        x = trial;
                        cost = new_cost;
                        lambda = (lambda * 0.5).max(1e-9);
                    } else {
                        lambda *= 4.0;
                        if lambda > 1e8 {
                            break;
                        }
                    }
                }
                None => {
                    lambda *= 10.0;
                    if lambda > 1e8 {
                        break;
                    }
                }
            }
        }

        // Convergence: tight solve, or any improvement over the start (a
        // live drag re-solves next frame; partial progress is still applied).
        status = if cost < TOL || cost < init_cost {
            SolveStatus::Converged
        } else {
            SolveStatus::MaxIterations
        };

        let positions = self
            .slots
            .iter()
            .enumerate()
            .map(|(s, &id)| {
                if let Some(v) = self.free_of[s] {
                    (id, x[v])
                } else {
                    (id, self.fixed_pos[s])
                }
            })
            .collect();
        let (max_lin_residual, max_angle_residual) = self.eq_residual_max(&x);
        Solution {
            status,
            positions,
            max_lin_residual,
            max_angle_residual,
        }
    }

    /// Largest unsatisfied equation residual at the iterate `x`, split into
    /// linear (doc units) and angular (radians) families.
    fn eq_residual_max(&self, x: &[Point2]) -> (f64, f64) {
        let mut lin = 0.0f64;
        let mut ang = 0.0f64;
        for eq in &self.eqs {
            let v = match *eq {
                Eq::Horizontal { a, b } => (self.pos(a, x).y - self.pos(b, x).y).abs(),
                Eq::Vertical { a, b } => (self.pos(a, x).x - self.pos(b, x).x).abs(),
                Eq::Distance { a, b, target } => {
                    let (pa, pb) = (self.pos(a, x), self.pos(b, x));
                    let dx = pb.x - pa.x;
                    let dy = pb.y - pa.y;
                    ((dx * dx + dy * dy).sqrt() - target).abs()
                }
                Eq::PointLineDist { p, l1, l2, target } => {
                    let (pp, pl1, pl2) = (self.pos(p, x), self.pos(l1, x), self.pos(l2, x));
                    let dx = pl2.x - pl1.x;
                    let dy = pl2.y - pl1.y;
                    let l = (dx * dx + dy * dy).sqrt().max(1e-9);
                    (((pp.x - pl1.x) * (-dy / l) + (pp.y - pl1.y) * (dx / l)) - target).abs()
                }
                Eq::LineDist { a1, a2, b1, target } => {
                    let (pa1, pa2, pb1) =
                        (self.pos(a1, x), self.pos(a2, x), self.pos(b1, x));
                    let dx = pa2.x - pa1.x;
                    let dy = pa2.y - pa1.y;
                    let l = (dx * dx + dy * dy).sqrt().max(1e-9);
                    (((pb1.x - pa1.x) * (-dy / l) + (pb1.y - pa1.y) * (dx / l)) - target).abs()
                }
                Eq::Angle { a1, a2, b1, b2, target } => {
                    let mut v = signed_angle(
                        self.pos(a1, x),
                        self.pos(a2, x),
                        self.pos(b1, x),
                        self.pos(b2, x),
                    ) - target;
                    // Wrap into (-PI, PI]: correct for reflex sweeps too.
                    while v > std::f64::consts::PI {
                        v -= std::f64::consts::TAU;
                    }
                    while v <= -std::f64::consts::PI {
                        v += std::f64::consts::TAU;
                    }
                    ang = ang.max(v.abs());
                    continue;
                }
                Eq::ArcRadius { s, e, c, target } => {
                    let (ps, pe, pc) = (self.pos(s, x), self.pos(e, x), self.pos(c, x));
                    let wx = pe.x - ps.x;
                    let wy = pe.y - ps.y;
                    let l = (wx * wx + wy * wy).sqrt().max(1e-9);
                    let ux = wx / l;
                    let uy = wy / l;
                    let m = l / 2.;
                    let h = (pc.x - ps.x) * (-uy) + (pc.y - ps.y) * ux;
                    let h_abs = h.abs().max(1e-6);
                    let v = ((m * m + h_abs * h_abs) / (2. * h_abs) - target).abs();
                    lin = lin.max(v);
                    continue;
                }
            };
            lin = lin.max(v);
        }
        (lin, ang)
    }
}

/// Signed angle (radians) between the directions a1->a2 and b1->b2.
fn signed_angle(a1: Point2, a2: Point2, b1: Point2, b2: Point2) -> f64 {
    let ax = a2.x - a1.x;
    let ay = a2.y - a1.y;
    let bx = b2.x - b1.x;
    let by = b2.y - b1.y;
    (ax * by - ay * bx).atan2(ax * bx + ay * by)
}

/// Gaussian elimination with partial pivoting; None when singular.
fn gauss_solve(a: &[Vec<f64>], b: &[f64]) -> Option<Vec<f64>> {
    let n = b.len();
    let mut m: Vec<Vec<f64>> = a.iter().cloned().collect();
    for i in 0..n {
        m[i].push(b[i]);
    }
    for col in 0..n {
        let pivot = (col..n)
            .max_by(|r1, r2| m[*r1][col].abs().partial_cmp(&m[*r2][col].abs()).unwrap())?;
        if m[pivot][col].abs() < 1e-14 {
            return None;
        }
        m.swap(col, pivot);
        let inv = 1.0 / m[col][col];
        for r in col + 1..n {
            let factor = m[r][col] * inv;
            if factor != 0.0 {
                for c in col..=n {
                    m[r][c] -= factor * m[col][c];
                }
            }
        }
    }
    let mut out = vec![0.0; n];
    for r in (0..n).rev() {
        let mut sum = m[r][n];
        for c in r + 1..n {
            sum -= m[r][c] * out[c];
        }
        out[r] = sum / m[r][r];
    }
    Some(out)
}
