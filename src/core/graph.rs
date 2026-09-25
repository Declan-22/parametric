use std::collections::HashMap;
use std::rc::Rc;

use super::constraints::{ConstraintKind, DimMode, DimTarget};
use super::document::{Document, SegmentKind};
use super::ids::{PointId, SegmentId};

// Constraint topology: which points belong to the same locked system,
// shared with the solver so drag frames stop rebuilding it from scratch.
//
// `Graph` answers "what moves together" (subsystems + DOF estimate) for
// diagnostics; `Topology` is the solver's exact adjacency (segment
// cliques + constraint links + per-point line/curve ownership), cached on
// the document behind a structural fingerprint. Positions, layers, fills,
// settings, and dimension values never touch the fingerprint — they don't
// change the equation structure, so drags (pure moves) always hit cache.
//
// DOF here is an ESTIMATE (2n − equations, floored at 0), not a Jacobian
// rank — redundant equations (e.g. the three radius distances) over-count.
// Rank computation lands with decomposition; until then this answers
// "what moves together" and "roughly how floppy", which is what the
// editor and diagnostics need.

pub struct Subsystem {
    pub points: Vec<PointId>,
    pub constraint_count: usize,
    pub dimension_count: usize,
    pub equation_count: usize,
}

impl Subsystem {
    pub fn dof_estimate(&self) -> usize {
        (2 * self.points.len()).saturating_sub(self.equation_count)
    }
}

pub struct Graph {
    pub subsystems: Vec<Subsystem>,
}

