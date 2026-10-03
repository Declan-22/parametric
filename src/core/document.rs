use super::constraints::{Constraint, ConstraintKind, Dimension, ElementRef};
use super::geometry::{Point2, Rect};
use super::ids::{FillId, PointId, SegmentId};
use std::cell::RefCell;
use std::rc::Rc;

// The permanent design. "What exists in the document?"
// No GPUI types here — the engine is UI-independent.
//
// The document knows exactly four kinds of things: points, segments, fills,
// and constraints/dimensions between them. There are no composite shape
// objects — a "rectangle" is simply 4 points, 4 segments sharing endpoints,
// horizontal/vertical constraints, and one fill over the loop, emitted by
// the rectangle tool. Deleting any piece leaves the rest valid.

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DocSettings {
    pub show_grid: bool,
    pub snap_to_grid: bool,
    pub snap_to_objects: bool,
}

impl Default for DocSettings {
    fn default() -> Self {
        Self { show_grid: true, snap_to_grid: false, snap_to_objects: true }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Document {
    // View/snap toggles. Saved PER design; new designs seed from the
    // app-level "last used" prefs (Registry) instead of hard-coded values.
    pub settings: DocSettings,
    pub layers: Vec<Layer>,
    points: Arena<Point2>,
    segments: Arena<Segment>,
    fills: Arena<Fill>,
    // Geometric constraints (H/V/coincident) binding point pairs.
    pub constraints: Vec<Constraint>,
    // Dimensional measurements; a locked dimension doubles as a distance
    // constraint during edits.
    pub dimensions: Vec<Dimension>,
    /// Parametric edge treatments. The source segments remain identifiable so
    /// the treatment can be removed without losing the user's geometry.
    pub modifiers: Vec<crate::core::fillet::Fillet>,
    // Cached solver topology (fingerprint-gated). Runtime-only: never
    // persisted, and deliberately excluded from PartialEq below so a warm
    // cache never compares unequal to an identical cold document.
    topo: RefCell<Option<(u64, Rc<super::graph::Topology>)>>,
}

// Equality is structural content only. The topology cache is derived state:
// excluding it keeps history comparison (`snap != doc`) and save/load
// round-trips exact regardless of cache warmth. Keep this in sync with the
// fields above when adding new ones.
impl PartialEq for Document {
    fn eq(&self, other: &Self) -> bool {
        self.settings == other.settings
            && self.layers == other.layers
            && self.points == other.points
            && self.segments == other.segments
            && self.fills == other.fills
            && self.constraints == other.constraints
            && self.dimensions == other.dimensions
            && self.modifiers == other.modifiers
    }
}

// -- entities --

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SegmentKind {
    Line,
    // A measuring ruler: renders with procedural inch ticks and labels,
    // carries no constraints, no fill, no dims.
    Ruler,
    // Circular arc through start, ctrl (a point ON the arc), end. Becomes
    // a full circle when start/end share a Coincident constraint.
    Arc,
    // Cubic bezier through start -> end with two handle points.
    // Storage reuses the existing slots: `ctrl` is handle 1, `center` is
    // handle 2 (center is meaningless for beziers). No schema migration —
    // the DB persists both slots already. All point lifecycle paths
    // (remove/merge/sweep/element_points) already follow both slots.
    Bezier,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StrokeDash {
    Solid,
    Dashed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StrokeCap {
    Butt,
    Round,
    Square,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Segment {
    pub start: PointId,
    pub end: PointId,
    pub kind: SegmentKind,
    // Screen-px stroke rendered for standalone lines; 0 = invisible
    // geometry (rectangle edges, etc.).
    pub stroke_width: f64,
    pub stroke_color: u32,
    pub opacity: f32,
    pub dash: StrokeDash,
    pub cap: StrokeCap,
    // Arc control point (a REAL point on the arc) for kind == Arc.
    // None for lines/rulers. Endpoints + ctrl define the circumcircle.
    pub ctrl: Option<PointId>,
    // Circumcenter point for arcs — a REAL document point that stays
    // centered. None for non-arcs. Enables snapping/constraints/hover.
    pub center: Option<PointId>,
}

impl Segment {
    fn line(start: PointId, end: PointId) -> Self {
        Self { start, end, kind: SegmentKind::Line, stroke_width: 0., stroke_color: 0x202124, opacity: 1., dash: StrokeDash::Solid, cap: StrokeCap::Butt, ctrl: None, center: None }
    }

    fn with_kind(start: PointId, end: PointId, kind: SegmentKind) -> Self {
        Self { start, end, kind, stroke_width: 0., stroke_color: 0x202124, opacity: 1., dash: StrokeDash::Solid, cap: StrokeCap::Butt, ctrl: None, center: None }
    }

    /// Bezier handles: (handle1, handle2). None slots mean sharp/degenerate.
    pub fn bezier_handles(&self) -> (Option<PointId>, Option<PointId>) {
        if self.kind != SegmentKind::Bezier {
            return (None, None);
        }
        (self.ctrl, self.center)
    }

    pub fn is_curve(&self) -> bool {
        matches!(self.kind, SegmentKind::Arc | SegmentKind::Bezier)
    }
}

// A fill covers an ordered, closed loop of segments. Each segment must
// chain onto the previous one's endpoint.
#[derive(Clone, Debug, PartialEq)]
pub struct Fill {
    pub segments: Vec<SegmentId>,
    pub fill_color: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayerKind {
    // Geometry home: one island, one shape. Organize may split a body
    // that grew a second island.
    Body,
    // Structural intent: never auto-split, even with direct elements.
    Group,
}

impl LayerKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            LayerKind::Body => "body",
            LayerKind::Group => "group",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "body" => Some(LayerKind::Body),
            "group" => Some(LayerKind::Group),
            _ => None,
        }
    }
}

/// What a body layer holds, derived live from its contents — the icon
/// key for the layers panel. Never stored (contents are truth).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BodyShape {
    Rectangle,
    Circle,
    Arc,
    Line,
    Curve,
    Path,
    Shape,
    Points,
    Rulers,
    Empty,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Layer {
    pub id: u64,
    pub name: String,
    pub elements: Vec<ElementRef>,
    pub visible: bool,
    // Nesting: groups are just layers with children. A group's eye
    // toggles its whole subtree by derivation (no extra flags).
    pub parent: Option<u64>,
    // None = legacy flat layer, never organized. organize_bodies()
    // assigns every layer a kind (auto-runs on load while any None
    // survives); everything created afterwards carries one.
    pub kind: Option<LayerKind>,
}

impl Layer {
    pub fn retain_element(&mut self, el: ElementRef) {
        self.elements.retain(|&e| e != el);
    }
}

/// Base name for an island bucket during organize (dedupe numbering
/// happens at the call site). Mirrors `body_shape` naming so panel
/// icons and names agree.
fn shape_name_for(doc: &Document, segs: &[SegmentId], fills: &[ElementRef]) -> String {
    if !fills.is_empty() {
        if fills.len() == 1
            && let ElementRef::Fill(fid) = fills[0]
            && let Some(fill) = doc.fill(fid)
        {
            let kinds: Vec<SegmentKind> = fill
                .segments
                .iter()
                .filter_map(|s| doc.segment(*s).map(|g| g.kind))
                .collect();
            let pristine =
                segs.len() == fill.segments.len() && fill.segments.iter().all(|s| segs.contains(s));
            if pristine {
                if kinds.len() == 4 && kinds.iter().all(|k| *k == SegmentKind::Line) {
                    return "Rectangle".into();
                }
                if !kinds.is_empty() && kinds.iter().all(|k| *k == SegmentKind::Arc) {
                    return "Circle".into();
                }
            }
        }
        return "Shape".into();
    }
    if segs.len() == 1
        && let Some(seg) = doc.segment(segs[0])
    {
        return match seg.kind {
            SegmentKind::Line => "Line",
            SegmentKind::Arc => "Arc",
            SegmentKind::Bezier => "Curve",
            SegmentKind::Ruler => "Ruler",
        }
        .into();
    }
    let mut bezier = false;
    for &s in segs {
        if doc.segment(s).is_some_and(|g| g.kind == SegmentKind::Bezier) {
            bezier = true;
            break;
        }
    }
    if bezier { "Curve" } else { "Path" }.into()
}

// -- arena --

/// Generational slot storage: stable ids survive deletes without dangling
/// references. A stale id resolves to None instead of wrong data.
#[derive(Clone, Debug, PartialEq)]
struct Arena<T> {
    slots: Vec<Slot<T>>,
    free: Vec<u32>,
}

impl<T> Default for Arena<T> {
    fn default() -> Self {
        Self { slots: Vec::new(), free: Vec::new() }
    }
}

#[derive(Clone, Debug, PartialEq)]
struct Slot<T> {
    generation: u32,
    value: Option<T>,
}

impl<T> Arena<T> {
    /// Returns (idx, generation) for the freshly stored value.
    fn insert(&mut self, value: T) -> (u32, u32) {
        match self.free.pop() {
            Some(idx) => {
                let slot = &mut self.slots[idx as usize];
                slot.value = Some(value);
                (idx, slot.generation)
            }
            None => {
                self.slots.push(Slot { generation: 0, value: Some(value) });
                ((self.slots.len() - 1) as u32, 0)
            }
        }
    }

    fn remove(&mut self, idx: u32) -> Option<T> {
        let slot = self.slots.get_mut(idx as usize)?;
        let value = slot.value.take()?;
        slot.generation += 1;
        self.free.push(idx);
        Some(value)
    }

    fn generation(&self, idx: u32) -> Option<u32> {
        self.slots.get(idx as usize).map(|s| s.generation)
    }

    fn get(&self, id: (u32, u32)) -> Option<&T> {
        let slot = self.slots.get(id.0 as usize)?;
        if slot.generation != id.1 {
            return None;
        }
        slot.value.as_ref()
    }

    fn get_mut(&mut self, id: (u32, u32)) -> Option<&mut T> {
        let slot = self.slots.get_mut(id.0 as usize)?;
        if slot.generation != id.1 {
            return None;
        }
        slot.value.as_mut()
    }

    fn iter(&self) -> impl Iterator<Item = (u32, u32, &T)> {
        self.slots.iter().enumerate().filter_map(|(i, s)| s.value.as_ref().map(|v| (i as u32, s.generation, v)))
    }

    #[allow(dead_code)]
    fn len(&self) -> usize {
        self.slots.iter().filter(|s| s.value.is_some()).count()
    }

    #[allow(dead_code)]
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Places a value at an exact slot (raw restore path).
    fn set_at(&mut self, idx: u32, value: T) {
        if let Some(slot) = self.slots.get_mut(idx as usize) {
            slot.value = Some(value);
        }
    }
}

// -- Document --

impl Document {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn layer_mut(&mut self, id: u64) -> Option<&mut Layer> {
        self.layers.iter_mut().find(|l| l.id == id)
    }

    pub fn layer(&self, id: u64) -> Option<&Layer> {
        self.layers.iter().find(|l| l.id == id)
    }

    // -- layer tree (panel) --

    /// Child ids of a parent, in document order. `None` = top level.
    pub fn layer_children(&self, parent: Option<u64>) -> Vec<u64> {
        self.layers
            .iter()
            .filter(|l| l.parent == parent)
            .map(|l| l.id)
            .collect()
    }

    /// Visibility folds the ancestor chain: hiding a group hides its
    /// whole subtree. A dangling parent id counts as visible (never
    /// vanish geometry over a stale link).
    pub fn layer_effective_visible(&self, id: u64) -> bool {
        let mut cur = Some(id);
        let mut guard = 0;
        while let Some(lid) = cur {
            guard += 1;
            if guard > self.layers.len() + 1 {
                return true;
            }
            let Some(layer) = self.layer(lid) else { return true };
            if !layer.visible {
                return false;
            }
            cur = layer.parent;
        }
        true
    }

    /// An element paints and picks iff ANY containing layer is
    /// effectively visible. Orphans (no layer) stay visible — a stale
    /// membership must never nuke geometry.
    pub fn element_visible(&self, el: ElementRef) -> bool {
        let mut contained = false;
        for layer in &self.layers {
            if layer.elements.contains(&el) {
                contained = true;
                if self.layer_effective_visible(layer.id) {
                    return true;
                }
            }
        }
        !contained
    }

    pub fn rename_layer(&mut self, id: u64, name: &str) {
        let name = name.trim();
        if name.is_empty() {
            return;
        }
        if let Some(layer) = self.layer_mut(id) {
            layer.name = name.into();
        }
    }

    pub fn set_layer_visible(&mut self, id: u64, visible: bool) {
        if let Some(layer) = self.layer_mut(id) {
            layer.visible = visible;
        }
    }

    /// Moves a layer one step within its sibling list. Returns false at
    /// the ends (or for unknown ids).
    pub fn move_layer(&mut self, id: u64, dir: i8) -> bool {
        let parent = self.layer(id).and_then(|l| l.parent);
        let sibs: Vec<u64> = self.layer_children(parent);
        let Some(pos) = sibs.iter().position(|&s| s == id) else {
            return false;
        };
        let to = pos as i64 + dir as i64;
        if to < 0 || to >= sibs.len() as i64 {
            return false;
        }
        let (a, b) = (sibs[pos], sibs[to as usize]);
        let (pa, pb) = (
            self.layers.iter().position(|l| l.id == a),
            self.layers.iter().position(|l| l.id == b),
        );
        if let (Some(pa), Some(pb)) = (pa, pb) {
            self.layers.swap(pa, pb);
            true
        } else {
            false
        }
    }

    /// Indents under the previous sibling (becomes its last child).
    pub fn indent_layer(&mut self, id: u64) -> bool {
        let parent = self.layer(id).and_then(|l| l.parent);
        let sibs = self.layer_children(parent);
        let Some(pos) = sibs.iter().position(|&s| s == id) else {
            return false;
        };
        if pos == 0 {
            return false;
        }
        let above = sibs[pos - 1];
        // Move to the end of the new sibling list (predictable landing).
        if let Some(layer) = self.layer_mut(id) {
            layer.parent = Some(above);
        }
        let cur = self.layers.iter().position(|l| l.id == id);
        if let Some(cur) = cur {
            let rec = self.layers.remove(cur);
            self.layers.push(rec);
        }
        true
    }

    /// Outdents to a sibling right after its old parent.
    pub fn outdent_layer(&mut self, id: u64) -> bool {
        let parent = self.layer(id).and_then(|l| l.parent);
        let Some(pid) = parent else { return false };
        let grand = self.layer(pid).and_then(|l| l.parent);
        if let Some(layer) = self.layer_mut(id) {
            layer.parent = grand;
        }
        let cur = self.layers.iter().position(|l| l.id == id);
        let anchor = self.layers.iter().position(|l| l.id == pid);
        match (cur, anchor) {
            (Some(cur), Some(anchor)) => {
                let rec = self.layers.remove(cur);
                // Removal shifts the anchor when it sat before us.
                let at = if anchor < cur { anchor + 1 } else { anchor };
                let at = at.min(self.layers.len());
                self.layers.insert(at, rec);
                true
            }
            _ => false,
        }
    }

    /// What a body layer holds, derived live (contents are truth — a
    /// rectangle with a glued line stops being a rectangle). Groups
    /// render the folder regardless of this.
    pub fn body_shape(&self, id: u64) -> BodyShape {
        let Some(layer) = self.layer(id) else {
            return BodyShape::Empty;
        };
        let mut line = 0;
        let mut ruler = 0;
        let mut arc = 0;
        let mut bezier = 0;
        let mut fills = 0;
        let mut points = 0;
        for &el in &layer.elements {
            match el {
                ElementRef::Point(p) => {
                    if self.point(p).is_some() {
                        points += 1;
                    }
                }
                ElementRef::Segment(s) => match self.segment(s) {
                    Some(seg) => match seg.kind {
                        SegmentKind::Line => line += 1,
                        SegmentKind::Ruler => ruler += 1,
                        SegmentKind::Arc => arc += 1,
                        SegmentKind::Bezier => bezier += 1,
                    },
                    None => {}
                },
                ElementRef::Fill(f) => {
                    if self.fill(f).is_some() {
                        fills += 1;
                    }
                }
            }
        }
        let curves = line + arc + bezier;
        if curves == 0 && fills == 0 {
            if ruler > 0 {
                return BodyShape::Rulers;
            }
            return if points > 0 { BodyShape::Points } else { BodyShape::Empty };
        }
        if ruler > 0 && curves == 0 && fills == 0 {
            return BodyShape::Rulers;
        }
        if fills > 0 {
            // A pristine rect/circle reads as one; anything glued on (or
            // a mixed loop) is a generic shape.
            if fills == 1 && curves > 0 {
                if let Some(fid) = layer.elements.iter().find_map(|e| match e {
                    ElementRef::Fill(f) => self.fill(*f).map(|_| *f),
                    _ => None,
                }) {
                    let members: Vec<SegmentId> =
                        self.fill(fid).map(|f| f.segments.clone()).unwrap_or_default();
                    let island: Vec<SegmentId> = layer
                        .elements
                        .iter()
                        .filter_map(|e| match e {
                            ElementRef::Segment(s) => Some(*s),
                            _ => None,
                        })
                        .collect();
                    let pristine = island.len() == members.len()
                        && members.iter().all(|s| island.contains(s));
                    if pristine {
                        let kinds: Vec<SegmentKind> = members
                            .iter()
                            .filter_map(|s| self.segment(*s).map(|g| g.kind))
                            .collect();
                        if kinds.len() == 4 && kinds.iter().all(|k| *k == SegmentKind::Line) {
                            return BodyShape::Rectangle;
                        }
                        if !kinds.is_empty() && kinds.iter().all(|k| *k == SegmentKind::Arc) {
                            return BodyShape::Circle;
                        }
                        return BodyShape::Shape;
                    }
                }
            }
            return BodyShape::Shape;
        }
        if curves == 1 && ruler == 0 {
            if line == 1 {
                return BodyShape::Line;
            }
            if arc == 1 {
                return BodyShape::Arc;
            }
            return BodyShape::Curve;
        }
        if bezier > 0 {
            return BodyShape::Curve;
        }
        BodyShape::Path
    }

    /// True while any legacy (never-organized) layer survives.
    pub fn needs_organize(&self) -> bool {        self.layers.iter().any(|l| l.kind.is_none())
    }

    /// Partitions flat/multi-island layers into one body layer per
    /// island (shape-named), rulers into a Rulers body, lone points
    /// into a Points body. Groups pass through untouched; single-island
    /// bodies keep their records. Geometry ids are stable — only layer
    /// records are rewritten, so selection/constraints/dims survive.
    /// Returns the next free layer id after `start_id`.
    pub fn organize_bodies(&mut self, start_id: u64) -> u64 {
        let island_of: std::collections::HashMap<SegmentId, usize> = {
            let mut map = std::collections::HashMap::new();
            for (i, isl) in self.islands().iter().enumerate() {
                for &s in isl {
                    map.insert(s, i);
                }
            }
            map
        };
        // Points claimed by any segment (for lone-point detection).
        let mut claimed: std::collections::HashSet<PointId> = std::collections::HashSet::new();
        for (_, seg) in self.all_segments() {
            claimed.insert(seg.start);
            claimed.insert(seg.end);
            claimed.extend(seg.ctrl);
            claimed.extend(seg.center);
        }
        let mut taken: std::collections::HashSet<String> =
            self.layers.iter().map(|l| l.name.clone()).collect();
        let mut fresh = start_id.max(1);
        let mut out: Vec<Layer> = Vec::new();
        let sources = std::mem::take(&mut self.layers);
        for src in &sources {
            if src.kind == Some(LayerKind::Group) {
                out.push(src.clone());
                continue;
            }
            // Bucket the source layer's live segments.
            let mut ruler_segs: Vec<SegmentId> = Vec::new();
            let mut by_island: std::collections::HashMap<usize, Vec<SegmentId>> =
                std::collections::HashMap::new();
            let mut order: Vec<usize> = Vec::new();
            for &el in &src.elements {
                if let ElementRef::Segment(s) = el
                    && self.segment(s).is_some()
                {
                    let seg = self.segment(s).unwrap();
                    if seg.kind == SegmentKind::Ruler {
                        if !ruler_segs.contains(&s) {
                            ruler_segs.push(s);
                        }
                        continue;
                    }
                    // Orphan segments (no island — dead points) group
                    // under their own id so they survive visibly.
                    let key = island_of.get(&s).copied().unwrap_or(usize::MAX - (s.idx as usize));
                    if !by_island.contains_key(&key) {
                        order.push(key);
                    }
                    let bucket = by_island.entry(key).or_default();
                    if !bucket.contains(&s) {
                        bucket.push(s);
                    }
                }
            }
            let lone: Vec<PointId> = src
                .elements
                .iter()
                .filter_map(|e| match e {
                    ElementRef::Point(p) => Some(*p),
                    _ => None,
                })
                .filter(|p| self.point(*p).is_some() && !claimed.contains(p))
                .collect();
            let buckets = by_island.len() + if ruler_segs.is_empty() { 0 } else { 1 } + if lone.is_empty() { 0 } else { 1 };
            if buckets <= 1 && src.kind == Some(LayerKind::Body) {
                let mut keep = src.clone();
                keep.kind = Some(LayerKind::Body);
                out.push(keep);
                continue;
            }
            if buckets == 0 {
                // Empty legacy layer: keep the record so nothing the
                // user named vanishes; it just gains a kind.
                let mut keep = src.clone();
                keep.kind = Some(LayerKind::Body);
                out.push(keep);
                continue;
            }
            let mut emit = |base: &str, els: Vec<ElementRef>| {
                if els.is_empty() {
                    return;
                }
                let name = if !taken.contains(base) {
                    base.to_string()
                } else {
                    let mut n = 2;
                    while taken.contains(&format!("{base} {n}")) {
                        n += 1;
                    }
                    format!("{base} {n}")
                };
                taken.insert(name.clone());
                let id = fresh;
                fresh += 1;
                out.push(Layer {
                    id,
                    name,
                    elements: els,
                    visible: src.visible,
                    parent: src.parent,
                    kind: Some(LayerKind::Body),
                });
            };
            // Island bodies in first-appearance order, then points,
            // then rulers (construction aids last).
            for key in order {
                let segs = &by_island[&key];
                let fills: Vec<ElementRef> = self
                    .all_fills()
                    .filter(|(_, f)| {
                        f.segments.first().is_some_and(|s| {
                            island_of.get(s).copied().unwrap_or(usize::MAX - (s.idx as usize)) == key
                        })
                    })
                    .map(|(id, _)| ElementRef::Fill(id))
                    .filter(|el| src.elements.contains(el))
                    .collect();
                let mut els: Vec<ElementRef> =
                    segs.iter().map(|&s| ElementRef::Segment(s)).collect();
                els.extend(fills.iter().copied());
                for &s in segs {
                    if let Some(seg) = self.segment(s) {
                        for p in [seg.start, seg.end].into_iter().chain(seg.ctrl).chain(seg.center) {
                            let el = ElementRef::Point(p);
                            if !els.contains(&el) {
                                els.push(el);
                            }
                        }
                    }
                }
                emit(&shape_name_for(self, segs, &fills), els);
            }
            if !lone.is_empty() {
                emit("Points", lone.into_iter().map(ElementRef::Point).collect());
            }
            if !ruler_segs.is_empty() {
                let mut els: Vec<ElementRef> =
                    ruler_segs.into_iter().map(ElementRef::Segment).collect();
                let ends: Vec<ElementRef> = els
                    .iter()
                    .filter_map(|e| match e {
                        ElementRef::Segment(s) => self.segment(*s),
                        _ => None,
                    })
                    .flat_map(|seg| {
                        [ElementRef::Point(seg.start), ElementRef::Point(seg.end)]
                    })
                    .collect();
                for el in ends {
                    if !els.contains(&el) {
                        els.push(el);
                    }
                }
                emit("Rulers", els);
            }
        }
        self.layers = out;
        // Re-home any parent link broken by the rewrite (paranoia:
        // parents are inherited, so this only fires on stale input).
        let live: std::collections::HashSet<u64> = self.layers.iter().map(|l| l.id).collect();
        for layer in &mut self.layers {
            if let Some(p) = layer.parent
                && !live.contains(&p)
            {
                layer.parent = None;
            }
        }
        fresh
    }

    /// The layer plus all descendants, in document order.
    pub fn layer_subtree_ids(&self, id: u64) -> Vec<u64> {
        let mut out = vec![id];
        let mut i = 0;
        while i < out.len() {
            let pid = out[i];
            out.extend(self.layer_children(Some(pid)));
            i += 1;
        }
        out
    }

    /// Drops layer records (the subtree) WITHOUT touching geometry —
    /// the editor deletes elements first via `delete_element`.
    pub fn remove_layer_records(&mut self, id: u64) {
        let kill = self.layer_subtree_ids(id);
        self.layers.retain(|l| !kill.contains(&l.id));
        // A dangling parent link counts as visible, but don't leave
        // them: re-home orphans to the top level.
        let live: std::collections::HashSet<u64> = self.layers.iter().map(|l| l.id).collect();
        for layer in &mut self.layers {
            if let Some(p) = layer.parent
                && !live.contains(&p)
            {
                layer.parent = None;
            }
        }
    }

    // -- points --

    pub fn add_point(&mut self, pos: Point2) -> PointId {
        let (idx, generation) = self.points.insert(pos.clamped());
        PointId { idx, generation: generation }
    }

    pub fn point(&self, id: PointId) -> Option<Point2> {
        self.points.get((id.idx, id.generation)).copied()
    }

    pub fn move_point(&mut self, id: PointId, to: Point2) {
        if let Some(p) = self.points.get_mut((id.idx, id.generation)) {
            *p = to.clamped();
        }
    }

    /// Translates several points by delta in one pass.
    pub fn move_points(&mut self, ids: &[PointId], delta: Point2) {
        for &id in ids {
            if let Some(p) = self.points.get_mut((id.idx, id.generation)) {
                *p = Point2::new(p.x + delta.x, p.y + delta.y).clamped();
            }
        }
    }

    /// Removes a point WITHOUT cascading: no segments, constraints,
    /// dimensions, or fills are touched. Only call this after migrating
    /// every reference off the point (fillet subdivision consumes corners
    /// this way) — otherwise the document keeps dangling ids.
    pub fn remove_point_raw(&mut self, id: PointId) -> bool {
        self.detach_from_layers(ElementRef::Point(id));
        self.points.remove(id.idx).is_some()
    }

    /// Removes a point plus everything that depends on it: touching
    /// segments, fills through those segments, constraints, dimensions.
    pub fn remove_point(&mut self, id: PointId) -> bool {
        let dead: Vec<SegmentId> = self
            .segments
            .iter()
            .filter(|(_, _, s)| s.start == id || s.end == id || s.ctrl == Some(id) || s.center == Some(id))
            .map(|(idx, generation, _)| SegmentId { idx, generation: generation })
            .collect();
        for sid in dead {
            self.remove_segment(sid);
        }
        self.constraints.retain(|c| c.a != id && c.b != id);
        // Dimensions touching the deleted point die too; point references
        // inside line/angle targets invalidate those dimensions as well.
        self.dimensions.retain(|d| match d.target {
            super::constraints::DimTarget::Points { a, b, .. } => a != id && b != id,
            super::constraints::DimTarget::PointLine { p, .. } => p != id,
            _ => true,
        });
        self.detach_from_layers(ElementRef::Point(id));
        self.points.remove(id.idx).is_some()
    }

    // -- segments --

    pub fn add_segment(&mut self, start: PointId, end: PointId) -> SegmentId {
        let (idx, generation) = self.segments.insert(Segment::line(start, end));
        SegmentId { idx, generation }
    }

    /// Adds a segment with an explicit kind (ruler, future arcs).
    pub fn add_segment_kind(&mut self, start: PointId, end: PointId, kind: SegmentKind) -> SegmentId {
        let (idx, generation) = self.segments.insert(Segment::with_kind(start, end, kind));
        SegmentId { idx, generation }
    }

    /// Adds a standalone stroked line (the line tool's output).
    pub fn add_stroked_segment(&mut self, start: PointId, end: PointId, stroke_width: f64) -> SegmentId {
        let (idx, generation) = self.segments.insert(Segment {
            start,
            end,
            kind: SegmentKind::Line,
            stroke_width,
            stroke_color: 0x202124,
            opacity: 1.,
            dash: StrokeDash::Solid,
            cap: StrokeCap::Butt,
            ctrl: None,
            center: None,
        });
        SegmentId { idx, generation }
    }

    /// Adds a cubic bezier start -> end with two real handle points.
    /// Handles are free (no solver equations); only endpoints constrain.
    pub fn add_bezier_segment(
        &mut self,
        start: PointId,
        handle1: PointId,
        handle2: PointId,
        end: PointId,
    ) -> SegmentId {
        let (idx, generation) = self.segments.insert(Segment {
            start,
            end,
            kind: SegmentKind::Bezier,
            stroke_width: 1.0,
            stroke_color: 0x202124,
            opacity: 1.,
            dash: StrokeDash::Solid,
            cap: StrokeCap::Round,
            ctrl: Some(handle1),
            center: Some(handle2),
        });
        SegmentId { idx, generation }
    }

    /// Adds a circular arc through start -> ctrl -> end, with a real
    /// center point (kept in sync by the editor).
    pub fn add_arc_segment(
        &mut self,
        start: PointId,
        ctrl: PointId,
        end: PointId,
        center: PointId,
    ) -> SegmentId {
        let (idx, generation) = self.segments.insert(Segment {
            start,
            end,
            kind: SegmentKind::Arc,
            stroke_width: 0.,
            stroke_color: 0x202124,
            opacity: 1.,
            dash: StrokeDash::Solid,
            cap: StrokeCap::Round,
            ctrl: Some(ctrl),
            center: Some(center),
        });
        SegmentId { idx, generation }
    }

    pub fn segment(&self, id: SegmentId) -> Option<Segment> {
        self.segments.get((id.idx, id.generation)).copied()
    }

    pub fn segment_mut(&mut self, id: SegmentId) -> Option<&mut Segment> {
        self.segments.get_mut((id.idx, id.generation))
    }

    /// Resolved endpoint positions of a segment.
    pub fn segment_geom(&self, id: SegmentId) -> Option<(Point2, Point2)> {
        let s = self.segment(id)?;
        Some((self.point(s.start)?, self.point(s.end)?))
    }

    /// Removes a segment and any fill loops passing through it.
    pub fn remove_segment(&mut self, id: SegmentId) -> bool {
        for fid in self.fills_referencing(id) {
            self.remove_fill(fid);
        }
        self.detach_from_layers(ElementRef::Segment(id));
        self.segments.remove(id.idx).is_some()
    }

    fn fills_referencing(&self, sid: SegmentId) -> Vec<FillId> {
        self.fills
            .iter()
            .filter(|(_, _, f)| f.segments.contains(&sid))
            .map(|(idx, generation, _)| FillId { idx, generation: generation })
            .collect()
    }

    /// All segments in the document (id + payload).
    pub fn all_segments(&self) -> impl Iterator<Item = (SegmentId, Segment)> + '_ {
        self.segments.iter().map(|(idx, generation, s)| (SegmentId { idx, generation: generation }, *s))
    }

    /// Object islands (Object-mode contract): connected components of
    /// segments joined by SHARED ENDPOINT ids (start/end only) plus
    /// point-point Coincident constraints. A bare Coincident is an
    /// unexecuted merge — Clay glue-by-default never shares foreign ids,
    /// so without this glued paths would stay separate objects and J
    /// would change objecthood instead of just cleaning up. Point-on-
    /// segment (slide attachments), dims, orientation locks, and handles
    /// NEVER fuse — a shared pivot across parts stays two objects. Each
    /// island is one Object-mode object: hover highlights all its edges,
    /// click boxes it.
    pub fn islands(&self) -> Vec<Vec<SegmentId>> {
        use std::collections::HashMap;
        let segs: Vec<(SegmentId, Segment)> = self.all_segments().collect();
        let n = segs.len();
        let mut parent: Vec<usize> = (0..n).collect();
        fn find(parent: &mut [usize], mut x: usize) -> usize {
            while parent[x] != x {
                parent[x] = parent[parent[x]];
                x = parent[x];
            }
            x
        }
        let mut owner: HashMap<PointId, usize> = HashMap::new();
        for (i, (_, s)) in segs.iter().enumerate() {
            for end in [s.start, s.end] {
                if let Some(&j) = owner.get(&end) {
                    let (ri, rj) = (find(&mut parent, i), find(&mut parent, j));
                    if ri != rj {
                        parent[ri] = rj;
                    }
                } else {
                    owner.insert(end, i);
                }
            }
        }
        for c in &self.constraints {
            if c.kind != ConstraintKind::Coincident
                || c.point_on_segment.is_some()
                || c.tangent_segments.is_some()
                || c.a == c.b
            {
                continue;
            }
            if self.point(c.a).is_none() || self.point(c.b).is_none() {
                continue;
            }
            let mut idxs = Vec::new();
            for (i, (_, s)) in segs.iter().enumerate() {
                if s.start == c.a || s.end == c.a || s.start == c.b || s.end == c.b {
                    idxs.push(i);
                }
            }
            for w in idxs.windows(2) {
                let (ri, rj) = (find(&mut parent, w[0]), find(&mut parent, w[1]));
                if ri != rj {
                    parent[ri] = rj;
                }
            }
        }
        let mut groups: HashMap<usize, Vec<SegmentId>> = HashMap::new();
        for (i, (id, _)) in segs.iter().enumerate() {
            groups.entry(find(&mut parent, i)).or_default().push(*id);
        }
        groups.into_values().collect()
    }

    /// All points in the document (id + position).
    pub fn all_points(&self) -> impl Iterator<Item = (PointId, Point2)> + '_ {
        self.points.iter().map(|(idx, generation, p)| (PointId { idx, generation: generation }, *p))
    }

    /// All fills in the document (id + payload).
    pub fn all_fills(&self) -> impl Iterator<Item = (FillId, &Fill)> + '_ {
        self.fills.iter().map(|(idx, generation, f)| (FillId { idx, generation: generation }, f))
    }

