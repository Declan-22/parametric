use crate::core::constraints::ElementRef;
use crate::core::geometry::Point2;
use crate::core::ids::SegmentId;

use super::Editor;
use super::PendingBezier;
use super::PendingCircle;
use super::PendingPen;
use super::PenMode;
use super::bezier;
use super::pick;
use super::snapping;
use super::tools;

// Pen tool: one unified path tool, chaining line/arc/bezier spans.
impl Editor {
    /// Switches the pen sub-mode (explicit only — drag never changes it).
    /// The chain anchor is PRESERVED so the path never breaks: the new
    /// mode stages its next span from the same live endpoint.
    pub fn set_pen_mode(&mut self, mode: PenMode) -> bool {
        if self.pen_mode == mode {
            return false;
        }
        self.pen_mode = mode;
        self.perpendicular_preview = None;
        self.snap_guides.clear();
        // Re-stage the new mode from the live chain point (if any) so
        // Line -> Bezier -> Arc flows as one path. A half-finished arc
        // chord is dropped — its two clicks belong to the old mode.
        // Bezier restaging pre-mirrors the near handle (smooth preview).
        self.pending_pen = self.pen_anchor.map(|a| {
            let mut pen = PendingPen::for_mode(mode, a);
            if mode == PenMode::Bezier
                && let Some(pb) = pen.bezier.as_mut()
            {
                pb.h1 = self
                    .pen_mirror_handle()
                    .or_else(|| self.pen_incoming_line_handle());
            }
            pen
        });
        self.pending_bezier = None;
        true
    }

    /// Cancels the live pen path entirely (Esc).
    pub fn cancel_pen_path(&mut self) -> bool {
        if self.pending_pen.is_none() && self.pen_anchor.is_none() {
            return false;
        }
        self.pending_pen = None;
        self.pending_bezier = None;
        self.pen_anchor = None;
        self.pen_anchor_id = None;
        self.perpendicular_preview = None;
        self.snap_guides.clear();
        true
    }

    /// Merges a freshly committed span's start point into the live chain
    /// anchor id (when one exists) and advances the anchor to the span's
    /// end point. Every span type chains through shared ids — no stacked
    /// duplicate points at joints.
    fn pen_chain_end(&mut self, seg: SegmentId) {
        let Some(s) = self.doc.segment(seg) else { return };
        let (fresh_start, end) = (s.start, s.end);
        if let Some(aid) = self.pen_anchor_id {
            if self.doc.point(aid).is_some() && fresh_start != aid {
                self.doc.merge_point(aid, fresh_start);
            }
        }
        self.pen_anchor_id = Some(end);
        if let Some(p) = self.doc.point(end) {
            self.pen_anchor = Some(p);
        }
    }

    /// Mirror of the previous bezier's far handle across the chain anchor:
    /// the default near handle for a chained bezier span (smooth joints
    /// out of the box). Prefers the span ARRIVING at the anchor (its far
    /// handle) over one leaving it. None when no bezier touches it.
    fn pen_mirror_handle(&self) -> Option<Point2> {
        let aid = self.pen_anchor_id?;
        let anchor = self.doc.point(aid)?;
        let mut fallback: Option<Point2> = None;
        for (_, s) in self.doc.all_segments() {
            if s.kind != crate::core::document::SegmentKind::Bezier {
                continue;
            }
            // Arriving span first: its far handle mirrors into our near one.
            if s.end == aid
                && let Some(hp) = s.center.and_then(|h| self.doc.point(h))
                && pick::distance(hp, anchor) > 1e-6
            {
                return Some(Point2::new(
                    2. * anchor.x - hp.x,
                    2. * anchor.y - hp.y,
                ));
            }
            if s.start == aid && fallback.is_none() {
                fallback = s.ctrl.and_then(|h| self.doc.point(h)).and_then(|hp| {
                    (pick::distance(hp, anchor) > 1e-6).then_some(Point2::new(
                        2. * anchor.x - hp.x,
                        2. * anchor.y - hp.y,
                    ))
                });
            }
        }
        fallback
    }

