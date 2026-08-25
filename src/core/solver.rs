use super::constraints::ConstraintKind;
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
            let Some(target) = d.value else { continue };
            let (Some(a), Some(b)) = (slot_of(d.a), slot_of(d.b)) else {
                continue;
            };
            eqs.push(Eq::Distance { a, b, target });
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
        }
    }

    /// Convenience: no auxiliary followers.
    pub fn build_simple(doc: &Document, drag: &[(PointId, Point2)]) -> Solver {
        Self::build(doc, drag, &[])
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
                    weight: ANCHOR_WEIGHT,
                });
                out.push(Residual {
                    value: x[v].y - anchor.y,
                    grad: vec![(v * 2 + 1, 1.0)],
                    weight: ANCHOR_WEIGHT,
                });
            }
        }
        out
    }

    fn cost(residuals: &[Residual]) -> f64 {
        residuals.iter().map(|r| r.weight * r.value * r.value).sum()
    }

    /// Runs LM from current geometry. Returns new positions for all slots
    /// plus a status.
    pub fn solve(&self) -> Solution {
        if !self.has_work() {
            return Solution { status: SolveStatus::Converged, positions: Vec::new() };
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
        Solution { status, positions }
    }
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