    /// Shared solver topology, rebuilt only when the structural
    /// fingerprint changed. Drags (pure moves) always hit cache.
    pub fn topology(&self) -> Rc<super::graph::Topology> {
        super::graph::Topology::for_document(self, &mut self.topo.borrow_mut())
    }

    // -- fills --

    pub fn add_fill(&mut self, segments: Vec<SegmentId>) -> FillId {
        let (idx, generation) = self.fills.insert(Fill { segments, fill_color: 0xD9E2F3 });
        FillId { idx, generation: generation }
    }

    pub fn fill(&self, id: FillId) -> Option<&Fill> {
        self.fills.get((id.idx, id.generation))
    }

    pub fn fill_mut(&mut self, id: FillId) -> Option<&mut Fill> {
        self.fills.get_mut((id.idx, id.generation))
    }

    pub fn remove_fill(&mut self, id: FillId) -> bool {
        self.detach_from_layers(ElementRef::Fill(id));
        self.fills.remove(id.idx).is_some()
    }

    // -- layers --

    fn detach_from_layers(&mut self, el: ElementRef) {
        for layer in &mut self.layers {
            layer.retain_element(el);
        }
    }

    pub fn push_to_layer(&mut self, layer_id: u64, el: ElementRef) {
        if let Some(layer) = self.layer_mut(layer_id) {
            layer.elements.push(el);
        }
    }