    /// Default near handle for the first Bezier span after a line. This is
    /// also used when starting on an existing line, not only while chaining.
    /// This is only preview/creation state: it does not add a tangent
    /// constraint and an explicit handle drag can replace it.
    fn pen_incoming_line_handle(&self) -> Option<Point2> {
        let anchor = self.pen_anchor?;
        let (_, _, start, end, distance) = self
            .doc
            .all_segments()
            .filter(|(_, s)| s.kind == crate::core::document::SegmentKind::Line)
            .filter_map(|(id, line)| {
                let start = self.doc.point(line.start)?;
                let end = self.doc.point(line.end)?;
                let distance = pick::point_segment_distance(anchor, start, end);
                Some((id, line, start, end, distance))
            })
            .min_by(|a, b| a.4.partial_cmp(&b.4).unwrap_or(std::cmp::Ordering::Equal))?;
        if distance > self.snap_tol_doc() * 2. {
            return None;
        }
        let dx = end.x - start.x;
        let dy = end.y - start.y;
        let length = (dx * dx + dy * dy).sqrt();
        (length > 1e-6).then_some(Point2::new(
            anchor.x + dx / 3.,
            anchor.y + dy / 3.,
        ))
    }

    pub(crate) fn pen_tool_click(&mut self, cursor: gpui::Point<gpui::Pixels>, shift: bool) -> bool {
        let (at, guides) = self.snap_creation_point(self.cursor_doc(cursor));
        self.snap_guides = guides;
        match self.pen_mode {
            PenMode::Line => self.pen_line_click(at, shift),
            PenMode::Arc => self.pen_arc_click(at, shift),
            PenMode::Bezier => self.pen_bezier_click(at),
        }
    }

    /// Commits one pen line span anchor -> b. Shared by press-commit and
    /// release-commit; re-stages the next span from b.
    fn commit_pen_line(&mut self, b: Point2, shift: bool) {
        let Some(anchor) = self.pen_anchor else { return };
        self.snap_guides.clear();
        let layer_id = self.doc.layers[0].id;
        let seg = self.create_line(layer_id, anchor, b);
        // Merge FIRST: constraint creation below must reference the live
        // (post-merge) point ids, or the constraints dangle on a destroyed
        // point and their chips never render.
        self.pen_chain_end(seg);
        if let Some((source, _, _)) = self.perpendicular_preview.take() {
            self.doc.add_perpendicular_constraint(source, seg);
        }
        if shift {
            self.maybe_add_tangent(seg, b);
            self.auto_tangent_for_pen(seg);
        }
        self.selection = vec![ElementRef::Segment(seg)];
        let chained = self.pen_anchor.unwrap_or(b);
        self.pending_pen = Some(PendingPen {
            mode: PenMode::Line,
            line: Some(tools::PendingLine { start: chained, cursor: chained }),
            bezier: None,
            circle: None,
        });
    }

    fn pen_line_click(&mut self, at: Point2, shift: bool) -> bool {
        // Press commits the LIVE preview (mirror the legacy line tool):
        // the preview has been tracking the cursor via moves, so commit
        // anchor -> preview end. A fresh/zero-length staging re-anchors.
        if let Some(pending) = self.pending_pen.take() {
            if let Some(line) = pending.line {
                let (_, mut b) = line.snapped(shift);
                if shift {
                    if let Some(q) = self.tangent_snap_for_line(line.start, line.cursor)
                        .or_else(|| self.bezier_tangent_snap(line.start, line.cursor))
                    {
                        b = q;
                    }
                }
                if pick::distance(b, line.start) > 1e-6 {
                    self.pending_pen = Some(pending);
                    self.commit_pen_line(b, shift);
                } else {
                    // Fresh link: re-anchor at the press point.
                    let (nat, g) = self.snap_creation_point(at);
                    self.snap_guides = g;
                    self.pen_anchor = Some(nat);
                    self.pending_pen = Some(PendingPen {
                        mode: PenMode::Line,
                        line: Some(tools::PendingLine { start: nat, cursor: nat }),
                        bezier: None,
                        circle: None,
                    });
                }
                self.pending_via_click = true;
                return true;
            }
            self.pending_pen = Some(pending);
        }
        // No staging (fresh tool or just switched with no anchor): anchor.
        let (nat, g) = self.snap_creation_point(at);
        self.snap_guides = g;
        self.pen_anchor = Some(nat);
        self.pending_pen = Some(PendingPen::for_mode(PenMode::Line, nat));
        self.pending_via_click = true;
        true
    }