impl Graph {
    pub fn build(doc: &Document) -> Self {
        let mut parent: HashMap<PointId, PointId> = HashMap::new();
        let union = |a: PointId, b: PointId, parent: &mut HashMap<PointId, PointId>| {
            let ra = find(a, parent);
            let rb = find(b, parent);
            if ra != rb {
                parent.insert(ra, rb);
            }
        };

        for (pid, _) in doc.all_points() {
            parent.entry(pid).or_insert(pid);
        }
        for (_, s) in doc.all_segments() {
            let mut pts = vec![s.start, s.end];
            pts.extend(s.ctrl);
            pts.extend(s.center);
            for w in pts.windows(2) {
                union(w[0], w[1], &mut parent);
            }
            if let (Some(&first), Some(&last)) = (pts.first(), pts.last()) {
                union(first, last, &mut parent);
            }
        }
        for c in &doc.constraints {
            union(c.a, c.b, &mut parent);
            if let Some(sid) = c.point_on_segment
                && let Some(seg) = doc.segment(sid)
            {
                for p in [Some(seg.start), Some(seg.end), seg.ctrl, seg.center]
                    .into_iter()
                    .flatten()
                {
                    union(c.a, p, &mut parent);
                    union(c.b, p, &mut parent);
                }
            }
            if let Some((first, second)) = c.tangent_segments {
                for sid in [first, second] {
                    if let Some(seg) = doc.segment(sid) {
                        for p in [Some(seg.start), Some(seg.end), seg.ctrl, seg.center]
                            .into_iter()
                            .flatten()
                        {
                            union(c.a, p, &mut parent);
                        }
                    }
                }
            }
        }
        for d in &doc.dimensions {
            match d.target {
                DimTarget::Points { a, b, .. } => union(a, b, &mut parent),
                DimTarget::PointLine { p, line } => {
                    union(p, p, &mut parent);
                    if let Some(seg) = doc.segment(line) {
                        union(p, seg.start, &mut parent);
                        union(p, seg.end, &mut parent);
                    }
                }
                DimTarget::Lines { a, b }
                | DimTarget::Angle { a, b }
                | DimTarget::EdgeMid { a, b, .. } => {
                    for sid in [a, b] {
                        if let Some(seg) = doc.segment(sid) {
                            union(seg.start, seg.end, &mut parent);
                        }
                    }
                    if let (Some(sa), Some(sb)) = (doc.segment(a), doc.segment(b)) {
                        union(sa.start, sb.start, &mut parent);
                    }
                }
                DimTarget::Radius { seg } | DimTarget::CurveLength { seg } => {
                    if let Some(s) = doc.segment(seg) {
                        union(s.start, s.end, &mut parent);
                    }
                }
            }
        }

        let mut order: HashMap<PointId, usize> = HashMap::new();
        let mut subs: Vec<Subsystem> = Vec::new();
        let mut points: Vec<PointId> = parent.keys().copied().collect();
        points.sort_by_key(|p| (p.idx, p.generation));
        for pid in points {
            let root = find(pid, &mut parent);
            let idx = match order.get(&root) {
                Some(&i) => i,
                None => {
                    let i = subs.len();
                    subs.push(Subsystem {
                        points: Vec::new(),
                        constraint_count: 0,
                        dimension_count: 0,
                        equation_count: 0,
                    });
                    order.insert(root, i);
                    i
                }
            };
            subs[idx].points.push(pid);
        }

        let at = |pid: PointId, order: &HashMap<PointId, usize>, parent: &mut HashMap<PointId, PointId>| {
            order.get(&find(pid, parent)).copied()
        };
        for c in &doc.constraints {
            if let Some(i) = at(c.a, &order, &mut parent) {
                subs[i].constraint_count += 1;
                subs[i].equation_count += match c.kind {
                    ConstraintKind::Horizontal | ConstraintKind::Vertical => 1,
                    ConstraintKind::Coincident => {
                        2 + usize::from(c.point_on_segment.is_some())
                    }
                    ConstraintKind::Tangent
                    | ConstraintKind::Parallel
                    | ConstraintKind::Perpendicular => 1,
                };
            }
        }
        for d in &doc.dimensions {
            let first = match d.target {
                DimTarget::Points { a, .. } => Some(a),
                DimTarget::PointLine { p, .. } => Some(p),
                DimTarget::Lines { a, .. }
                | DimTarget::Angle { a, .. }
                | DimTarget::EdgeMid { a, .. } => {
                    doc.segment(a).map(|s| s.start)
                }
                DimTarget::Radius { seg } | DimTarget::CurveLength { seg } => {
                    doc.segment(seg).map(|s| s.start)
                }
            };
            if let Some(i) = first.and_then(|p| at(p, &order, &mut parent)) {
                subs[i].dimension_count += 1;
                subs[i].equation_count += match d.target {
                    DimTarget::Radius { .. } => 3,
                    _ => 1,
                };
            }
        }
        for (_, s) in doc.all_segments() {
            if s.kind != SegmentKind::Arc {
                continue;
            }
            if let Some(i) = at(s.start, &order, &mut parent) {
                subs[i].equation_count += 2;
            }
        }

        subs.sort_by_key(|s| {
            s.points
                .iter()
                .map(|p| (p.idx, p.generation))
                .min()
                .unwrap_or((u32::MAX, u32::MAX))
        });
        Graph { subsystems: subs }
    }

    pub fn subsystem_of(&self, pid: PointId) -> Option<usize> {
        self.subsystems
            .iter()
            .position(|s| s.points.contains(&pid))
    }

    pub fn is_empty(&self) -> bool {
        self.subsystems.is_empty()
    }
}

fn find(mut pid: PointId, parent: &mut HashMap<PointId, PointId>) -> PointId {
    let mut root = pid;
    while let Some(&next) = parent.get(&root) {
        if next == root {
            break;
        }
        root = next;
    }
    while pid != root {
        let next = parent.get(&pid).copied().unwrap_or(pid);
        parent.insert(pid, root);
        pid = next;
    }
    root
}

// -- solver topology (SOL-06) --

/// The solver's exact point adjacency: segment cliques, constraint links,
/// and per-point line/curve ownership for tangent inference. Rebuilt only
/// when the structural fingerprint changes; drags (pure moves) reuse it.
#[derive(Clone, Debug, Default)]
pub struct Topology {
    pub adjacency: HashMap<PointId, Vec<PointId>>,
    pub touch_line: HashMap<PointId, SegmentId>,
    pub touch_curve: HashMap<PointId, SegmentId>,
}