    // -- derived geometry --

    /// Bounding rect of a set of points. Dead ids are skipped (a stale
    /// selection ref must never nuke the whole box).
    pub fn bounds_of_points<'a>(&self, ids: impl IntoIterator<Item = &'a PointId>) -> Option<Rect> {
        let pts: Vec<Point2> = ids.into_iter().filter_map(|id| self.point(*id)).collect();
        Self::bounds_of_positions(&pts)
    }

    fn bounds_of_positions(pts: &[Point2]) -> Option<Rect> {
        let mut acc: Option<Rect> = None;
        for p in pts {
            let r = Rect::from_points(*p, *p);
            acc = Some(match acc {
                Some(a) => a.union(&r),
                None => r,
            });
        }
        acc
    }

    /// Bounding rect of a closed fill loop.
    pub fn fill_bounds(&self, id: FillId) -> Option<Rect> {
        let f = self.fill(id)?;
        let pts: Vec<PointId> = f
            .segments
            .iter()
            .filter_map(|&sid| self.segment(sid))
            .flat_map(|s| [s.start, s.end])
            .collect();
        self.bounds_of_points(&pts)
    }

    /// All points referenced by an element (segment endpoints / loop corners).
    pub fn element_points(&self, el: ElementRef) -> Vec<PointId> {
        match el {
            ElementRef::Point(p) => vec![p],
            ElementRef::Segment(s) => match self.segment(s) {
                Some(seg) => {
                    // A completed arc is edited as two endpoints plus its
                    // radius. Its construction points remain internal so
                    // they cannot appear as a third/fourth user handle.
                    if seg.kind == SegmentKind::Arc {
                        vec![seg.start, seg.end]
                    } else {
                        let mut v = vec![seg.start, seg.end];
                        if let Some(c) = seg.ctrl {
                            v.push(c);
                        }
                        if let Some(c) = seg.center {
                            v.push(c);
                        }
                        v
                    }
                }
                None => Vec::new(),
            },
            ElementRef::Fill(f) => match self.fill(f) {
                Some(fill) => fill
                    .segments
                    .iter()
                    .filter_map(|&sid| self.segment(sid))
                    .flat_map(|s| [s.start, s.end])
                    .collect(),
                None => Vec::new(),
            },
        }
    }

    /// Deduplicated points of a set of elements.
    pub fn selection_points(&self, els: &[ElementRef]) -> Vec<PointId> {
        let mut out = Vec::new();
        for el in els {
            for p in self.element_points(*el) {
                if !out.contains(&p) {
                    out.push(p);
                }
            }
        }
        out
    }

    /// Selection grown to whole islands (Object-world box + Edit-button
    /// anchor + Move expansion): segments/points resolve to their island
    /// members (endpoints and construction slots both match); fills, lone
    /// points, and stale ids pass through untouched.
    pub fn island_elements(&self, selection: &[ElementRef]) -> Vec<ElementRef> {
        let islands = self.islands();
        let mut out: Vec<ElementRef> = Vec::new();
        let mut push = |el: ElementRef| {
            if !out.contains(&el) {
                out.push(el);
            }
        };
        for el in selection {
            match *el {
                ElementRef::Segment(sid) => match islands.iter().find(|v| v.contains(&sid)) {
                    Some(isl) => {
                        for s in isl {
                            push(ElementRef::Segment(*s));
                        }
                    }
                    None => push(*el),
                },
                ElementRef::Point(pid) => {
                    let mut found = false;
                    for isl in &islands {
                        let touches = isl.iter().any(|s| {
                            self.segment(*s).is_some_and(|seg| {
                                seg.start == pid
                                    || seg.end == pid
                                    || seg.ctrl == Some(pid)
                                    || seg.center == Some(pid)
                            })
                        });
                        if touches {
                            for s in isl {
                                push(ElementRef::Segment(*s));
                            }
                            found = true;
                            break;
                        }
                    }
                    if !found {
                        push(*el);
                    }
                }
                _ => push(*el),
            }
        }
        out
    }

    /// Exact bounding rect of one segment. Arcs bound the CURVE (center +
    /// radius + ctrl-branch sweep, cardinal extrema included) — endpoints
    /// alone miss the bulge, which is exactly the fillet body. Everything
    /// else bounds its referenced points (bezier handles conservatively
    /// included; the curve never leaves their hull).
    pub fn segment_bounds(&self, sid: SegmentId) -> Option<Rect> {
        let s = self.segment(sid)?;
        if s.kind != SegmentKind::Arc {
            return self.bounds_of_points(&self.element_points(ElementRef::Segment(sid)));
        }
        self.arc_curve_bounds(s)
    }

    /// Bounding rect of a set of elements (points / segments / fills).
    /// The box hugs visible geometry: arcs contribute their curve extent.
    pub fn elements_bounds(&self, els: &[ElementRef]) -> Option<Rect> {
        let mut acc: Option<Rect> = None;
        let mut push = |r: Option<Rect>| {
            if let Some(r) = r {
                acc = Some(match acc {
                    Some(a) => a.union(&r),
                    None => r,
                });
            }
        };
        for el in els {
            match *el {
                ElementRef::Point(p) => {
                    push(self.point(p).map(|q| Rect::from_points(q, q)));
                }
                ElementRef::Segment(s) => push(self.segment_bounds(s)),
                ElementRef::Fill(f) => {
                    if let Some(fill) = self.fill(f) {
                        for sid in &fill.segments {
                            push(self.segment_bounds(*sid));
                        }
                    }
                }
            }
        }
        acc
    }

    /// Exact bbox of an arc's curve. Center is authoritative when stored
    /// (refresh seats it exactly); otherwise the circumcircle of
    /// start/end/ctrl. Sweep branch comes from the ctrl side, mirroring
    /// the solver-side span convention. Degenerate arcs fall back to
    /// their construction points.
    fn arc_curve_bounds(&self, s: Segment) -> Option<Rect> {
        const TAU: f64 = std::f64::consts::TAU;
        const PI: f64 = std::f64::consts::PI;
        let (a, b) = (self.point(s.start)?, self.point(s.end)?);
        let c = self.point(s.ctrl?)?;
        let (o, r) = match s.center.and_then(|id| self.point(id)) {
            Some(o) => {
                let r = ((a.x - o.x).powi(2) + (a.y - o.y).powi(2)).sqrt();
                (o, r)
            }
            None => crate::editor::arc::circumcircle(a, b, c)?,
        };
        if !(r > 1e-9) {
            return self.bounds_of_points(&[s.start, s.end, s.ctrl?]);
        }
        let ang = |p: Point2| (p.y - o.y).atan2(p.x - o.x);
        let (a0, b0, c0) = (ang(a), ang(b), ang(c));
        let forward = (b0 - a0).rem_euclid(TAU);
        let c_forward = (c0 - a0).rem_euclid(TAU);
        let span = if c_forward <= forward { forward } else { forward - TAU };
        if span.abs() < 1e-12 {
            return self.bounds_of_points(&[s.start, s.end, s.ctrl?]);
        }
        let mut pts = vec![a, b];
        for k in 0..4 {
            let t0 = k as f64 * PI / 2.0;
            let d = (t0 - a0).rem_euclid(TAU);
            let on = if span > 0. {
                d <= span + 1e-9
            } else {
                d >= TAU + span - 1e-9
            };
            if on {
                let t = a0 + d;
                pts.push(Point2::new(o.x + r * t.cos(), o.y + r * t.sin()));
            }
        }
        Self::bounds_of_positions(&pts)
    }

    // -- constraints / dimensions --

    pub fn add_constraint(&mut self, kind: ConstraintKind, a: PointId, b: PointId) {
        let c = Constraint { kind, a, b, tangent_segments: None, point_on_segment: None };
        let unordered = matches!(kind, ConstraintKind::Coincident)
            && self.constraints.iter().any(|existing| {
                existing.kind == kind
                    && existing.point_on_segment.is_none()
                    && ((existing.a == a && existing.b == b)
                        || (existing.a == b && existing.b == a))
            });
        if !self.constraints.contains(&c) && !unordered {
            self.constraints.push(c);
        }
    }

    pub fn add_tangent_constraint(
        &mut self,
        line: SegmentId,
        arc: SegmentId,
        point: PointId,
    ) {
        // Tangency is a relation between the two owning segments, not between
        // whichever duplicate point ids happened to be selected at the
        // junction. Treat the pair as unordered and keep exactly one record.
        // This also cleans up older documents that already contain duplicate
        // tangent rows when another tangent is added.
        let mut found = false;
        self.constraints.retain(|existing| {
            let same_pair = existing.kind == ConstraintKind::Tangent
                && existing.tangent_segments.is_some_and(|(a, b)| {
                    (a == line && b == arc) || (a == arc && b == line)
                });
            if same_pair {
                if found {
                    false
                } else {
                    found = true;
                    true
                }
            } else {
                true
            }
        });
        if found {
            return;
        }
        let c = Constraint {
            kind: ConstraintKind::Tangent,
            a: point,
            b: point,
            tangent_segments: Some((line, arc)),
            point_on_segment: None,
        };
        if !self.constraints.contains(&c) {
            self.constraints.push(c);
        }
    }

    pub fn add_parallel_constraint(&mut self, first: SegmentId, second: SegmentId) {
        let (Some(a), Some(b)) = (self.segment(first), self.segment(second)) else { return };
        let c = Constraint {
            kind: ConstraintKind::Parallel,
            a: a.start,
            b: b.start,
            // Reuse the owning-segment pair already carried by tangent
            // constraints; the kind determines how the pair is interpreted.
            tangent_segments: Some((first, second)),
            point_on_segment: None,
        };
        if !self.constraints.contains(&c) {
            self.constraints.push(c);
        }
    }

    pub fn add_perpendicular_constraint(&mut self, first: SegmentId, second: SegmentId) {
        let (Some(a), Some(b)) = (self.segment(first), self.segment(second)) else { return };
        let c = Constraint {
            kind: ConstraintKind::Perpendicular,
            a: a.start,
            b: b.start,
            tangent_segments: Some((first, second)),
            point_on_segment: None,
        };
        if !self.constraints.contains(&c) {
            self.constraints.push(c);
        }
    }

    /// Constrains `point` to a new/selected point that lies on `segment`.
    /// Keeping the edge point as a real point makes the relationship
    /// persistent and lets the solver preserve it when either object moves.
    pub fn add_point_on_segment_constraint(
        &mut self,
        point: PointId,
        edge_point: PointId,
        segment: SegmentId,
    ) {
        let c = Constraint {
            kind: ConstraintKind::Coincident,
            a: point,
            b: edge_point,
            tangent_segments: None,
            point_on_segment: Some(segment),
        };
        if !self.constraints.contains(&c) {
            self.constraints.push(c);
        }
    }

    /// Fuses `drop` into `keep`: every reference to `drop` (segments,
    /// constraints, dimensions, layer listings) is rewritten to `keep`,
    /// then `drop` is deleted. Degenerate self-referential constraints and
    /// dimensions are dropped.
    pub fn merge_point(&mut self, keep: PointId, drop: PointId) {
        if keep == drop || self.point(drop).is_none() {
            return;
        }
        for slot in &mut self.segments.slots {
            if let Some(s) = &mut slot.value {
                if s.start == drop {
                    s.start = keep;
                }
                if s.end == drop {
                    s.end = keep;
                }
                if s.ctrl == Some(drop) {
                    s.ctrl = Some(keep);
                }
                if s.center == Some(drop) {
                    s.center = Some(keep);
                }
            }
        }
        for c in &mut self.constraints {
            if c.a == drop {
                c.a = keep;
            }
            if c.b == drop {
                c.b = keep;
            }
        }
        for d in &mut self.dimensions {
            match &mut d.target {
                super::constraints::DimTarget::Points { a, b, .. } => {
                    if *a == drop {
                        *a = keep;
                    }
                    if *b == drop {
                        *b = keep;
                    }
                }
                super::constraints::DimTarget::PointLine { p, .. } => {
                    if *p == drop {
                        *p = keep;
                    }
                }
                _ => {}
            }
        }
        self.constraints.retain(|c| c.a != c.b);
        self.dimensions.retain(|d| match d.target {
            super::constraints::DimTarget::Points { a, b, .. } => a != b,
            _ => true,
        });
        self.detach_from_layers(ElementRef::Point(drop));
        self.points.remove(drop.idx);
    }

    pub fn add_dimension(&mut self, dim: Dimension) {
        self.dimensions.push(dim);
    }

    pub fn add_modifier(&mut self, modifier: crate::core::fillet::Fillet) { self.modifiers.push(modifier); }

    pub fn remove_modifier(&mut self, index: usize) -> Option<crate::core::fillet::Fillet> {
        (index < self.modifiers.len()).then(|| self.modifiers.remove(index))
    }

    /// Removes dimensions whose referenced geometry no longer exists, then
    /// points that nothing references anymore (no segment endpoints, no
    /// constraint, no dimension). Called after deletions so a deleted shape
    /// takes its dimensions and corner points with it.
    pub fn sweep_orphans(&mut self) {
        use super::constraints::DimTarget;
        // Drop dims whose referenced geometry vanished.
        let dims = self.dimensions.clone();
        self.dimensions = dims
            .into_iter()
            .filter(|d| match &d.target {
                DimTarget::Points { a, b, .. } => {
                    self.point(*a).is_some() && self.point(*b).is_some()
                }
                DimTarget::PointLine { p, line } => {
                    self.point(*p).is_some() && self.segment(*line).is_some()
                }
                DimTarget::Lines { a, b }
                | DimTarget::Angle { a, b }
                | DimTarget::EdgeMid { a, b, .. } => {
                    self.segment(*a).is_some() && self.segment(*b).is_some()
                }
                DimTarget::Radius { seg } | DimTarget::CurveLength { seg } => {
                    self.segment(*seg).is_some()
                }
            })
            .collect();
        let dims = self.dimensions.clone();
        // A point survives only while something references it. Note a
        // dimension counts as a reference: dims pin their geometry.
        let referenced = |id: PointId| -> bool {
            self.all_segments().any(|(_, s)| {
                s.start == id || s.end == id || s.ctrl == Some(id) || s.center == Some(id)
            }) || self.constraints.iter().any(|c| c.a == id || c.b == id)
                || dims.iter().any(|d| match &d.target {
                    DimTarget::Points { a, b, .. } => *a == id || *b == id,
                    DimTarget::PointLine { p, .. } => *p == id,
                    _ => false,
                })
        };
        let dead: Vec<PointId> = self
            .all_points()
            .filter(|(id, _)| !referenced(*id))
            .map(|(id, _)| id)
            .collect();
        for id in dead {
            self.points.remove(id.idx);
            self.detach_from_layers(super::constraints::ElementRef::Point(id));
        }
    }

    // -- raw inserts (persistence round-trips ids exactly) --

    pub fn insert_point_with_id(&mut self, id: PointId, pos: Point2) {
        Self::reserve(&mut self.points, id.idx, id.generation);
        self.points.set_at(id.idx, pos.clamped());
    }

    pub fn insert_segment_with_id(
        &mut self,
        id: SegmentId,
        start: PointId,
        end: PointId,
        kind: SegmentKind,
        stroke_width: f64,
        ctrl: Option<PointId>,
        center: Option<PointId>,
    ) {
        Self::reserve(&mut self.segments, id.idx, id.generation);
        self.segments.set_at(id.idx, Segment { start, end, kind, stroke_width, stroke_color: 0x202124, opacity: 1., dash: StrokeDash::Solid, cap: StrokeCap::Butt, ctrl, center });
    }

    pub fn insert_fill_with_id(&mut self, id: FillId, segments: Vec<SegmentId>) {
        Self::reserve(&mut self.fills, id.idx, id.generation);
        self.fills.set_at(id.idx, Fill { segments, fill_color: 0xD9E2F3 });
    }

    fn reserve<T>(arena: &mut Arena<T>, idx: u32, generation: u32) {
        while arena.slots.len() <= idx as usize {
            arena.slots.push(Slot { generation: 0, value: None });
        }
        arena.slots[idx as usize].generation = generation;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree_doc() -> Document {
        // G (group) -> [A, B]; C top-level.
        let mut doc = Document::new();
        doc.layers.push(Layer { id: 1, name: "G".into(), elements: vec![], visible: true, parent: None, kind: Some(LayerKind::Group) });
        doc.layers.push(Layer { id: 2, name: "A".into(), elements: vec![], visible: true, parent: Some(1), kind: Some(LayerKind::Body) });
        doc.layers.push(Layer { id: 3, name: "B".into(), elements: vec![], visible: true, parent: Some(1), kind: Some(LayerKind::Body) });
        doc.layers.push(Layer { id: 4, name: "C".into(), elements: vec![], visible: true, parent: None, kind: Some(LayerKind::Body) });
        doc
    }

    #[test]
    fn layer_visibility_folds_the_ancestor_chain() {
        let mut doc = tree_doc();
        assert!(doc.layer_effective_visible(2));
        doc.set_layer_visible(1, false);
        assert!(!doc.layer_effective_visible(2));
        assert!(!doc.layer_effective_visible(3));
        assert!(doc.layer_effective_visible(4));
        doc.set_layer_visible(1, true);
        doc.set_layer_visible(2, false);
        assert!(!doc.layer_effective_visible(2));
        assert!(doc.layer_effective_visible(3));
    }

    #[test]
    fn layer_reorder_indent_outdent() {
        let mut doc = tree_doc();
        assert!(doc.move_layer(2, 1));
        assert_eq!(doc.layer_children(Some(1)), vec![3, 2]);
        assert!(!doc.move_layer(2, 1));
        assert!(doc.indent_layer(4));
        assert_eq!(doc.layer(4).unwrap().parent, Some(1));
        assert!(doc.outdent_layer(4));
        assert_eq!(doc.layer(4).unwrap().parent, None);
        assert!(!doc.outdent_layer(4));
        assert!(!doc.indent_layer(3));
    }

    #[test]
    fn element_visibility_follows_any_visible_home() {
        let mut doc = tree_doc();
        let p = doc.add_point(Point2::new(0., 0.));
        // Orphan: visible (never nuke geometry over stale membership).
        assert!(doc.element_visible(ElementRef::Point(p)));
        doc.push_to_layer(2, ElementRef::Point(p));
        assert!(doc.element_visible(ElementRef::Point(p)));
        doc.set_layer_visible(1, false);
        assert!(!doc.element_visible(ElementRef::Point(p)));
        doc.push_to_layer(4, ElementRef::Point(p));
        assert!(doc.element_visible(ElementRef::Point(p)));
    }

    #[test]
    fn organize_splits_flat_layer_into_shape_named_bodies() {
        use crate::core::document::SegmentKind;
        let mut doc = Document::new();
        doc.layers.push(Layer { id: 1, name: "Layer 1".into(), elements: vec![], visible: true, parent: None, kind: None });
        // Rectangle: 4-line fill loop.
        let c = [
            doc.add_point(Point2::new(0., 0.)),
            doc.add_point(Point2::new(10., 0.)),
            doc.add_point(Point2::new(10., 10.)),
            doc.add_point(Point2::new(0., 10.)),
        ];
        let rsegs = [
            doc.add_segment(c[0], c[1]),
            doc.add_segment(c[1], c[2]),
            doc.add_segment(c[2], c[3]),
            doc.add_segment(c[3], c[0]),
        ];
        let fill = doc.add_fill(rsegs.to_vec());
        // Lone line + ruler + lone point.
        let a = doc.add_point(Point2::new(50., 50.));
        let b = doc.add_point(Point2::new(70., 50.));
        let line = doc.add_segment(a, b);
        let r1 = doc.add_point(Point2::new(50., 70.));
        let r2 = doc.add_point(Point2::new(90., 70.));
        let ruler = doc.add_segment(r1, r2);
        doc.segment_mut(ruler).unwrap().kind = SegmentKind::Ruler;
        let lone = doc.add_point(Point2::new(200., 200.));
        for el in rsegs
            .iter()
            .map(|&s| ElementRef::Segment(s))
            .chain([
                ElementRef::Fill(fill),
                ElementRef::Segment(line),
                ElementRef::Segment(ruler),
                ElementRef::Point(lone),
            ])
        {
            doc.push_to_layer(1, el);
        }
        assert!(doc.needs_organize());
        let next = doc.organize_bodies(2);
        assert!(!doc.needs_organize());
        assert_eq!(next, 6);
        let names: Vec<&str> = doc.layers.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(names, vec!["Rectangle", "Line", "Points", "Rulers"]);
        let shape_of = |n: &str| {
            let id = doc.layers.iter().find(|l| l.name == n).unwrap().id;
            doc.body_shape(id)
        };
        assert_eq!(shape_of("Rectangle"), BodyShape::Rectangle);
        assert_eq!(shape_of("Line"), BodyShape::Line);
        assert_eq!(shape_of("Points"), BodyShape::Points);
        assert_eq!(shape_of("Rulers"), BodyShape::Rulers);
        // Rectangle body owns its loop + fill + corners.
        let rect = doc.layers.iter().find(|l| l.name == "Rectangle").unwrap();
        assert_eq!(rect.elements.len(), 4 + 1 + 4);
        // Geometry ids are stable (only records were rewritten).
        assert!(doc.segment(line).is_some());
        assert!(doc.fill(fill).is_some());
    }

    #[test]
    fn organize_keeps_groups_and_single_bodies() {
        let mut doc = Document::new();
        doc.layers.push(Layer { id: 1, name: "G".into(), elements: vec![], visible: true, parent: None, kind: Some(LayerKind::Group) });
        let a = doc.add_point(Point2::new(0., 0.));
        let b = doc.add_point(Point2::new(5., 0.));
        let s = doc.add_segment(a, b);
        doc.layers.push(Layer { id: 2, name: "Kept".into(), elements: vec![ElementRef::Segment(s), ElementRef::Point(a), ElementRef::Point(b)], visible: false, parent: Some(1), kind: Some(LayerKind::Body) });
        let next = doc.organize_bodies(3);
        assert_eq!(next, 3);
        assert_eq!(doc.layers.len(), 2);
        let kept = doc.layer(2).unwrap();
        assert_eq!(kept.name, "Kept");
        assert!(!kept.visible);
        assert_eq!(kept.parent, Some(1));
    }

    #[test]
    fn islands_fuse_shared_endpoints_and_point_coincident() {
        use crate::core::constraints::{DimMode, DimTarget, Dimension};
        let mut doc = Document::new();
        // Chain A-B-C (shared ids) + lone segment + point-coincident
        // pair (fuses: an unexecuted merge) + point-on-segment pair and
        // dimension-linked pair (stay separate: slide attachments and
        // measurements are not merges).
        let a = doc.add_point(Point2::new(0., 0.));
        let b = doc.add_point(Point2::new(10., 0.));
        let c = doc.add_point(Point2::new(20., 0.));
        let d = doc.add_point(Point2::new(100., 100.));
        let e = doc.add_point(Point2::new(110., 100.));
        let p = doc.add_point(Point2::new(200., 0.));
        let q = doc.add_point(Point2::new(210., 0.));
        let r = doc.add_point(Point2::new(200., 0.));
        let s = doc.add_point(Point2::new(210., 0.));
        let s1 = doc.add_segment(a, b);
        let s2 = doc.add_segment(b, c);
        let lone = doc.add_segment(d, e);
        let c1 = doc.add_segment(p, q);
        let c2 = doc.add_segment(r, s);
        doc.add_constraint(ConstraintKind::Coincident, p, r);
        let t = doc.add_point(Point2::new(300., 0.));
        let u = doc.add_point(Point2::new(290., 0.));
        let v = doc.add_point(Point2::new(310., 0.));
        let w = doc.add_point(Point2::new(320., 5.));
        let g1 = doc.add_segment(t, u);
        let g2 = doc.add_segment(v, w);
        doc.add_point_on_segment_constraint(t, t, g2);
        let x1 = doc.add_point(Point2::new(400., 0.));
        let y1 = doc.add_point(Point2::new(410., 0.));
        let x2 = doc.add_point(Point2::new(500., 0.));
        let y2 = doc.add_point(Point2::new(510., 0.));
        let h1 = doc.add_segment(x1, y1);
        let h2 = doc.add_segment(x2, y2);
        doc.dimensions.push(Dimension {
            target: DimTarget::Points { a: x1, b: x2, mode: DimMode::Aligned },
            value: 100.,
            offset: 0.,
            slide: 0.,
            sweep: 0.,
        });
        let mut islands = doc.islands();
        for isl in islands.iter_mut() {
            isl.sort_by_key(|id| (id.idx, id.generation));
        }
        islands.sort_by_key(|isl| (isl.len(), isl[0].idx));
        assert_eq!(islands.len(), 7, "chain + lone + fused pair + 4 singles");
        assert!(islands.iter().any(|isl| isl == &vec![s1, s2]));
        assert!(islands.iter().any(|isl| isl == &vec![lone]));
        assert!(islands.iter().any(|isl| isl == &vec![c1, c2]));
        assert!(islands.iter().any(|isl| isl == &vec![g1]));
        assert!(islands.iter().any(|isl| isl == &vec![g2]));
        assert!(islands.iter().any(|isl| isl == &vec![h1]));
        assert!(islands.iter().any(|isl| isl == &vec![h2]));
    }

    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6
    }

    /// Arc bounds hug the CURVE, not the chord: a 45°→135° arc peaks at
    /// y=10 while its endpoints sit at y≈7.07.
    #[test]
    fn arc_bounds_cover_bulge() {
        let mut doc = Document::new();
        let o = doc.add_point(Point2::new(0., 0.));
        let a = doc.add_point(Point2::new(7.0710678, 7.0710678));
        let c = doc.add_point(Point2::new(0., 10.));
        let b = doc.add_point(Point2::new(-7.0710678, 7.0710678));
        let arc = doc.add_arc_segment(a, c, b, o);
        let bb = doc.segment_bounds(arc).expect("arc has bounds");
        assert!(approx(bb.origin.x, -7.0710678), "min.x={}", bb.origin.x);
        assert!(approx(bb.origin.y, 7.0710678), "min.y={}", bb.origin.y);
        assert!(approx(bb.origin.x + bb.size.w, 7.0710678));
        assert!(approx(bb.origin.y + bb.size.h, 10.), "max.y={}", bb.origin.y + bb.size.h);
    }

    /// Near-closed spans: endpoints cluster at 0° but the curve sweeps
    /// the whole circle — the box must too.
    #[test]
    fn arc_bounds_near_closed_span() {
        let mut doc = Document::new();
        let o = doc.add_point(Point2::new(0., 0.));
        let a = doc.add_point(Point2::new(10., 0.));
        let c = doc.add_point(Point2::new(-10., 0.));
        let b = doc.add_point(Point2::new(9.8480775, -1.7364818));
        let arc = doc.add_arc_segment(a, c, b, o);
        let bb = doc.segment_bounds(arc).expect("arc has bounds");
        assert!(approx(bb.origin.x, -10.), "min.x={}", bb.origin.x);
        assert!(approx(bb.origin.y, -10.), "min.y={}", bb.origin.y);
        assert!(approx(bb.origin.x + bb.size.w, 10.));
        assert!(approx(bb.origin.y + bb.size.h, 10.));
    }
}