    fn pen_arc_click(&mut self, at: Point2, shift: bool) -> bool {
        // 3-press arc through the shared chain anchor: press1 fixes the
        // chord start at the anchor, press2 fixes the chord end, press3
        // (on-arc point) commits and chains from the chord end.
        if let Some(pending) = self.pending_pen.take() {
            if let Some(mut pc) = pending.circle {
                if pc.a.is_some() && pc.b.is_some() {
                    self.pending_via_click = false;
                    self.snap_guides.clear();
                    if let (Some(a), Some(b)) = (pc.a, pc.b) {
                        let c = pc.cursor;
                        let layer_id = self.doc.layers[0].id;
                        let seg = self.create_arc(layer_id, a, b, c);
                        // Merge before constraining (see commit_pen_line).
                        self.pen_chain_end(seg);
                        self.auto_tangent_for_pen(seg);
                        self.selection = vec![ElementRef::Segment(seg)];
                        let chained = self.pen_anchor.unwrap_or(b);
                        self.pending_pen =
                            Some(PendingPen::for_mode(PenMode::Arc, chained));
                    } else {
                        self.pending_pen = Some(pending);
                    }
                    return true;
                }
                match (&pc.a, &pc.b) {
                    (Some(_), None) => {
                        let mut nat = at;
                        if shift && let Some(a) = pc.a {
                            nat = tools::snap_angle(a, nat);
                        }
                        pc.b = Some(nat);
                        pc.cursor = nat;
                        self.pending_via_click = true;
                    }
                    _ => {
                        // Anchor the chord start (anchor wins when set).
                        let start = self.pen_anchor.unwrap_or(at);
                        pc.a = Some(start);
                        pc.cursor = at;
                        self.pen_anchor = Some(start);
                        self.pending_via_click = true;
                    }
                }
                self.pending_pen = Some(PendingPen {
                    mode: PenMode::Arc,
                    line: None,
                    bezier: None,
                    circle: Some(pc),
                });
                return true;
            }
            self.pending_pen = Some(pending);
        }
        // Fresh staging starts at the live anchor when chaining.
        let start = self.pen_anchor.unwrap_or(at);
        self.pen_anchor = Some(start);
        let mut pc = PendingCircle { a: Some(start), b: None, cursor: at };
        // Single-press anchor set: cursor stays on the press.
        if self.pen_anchor == Some(at) {
            pc.cursor = at;
        }
        self.pending_pen = Some(PendingPen {
            mode: PenMode::Arc,
            line: None,
            bezier: None,
            circle: Some(pc),
        });
        self.pending_via_click = true;
        true
    }