fn mix(h: u64, v: u64) -> u64 {
    (h ^ v).wrapping_mul(0x100000001b3)
}

fn pid_hash(p: PointId) -> u64 {
    ((p.idx as u64) << 32) | p.generation as u64
}

fn opt_pid_hash(p: Option<PointId>, h: u64) -> u64 {
    match p {
        Some(id) => mix(mix(h, 1), pid_hash(id)),
        None => mix(h, 0),
    }
}

fn sid_hash(s: SegmentId) -> u64 {
    ((s.idx as u64) << 32) | s.generation as u64
}

fn opt_sid_hash(s: Option<SegmentId>, h: u64) -> u64 {
    match s {
        Some(id) => mix(mix(h, 1), sid_hash(id)),
        None => mix(h, 0),
    }
}

/// Structural fingerprint of everything the solver's equation graph reads:
/// live points/segments (ids + generations), segment wiring + kinds,
/// constraint endpoints/kinds/links, dimension targets + modes, modifier
/// wiring. Positions, values, layers, fills, and settings are ignored on
/// purpose — they never change which points link to which.
pub fn fingerprint_of(doc: &Document) -> u64 {
    let mut h = 0xcbf29ce484222325u64;
    for (pid, _) in doc.all_points() {
        h = mix(h, pid_hash(pid));
    }
    h = mix(h, 0x9e3779b97f4a7c15);
    for (sid, s) in doc.all_segments() {
        h = mix(h, sid_hash(sid));
        let kind = match s.kind {
            SegmentKind::Line => 1,
            SegmentKind::Ruler => 2,
            SegmentKind::Arc => 3,
            SegmentKind::Bezier => 4,
        };
        h = mix(h, kind);
        h = mix(h, pid_hash(s.start));
        h = mix(h, pid_hash(s.end));
        h = opt_pid_hash(s.ctrl, h);
        h = opt_pid_hash(s.center, h);
    }
    h = mix(h, 0xbf58476d1ce4e5b9);
    for c in &doc.constraints {
        let kind = match c.kind {
            ConstraintKind::Coincident => 0,
            ConstraintKind::Horizontal => 1,
            ConstraintKind::Vertical => 2,
            ConstraintKind::Tangent => 3,
            ConstraintKind::Parallel => 4,
            ConstraintKind::Perpendicular => 5,
        };
        h = mix(h, kind);
        h = mix(h, pid_hash(c.a));
        h = mix(h, pid_hash(c.b));
        h = opt_sid_hash(c.point_on_segment, h);
        match c.tangent_segments {
            Some((a, b)) => {
                h = mix(mix(h, 1), sid_hash(a));
                h = mix(h, sid_hash(b));
            }
            None => h = mix(h, 0),
        }
    }
    h = mix(h, 0x94d049bb133111eb);
    for d in &doc.dimensions {
        h = match d.target {
            DimTarget::Points { a, b, mode } => {
                let mode = match mode {
                    DimMode::Aligned => 0,
                    DimMode::X => 1,
                    DimMode::Y => 2,
                };
                mix(mix(mix(h, 10), pid_hash(a)), pid_hash(b)) ^ mode
            }
            DimTarget::EdgeMid { a, b, mode } => {
                let mode = match mode {
                    DimMode::Aligned => 0,
                    DimMode::X => 1,
                    DimMode::Y => 2,
                };
                mix(mix(mix(h, 11), sid_hash(a)), sid_hash(b)) ^ mode
            }
            DimTarget::PointLine { p, line } => {
                mix(mix(mix(h, 12), pid_hash(p)), sid_hash(line))
            }
            DimTarget::Lines { a, b } => mix(mix(mix(h, 13), sid_hash(a)), sid_hash(b)),
            DimTarget::Angle { a, b } => mix(mix(mix(h, 14), sid_hash(a)), sid_hash(b)),
            DimTarget::Radius { seg } => mix(mix(h, 15), sid_hash(seg)),
            DimTarget::CurveLength { seg } => mix(mix(h, 16), sid_hash(seg)),
        };
    }
    h = mix(h, 0xda942042e4dd58b5);
    for m in &doc.modifiers {
        h = mix(h, sid_hash(m.first));
        h = mix(h, sid_hash(m.second));
        h = opt_pid_hash(Some(m.corner), h);
        h = opt_sid_hash(m.arc, h);
        h = opt_pid_hash(m.first_tangent, h);
        h = opt_pid_hash(m.second_tangent, h);
        h = opt_pid_hash(m.center, h);
        h = opt_pid_hash(m.control, h);
    }
    h
}