    fn pen_bezier_click(&mut self, at: Point2) -> bool {
        // Press fixes geometry, release commits:
        //  - press 1 (no staging): anchor p0, await the second press;
        //  - press 2: fix the endpoint p1 = press point; the drag that
        //    follows shapes the far handle, release commits the span.
        if let Some(pending) = self.pending_pen.take() {
            if let Some(mut pb) = pending.bezier {
                // (Re-)fix the endpoint at the press point; the drag that
                // follows shapes the nearest handle, release commits.
                // The near side defaults to the smooth mirror of the
                // previous span (a later drag near that side overrides it).
                let (nat, g) = self.snap_creation_point(at);
                self.snap_guides = g;
                let mirror = self
                    .pen_mirror_handle()
                    .or_else(|| self.pen_incoming_line_handle());
                pb.p1 = Some(nat);
                pb.h1 = mirror;
                pb.h2 = None;
                pb.cursor = nat;
                self.pending_pen = Some(PendingPen {
                    mode: PenMode::Bezier,
                    line: None,
                    bezier: Some(pb),
                    circle: None,
                });
                self.pending_via_click = true;
                return true;
            }
            self.pending_pen = Some(pending);
        }
        // Fresh staging starts at the live anchor when chaining, with the
        // near handle pre-mirrored so the preview already guesses the
        // smooth continuation curve (never a straight chord).
        let start = self.pen_anchor.unwrap_or(at);
        let (nat, g) = if self.pen_anchor.is_some() {
            (start, Vec::new())
        } else {
            self.snap_creation_point(at)
        };
        self.snap_guides = g;
        self.pen_anchor = Some(nat);
        let h1 = self
            .pen_mirror_handle()
            .or_else(|| self.pen_incoming_line_handle());
        self.pending_pen = Some(PendingPen {
            mode: PenMode::Bezier,
            line: None,
            bezier: Some(PendingBezier {
                p0: nat,
                p1: None,
                h1,
                h2: None,
                cursor: at,
                active_handle: None,
            }),
            circle: None,
        });
        self.pending_via_click = true;
        true
    }

    /// Bezier release-commit: straight span on click-click, handled span
    /// when the second press dragged (handle = release point).
    fn pen_bezier_release(&mut self) -> bool {
        let Some(pending) = self.pending_pen.take() else {
            return false;
        };
        let Some(pb) = pending.bezier else {
            self.pending_pen = Some(pending);
            return false;
        };
        let Some(p1) = pb.p1 else {
            // First click only anchored p0 — await the second press.
            self.pending_pen = Some(PendingPen {
                mode: PenMode::Bezier,
                line: None,
                bezier: Some(pb),
                circle: None,
            });
            return true;
        };
        let end = p1;
        if pick::distance(end, pb.p0) <= 1e-6 {
            // Degenerate: keep staging, await a real endpoint.
            self.pending_pen = Some(PendingPen {
                mode: PenMode::Bezier,
                line: None,
                bezier: Some(PendingBezier {
                    p0: pb.p0,
                    p1: None,
                    h1: None,
                    h2: None,
                    cursor: pb.cursor,
                    active_handle: None,
                }),
                circle: None,
            });
            return true;
        }
        let lerp = |t: f64| {
            Point2::new(
                pb.p0.x + (end.x - pb.p0.x) * t,
                pb.p0.y + (end.y - pb.p0.y) * t,
            )
        };
        // Dragged handles win per side (the far drag rides opposite, via
        // `effective`); an untouched near side mirrors the previous span's
        // far handle (smooth joints); otherwise thirds (straight).
        let mirror = self
            .pen_mirror_handle()
            .or_else(|| self.pen_incoming_line_handle());
        let c1 = pb.h1.or(mirror).unwrap_or_else(|| lerp(1. / 3.));
        let c2 = pb
            .h2
            .map(|h| Point2::new(2. * end.x - h.x, 2. * end.y - h.y))
            .unwrap_or_else(|| lerp(2. / 3.));
        self.snap_guides.clear();
        let layer_id = self.doc.layers[0].id;
        let seg = self.create_bezier(layer_id, pb.p0, c1, c2, end);
        // Merge before constraining (see commit_pen_line).
        self.pen_chain_end(seg);
        self.auto_tangent_for_pen(seg);
        self.selection = vec![ElementRef::Segment(seg)];
        let chained = self.pen_anchor.unwrap_or(end);
        // Restage with the near handle already reflecting the span we
        // just laid (smooth continuation preview from the first move).
        let next_h1 = Point2::new(2. * end.x - c2.x, 2. * end.y - c2.y);
        let next_h1 = (pick::distance(next_h1, end) > 1e-6).then_some(next_h1);
        self.pending_pen = Some(PendingPen {
            mode: PenMode::Bezier,
            line: None,
            bezier: Some(PendingBezier {
                p0: chained,
                p1: None,
                h1: next_h1,
                h2: None,
                cursor: chained,
                active_handle: None,
            }),
            circle: None,
        });
        self.pending_via_click = true;
        true
    }

    /// Release-commit for the pen (mirrors the legacy line release):
    /// a dragged preview commits, a motionless click keeps staging.
    pub(crate) fn pen_release_commit(&mut self, shift: bool) -> bool {
        let Some(pending) = self.pending_pen.take() else {
            return false;
        };
        match pending.mode {
            PenMode::Bezier => {
                self.pending_pen = Some(pending);
                return self.pen_bezier_release();
            }
            PenMode::Line => {
                let Some(line) = pending.line else {
                    self.pending_pen = Some(pending);
                    return false;
                };
                let (_, mut b) = line.snapped(shift);
                if shift {
                    if let Some(q) = self.tangent_snap_for_line(line.start, line.cursor)
                        .or_else(|| self.bezier_tangent_snap(line.start, line.cursor))
                    {
                        b = q;
                    }
                }
                if self.pending_via_click && pick::distance(b, line.start) <= 1e-6 {
                    self.pending_pen = Some(pending);
                    return true;
                }
                self.pending_via_click = true;
                if pick::distance(b, line.start) > 1e-6 {
                    self.pending_pen = Some(pending);
                    self.commit_pen_line(b, shift);
                } else {
                    self.pending_pen = Some(pending);
                }
                return true;
            }
            // Arcs commit on the third press, never on release.
            PenMode::Arc => {
                self.pending_pen = Some(pending);
                return false;
            }
        }
    }

    /// Auto-tangent for chained pen spans: when the new span starts where
    /// the previous span ended (within snap tolerance) and directions align
    /// (~5°), bond them with a Tangent constraint + coincident endpoints.
    fn auto_tangent_for_pen(&mut self, fresh: SegmentId) {
        let Some(cur) = self.doc.segment(fresh) else { return };
        let (Some(cp0), Some(cp1)) = (self.doc.point(cur.start), self.doc.point(cur.end)) else {
            return;
        };
        let cur_tan = self.span_start_tangent(fresh);
        let tol = self.snap_tol_doc() * 1.5;
        // Find a prior span sharing the fresh start point.
        let mut prev_id: Option<SegmentId> = None;
        for (sid, _) in self.doc.all_segments() {
            if sid == fresh {
                continue;
            }
            let Some(s) = self.doc.segment(sid) else { continue };
            if !matches!(
                s.kind,
                crate::core::document::SegmentKind::Line
                    | crate::core::document::SegmentKind::Arc
                    | crate::core::document::SegmentKind::Bezier
            ) {
                continue;
            }
            let touches = [s.start, s.end].iter().any(|&p| {
                self.doc.point(p).is_some_and(|q| pick::distance(q, cp0) <= tol)
            });
            if touches {
                prev_id = Some(sid);
                break;
            }
        }
        let Some(prev) = prev_id else { return };
        let prev_tan = self.span_end_tangent(prev);
        let (Some(a), Some(b)) = (cur_tan, prev_tan) else { return };
        // Both tangents point AWAY from the joint; incoming must oppose.
        let dot = a.0 * b.0 + a.1 * b.1;
        if dot < -0.996 {
            let prev_seg = self.doc.segment(prev);
            // Line-on-line tangency is collinearity: Parallel (+ the joint
            // is already shared by chaining). A raw Tangent constraint has
            // no line-line equation and would sit dead.
            if cur.kind == crate::core::document::SegmentKind::Line
                && prev_seg.is_some_and(|s| s.kind == crate::core::document::SegmentKind::Line)
            {
                self.doc.add_parallel_constraint(prev, fresh);
                let _ = self.solve_constraint_now(&[
                    ElementRef::Segment(prev),
                    ElementRef::Segment(fresh),
                ]);
                return;
            }
            // Coincident joint (distinct ids from chaining).
            let pj = self.doc.segment(prev).map(|s| s.end);
            let contact = if let Some(pj) = pj
                && pj != cur.start
                && self.doc.point(pj).zip(self.doc.point(cur.start))
                    .is_some_and(|(a, b)| pick::distance(a, b) <= tol)
            {
                // Chained geometry shares a topological vertex. Merge the
                // ids instead of layering a solver Coincident constraint on
                // top of an already-connected joint.
                self.doc.merge_point(pj, cur.start);
                pj
            } else {
                cur.start
            };
            self.doc.add_tangent_constraint(prev, fresh, contact);
            let _ = self.solve_constraint_now(&[ElementRef::Segment(prev), ElementRef::Segment(fresh)]);
            let _ = cp1;
        }
    }