impl Topology {
    /// Rebuilds from the document. Mirrors the solver's former inline
    /// construction exactly: segment cliques, endpoint-only tangent
    /// ownership, constraint links, sorted + deduped adjacency.
    pub fn rebuild(doc: &Document) -> Self {
        let mut topo = Topology::default();
        let mut link = |a: PointId, b: PointId| {
            if a == b {
                return;
            }
            topo.adjacency.entry(a).or_default().push(b);
            topo.adjacency.entry(b).or_default().push(a);
        };
        for (sid, s) in doc.all_segments() {
            let pts = [Some(s.start), Some(s.end), s.ctrl, s.center];
            let mut prev: Option<PointId> = None;
            for p in pts.into_iter().flatten() {
                if let Some(q) = prev {
                    link(q, p);
                }
                prev = Some(p);
            }
            for p in [s.start, s.end] {
                if s.kind == SegmentKind::Line {
                    topo.touch_line.entry(p).or_insert(sid);
                } else if matches!(s.kind, SegmentKind::Arc | SegmentKind::Bezier) {
                    topo.touch_curve.entry(p).or_insert(sid);
                }
            }
            link(s.start, s.end);
        }
        for c in &doc.constraints {
            link(c.a, c.b);
            if let Some(sid) = c.point_on_segment
                && let Some(seg) = doc.segment(sid)
            {
                for p in [Some(seg.start), Some(seg.end), seg.ctrl, seg.center].into_iter().flatten() {
                    link(c.a, p);
                    link(c.b, p);
                }
            }
        }
        for v in topo.adjacency.values_mut() {
            v.sort_by_key(|p| (p.idx, p.generation));
            v.dedup();
        }
        topo
    }