    pub(crate) fn span_start_tangent(&self, sid: SegmentId) -> Option<(f64, f64)> {
        let s = self.doc.segment(sid)?;
        let (p0, p1) = (self.doc.point(s.start)?, self.doc.point(s.end)?);
        match s.kind {
            crate::core::document::SegmentKind::Line => {
                let (dx, dy) = (p1.x - p0.x, p1.y - p0.y);
                let l = (dx * dx + dy * dy).sqrt().max(1e-9);
                Some((dx / l, dy / l))
            }
            crate::core::document::SegmentKind::Arc => {
                let c = s.ctrl.and_then(|id| self.doc.point(id))?;
                let (o, _) = crate::editor::arc::circumcircle(p0, p1, c)?;
                let (dx, dy) = (p0.x - o.x, p0.y - o.y);
                let l = (dx * dx + dy * dy).sqrt().max(1e-9);
                // Tangent at p0, oriented away (p0 -> along sweep). Sign from
                // sweep side: use perpendicular, pick the one leaving p0.
                let (tx, ty) = (-dy / l, dx / l);
                // Orient along the arc: dot with (c - p0) tangent side.
                let mx = c.x - p0.x;
                let my = c.y - p0.y;
                let s = tx * mx + ty * my;
                Some(if s >= 0. { (tx, ty) } else { (-tx, -ty) })
            }
            crate::core::document::SegmentKind::Bezier => {
                let (h1, h2) = s.bezier_handles();
                let c1 = h1.and_then(|id| self.doc.point(id)).unwrap_or(p1);
                let c2 = h2.and_then(|id| self.doc.point(id)).unwrap_or(p0);
                Some(bezier::end_tangent(p0, c1, c2, p1, true))
            }
            _ => None,
        }
    }

    pub(crate) fn span_end_tangent(&self, sid: SegmentId) -> Option<(f64, f64)> {
        let s = self.doc.segment(sid)?;
        let (p0, p1) = (self.doc.point(s.start)?, self.doc.point(s.end)?);
        match s.kind {
            crate::core::document::SegmentKind::Line => {
                let (dx, dy) = (p1.x - p0.x, p1.y - p0.y);
                let l = (dx * dx + dy * dy).sqrt().max(1e-9);
                Some((dx / l, dy / l))
            }
            crate::core::document::SegmentKind::Arc => {
                let c = s.ctrl.and_then(|id| self.doc.point(id))?;
                let (o, _) = crate::editor::arc::circumcircle(p0, p1, c)?;
                let (dx, dy) = (p1.x - o.x, p1.y - o.y);
                let l = (dx * dx + dy * dy).sqrt().max(1e-9);
                let (tx, ty) = (-dy / l, dx / l);
                let mx = c.x - p1.x;
                let my = c.y - p1.y;
                let sg = tx * mx + ty * my;
                Some(if sg >= 0. { (tx, ty) } else { (-tx, -ty) })
            }
            crate::core::document::SegmentKind::Bezier => {
                let (h1, h2) = s.bezier_handles();
                let c1 = h1.and_then(|id| self.doc.point(id)).unwrap_or(p1);
                let c2 = h2.and_then(|id| self.doc.point(id)).unwrap_or(p0);
                Some(bezier::end_tangent(p0, c1, c2, p1, false))
            }
            _ => None,
        }
    }

    /// Live pen preview tracking (called from `canvas_drag`).
    pub(crate) fn pen_drag_update(&mut self, cursor: gpui::Point<gpui::Pixels>, shift: bool) -> bool {
        let Some(pending) = self.pending_pen else {
            return false;
        };
        let at = self.cursor_doc(cursor);
        let (at, guides) = self.snap_creation_point(at);
        self.snap_guides = guides;
        match pending.mode {
            PenMode::Line => {
                let Some(line) = pending.line else {
                    return true;
                };
                // Free: plain object/grid snap only. Shift arms the whole
                // constraint layer — 45° lock, arc-tangent lock and the
                // perpendicular snap. Never yank the preview otherwise.
                let (tangent_at, perpendicular) = if shift {
                    let tan = self.tangent_snap_for_line(line.start, at)
                        .or_else(|| self.bezier_tangent_snap(line.start, at));
                    let perp =
                        self.perpendicular_snap_for_line(line.start, tan.unwrap_or(at));
                    (tan, perp)
                } else {
                    (None, None)
                };
                let final_at =
                    perpendicular.map(|(p, _)| p).unwrap_or(tangent_at.unwrap_or(at));
                self.perpendicular_preview =
                    perpendicular.map(|(p, sid)| (sid, line.start, p));
                if let Some(dst) = self.pending_pen.as_mut().and_then(|p| p.line.as_mut()) {
                    dst.cursor = final_at;
                }
            }
            PenMode::Arc => {
                let Some(mut pc) = pending.circle else {
                    return true;
                };
                let (nat, shifted) =
                    Self::arc_creation_shift(pc.stage(), pc.a, pc.b, at, shift);
                if shifted {
                    self.snap_guides.clear();
                }
                pc.cursor = nat;
                if let Some(dst) = self.pending_pen.as_mut().and_then(|p| p.circle.as_mut()) {
                    *dst = pc;
                }
            }
            PenMode::Bezier => {
                let Some(mut pb) = pending.bezier else {
                    return true;
                };
                // Once grabbed, stay on the chosen handle throughout the drag gesture.
                // Do not switch handles mid-drag even if cursor moves closer to the other point.
                if pb.p1.is_some() {
                    let handle = pb.active_handle.unwrap_or_else(|| {
                        let (d0, d1) = (
                            pick::distance(at, pb.p0),
                            pb.p1.map(|p| pick::distance(at, p)).unwrap_or(f64::MAX),
                        );
                        if d1 <= d0 { 2 } else { 1 }
                    });
                    pb.active_handle = Some(handle);
                    if handle == 2 {
                        pb.h2 = Some(at);
                    } else {
                        pb.h1 = Some(at);
                    }
                }
                pb.cursor = at;
                if let Some(dst) = self.pending_pen.as_mut().and_then(|p| p.bezier.as_mut()) {
                    *dst = pb;
                }
            }
        }
        // Tangent preview: the preview span leaves the anchor along an
        // existing span's tangent — dashed ray off the anchor. Commit
        // applies the real constraint (chip appears then).
        if let (Some(anchor), Some((_, out))) = (self.pen_anchor, self.pen_preview_tangent()) {
            let ray = 28. / self.camera.zoom;
            self.snap_guides.push(snapping::SnapGuide {
                vertical: false,
                from: anchor,
                to: Point2::new(anchor.x + out.0 * ray, anchor.y + out.1 * ray),
                kind: snapping::SnapKind::Edge,
                solid: true,
                linked: false,
                span_is_x: false,
                span_lo: 0.,
                span_hi: 0.,
            });
        }
        true
    }