    /// Cached topology for a document: rebuilds only when the structural
    /// fingerprint changed. Share the `Rc`, never the derivation.
    pub fn for_document(doc: &Document, slot: &mut Option<(u64, Rc<Topology>)>) -> Rc<Topology> {
        let fp = fingerprint_of(doc);
        if let Some((old_fp, topo)) = slot.as_ref() {
            if *old_fp == fp {
                return Rc::clone(topo);
            }
        }
        let topo = Rc::new(Topology::rebuild(doc));
        *slot = Some((fp, Rc::clone(&topo)));
        topo
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::constraints::{DimMode, Dimension};
    use crate::core::geometry::Point2;

    fn rectangle() -> (Document, [PointId; 4]) {
        let mut doc = Document::new();
        let p = [
            doc.add_point(Point2::new(0., 0.)),
            doc.add_point(Point2::new(100., 0.)),
            doc.add_point(Point2::new(100., 50.)),
            doc.add_point(Point2::new(0., 50.)),
        ];
        doc.add_segment(p[0], p[1]);
        doc.add_segment(p[1], p[2]);
        doc.add_segment(p[2], p[3]);
        doc.add_segment(p[3], p[0]);
        doc.add_constraint(ConstraintKind::Horizontal, p[0], p[1]);
        doc.add_constraint(ConstraintKind::Horizontal, p[2], p[3]);
        doc.add_constraint(ConstraintKind::Vertical, p[0], p[3]);
        doc.add_constraint(ConstraintKind::Vertical, p[1], p[2]);
        (doc, p)
    }

    #[test]
    fn rectangle_is_one_subsystem_with_dof_4() {
        let (doc, p) = rectangle();
        let g = Graph::build(&doc);
        assert_eq!(g.subsystems.len(), 1);
        let s = &g.subsystems[0];
        assert_eq!(s.points.len(), 4);
        assert_eq!(s.constraint_count, 4);
        assert_eq!(s.equation_count, 4);
        assert_eq!(s.dof_estimate(), 4);
        assert!(p.iter().all(|pid| g.subsystem_of(*pid) == Some(0)));
    }

    #[test]
    fn disjoint_rectangles_are_two_subsystems() {
        let (mut doc, _) = rectangle();
        let q = [
            doc.add_point(Point2::new(500., 500.)),
            doc.add_point(Point2::new(600., 500.)),
        ];
        doc.add_segment(q[0], q[1]);
        doc.add_constraint(ConstraintKind::Horizontal, q[0], q[1]);
        let g = Graph::build(&doc);
        assert_eq!(g.subsystems.len(), 2);
        let mut dofs: Vec<usize> = g.subsystems.iter().map(|s| s.dof_estimate()).collect();
        dofs.sort_unstable();
        assert_eq!(dofs, vec![3, 4]);
    }

    #[test]
    fn coincident_and_dimension_join_subsystems() {
        let (mut doc, p) = rectangle();
        let lone = doc.add_point(Point2::new(1000., 1000.));
        assert_eq!(Graph::build(&doc).subsystems.len(), 2);
        doc.add_constraint(ConstraintKind::Coincident, p[0], lone);
        assert_eq!(Graph::build(&doc).subsystems.len(), 1);

        let mut doc2 = Document::new();
        let a = doc2.add_point(Point2::new(0., 0.));
        let b = doc2.add_point(Point2::new(10., 0.));
        let c = doc2.add_point(Point2::new(100., 100.));
        let d = doc2.add_point(Point2::new(110., 100.));
        doc2.add_segment(a, b);
        doc2.add_segment(c, d);
        assert_eq!(Graph::build(&doc2).subsystems.len(), 2);
        doc2.add_dimension(Dimension {
            target: DimTarget::Points { a: b, b: c, mode: DimMode::Aligned },
            value: 50.,
            offset: 0.,
            slide: 0.,
            sweep: 0.,
        });
        let g = Graph::build(&doc2);
        assert_eq!(g.subsystems.len(), 1);
        assert_eq!(g.subsystem_of(b), g.subsystem_of(c));
    }

    #[test]
    fn empty_document_has_no_subsystems() {
        let doc = Document::new();
        let g = Graph::build(&doc);
        assert!(g.is_empty());
    }

    #[test]
    fn topology_cache_survives_moves_but_not_rewires() {
        let (mut doc, p) = rectangle();
        let fp = fingerprint_of(&doc);
        let t0 = doc.topology();
        doc.move_point(p[0], Point2::new(5., 5.));
        assert_eq!(fp, fingerprint_of(&doc));
        assert!(Rc::ptr_eq(&t0, &doc.topology()));
        doc.add_constraint(ConstraintKind::Coincident, p[0], p[2]);
        assert_ne!(fp, fingerprint_of(&doc));
        assert!(!Rc::ptr_eq(&t0, &doc.topology()));
    }

    #[test]
    fn document_equality_ignores_topology_cache() {
        let (doc, _) = rectangle();
        let (cold, _) = rectangle();
        let _ = doc.topology();
        assert_eq!(doc, cold);
        let shared = doc.clone();
        assert!(Rc::ptr_eq(&doc.topology(), &shared.topology()));
    }

    #[test]
    fn topology_adjacency_matches_segment_clique() {
        let (doc, p) = rectangle();
        let topo = doc.topology();
        let mut nbrs = topo.adjacency.get(&p[0]).cloned().unwrap_or_default();
        nbrs.sort_by_key(|q| (q.idx, q.generation));
        let mut expect = vec![p[1], p[3]];
        expect.sort_by_key(|q| (q.idx, q.generation));
        assert_eq!(nbrs, expect);
        assert_eq!(topo.touch_line.len(), 4);
        assert!(topo.touch_curve.is_empty());
    }
}