    /// Preview tangent alignment for the live pen span: the existing span
    /// id when the preview leaves the chain anchor along that span's own
    /// tangent (~5°). The drag layer draws a dashed guide off the anchor;
    /// commit applies the real Tangent constraint via
    /// `auto_tangent_for_pen` (whose chip then appears).
    fn pen_preview_tangent(&self) -> Option<(SegmentId, (f64, f64))> {
        let aid = self.pen_anchor_id?;
        if self.doc.point(aid).is_none() {
            return None;
        }
        let pending = self.pending_pen?;
        let unit = |v: (f64, f64)| {
            let l = (v.0 * v.0 + v.1 * v.1).sqrt();
            if l < 1e-6 { None } else { Some((v.0 / l, v.1 / l)) }
        };
        let out: Option<(f64, f64)> = match pending.mode {
            PenMode::Line => {
                let l = pending.line?;
                unit((l.cursor.x - l.start.x, l.cursor.y - l.start.y))
            }
            PenMode::Bezier => {
                let pb = pending.bezier?;
                let end = pb.p1.unwrap_or(pb.cursor);
                if pick::distance(end, pb.p0) < 1e-6 {
                    return None;
                }
                let (c1, c2) = pb.effective(end);
                Some(bezier::end_tangent(pb.p0, c1, c2, end, true))
            }
            PenMode::Arc => {
                let pc = pending.circle?;
                let a = pc.a?;
                let b = pc.b.unwrap_or(pc.cursor);
                unit((b.x - a.x, b.y - a.y))
            }
        };
        let out = out?;
        for (sid, s) in self.doc.all_segments() {
            if !matches!(
                s.kind,
                crate::core::document::SegmentKind::Line
                    | crate::core::document::SegmentKind::Arc
                    | crate::core::document::SegmentKind::Bezier
            ) {
                continue;
            }
            // Prev-span direction AWAY from the joint.
            let prev = if s.end == aid {
                self.span_end_tangent(sid)
            } else if s.start == aid {
                self.span_start_tangent(sid)
            } else {
                continue;
            };
            if let Some(p) = prev
                && out.0 * p.0 + out.1 * p.1 < -0.996
            {
                return Some((sid, out));
            }
        }
        None
    }

    /// Free (no-shift) pen tangent: ONLY continues a tangent when the span
    /// starts ON an existing bezier (smooth chaining). Never touches arcs
    /// so the preview is never yanked onto a distant external tangent.
    pub(crate) fn bezier_tangent_snap(&self, start: Point2, cursor: Point2) -> Option<Point2> {
        // Bezier targets: nearest-sample tangent ray through `start`.
        let mut best: Option<(f64, Point2)> = None;
        let tol = self.snap_tol_doc();
        for (sid, seg) in self.doc.all_segments() {
            if seg.kind != crate::core::document::SegmentKind::Bezier {
                continue;
            }
            let (h1, h2) = seg.bezier_handles();
            let (Some(p0), Some(c1), Some(c2), Some(p1)) = (
                self.doc.point(seg.start),
                h1.and_then(|id| self.doc.point(id)),
                h2.and_then(|id| self.doc.point(id)),
                self.doc.point(seg.end),
            ) else {
                continue;
            };
            // Cheap control-cage reject before the 48-eval nearest search
            // (runs per bezier per mousemove while pen-chaining).
            let (mut lox, mut hix) = (p0.x.min(p1.x), p0.x.max(p1.x));
            let (mut loy, mut hiy) = (p0.y.min(p1.y), p0.y.max(p1.y));
            for q in [c1, c2] {
                lox = lox.min(q.x);
                hix = hix.max(q.x);
                loy = loy.min(q.y);
                hiy = hiy.max(q.y);
            }
            if start.x < lox - tol || start.x > hix + tol || start.y < loy - tol || start.y > hiy + tol {
                continue;
            }
            // Only snap when starting ON the curve (chaining / G1).
            let (near, d, tan) = bezier::nearest_on_curve(p0, c1, c2, p1, start, 48);
            if d > tol {
                continue;
            }
            let len = pick::distance(start, cursor).max(tol);
            for side in [-1.0, 1.0] {
                let q = Point2::new(
                    near.x + tan.0 * len * side,
                    near.y + tan.1 * len * side,
                );
                let score = pick::distance(q, cursor);
                if best.map_or(true, |(s, _)| score < s) {
                    best = Some((score, q));
                }
            }
            let _ = sid;
        }
        best.map(|(_, q)| q)
    }
}
