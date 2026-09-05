# Redesigned Pen Tool

## 1. Overview

The Pen tool should be redesigned around one fundamental principle:

> **A path is continuously editable while it is being created.**

Traditional Pen tools treat drawing and editing as separate activities:

1. Place anchors.
2. Finish the path.
3. Switch to a node/selection tool.
4. Select anchors.
5. Manipulate Bézier handles.
6. Return to the Pen tool.
7. Continue drawing.

This creates unnecessary mode switching and makes editing an existing point during construction particularly frustrating.

The redesigned Pen tool treats an open path as a **live, continuously editable object**.

The user can simultaneously:

* Create new points.
* Move existing points.
* Insert points.
* Delete points.
* Bend segments.
* Modify curvature.
* Create asymmetric curves.
* Preserve smooth continuity.
* Convert points between smooth and corner behavior.
* Continue extending the path after any of these operations.

The underlying geometry may still use cubic Bézier curves, B-splines, or another mathematical representation. However, **the user should not be required to think in terms of Bézier handles**.

Handles become an advanced representation of the geometry rather than the primary interaction model.

---

# 2. Design Goals

The redesigned Pen tool should satisfy six primary goals.

### 2.1 Continuous editing

Editing an existing point must never force the user to leave Pen mode.

### 2.2 No accidental discontinuity

Moving or reshaping a smooth point should preserve continuity automatically.

### 2.3 Simple default interaction

A user should be able to create sophisticated curves without understanding Bézier handles.

### 2.4 Full geometric control

The simplified interaction must **not reduce the expressive power of the curve**.

Advanced users must still be able to independently control the equivalent of:

* Anchor position.
* Incoming tangent.
* Outgoing tangent.
* Tangent magnitude.
* Tangent direction.
* Continuity constraints.

### 2.5 Spatial interaction

The location where the user grabs a curve should determine what aspect of the curve is affected.

### 2.6 One object, one mode

The path being drawn is always an editable object. There should not be an artificial distinction between "drawing mode" and "editing mode."

---

# 3. Mental Model

The user should not think:

> Anchor → Handle → Handle → Anchor

Instead, the mental model should be:

> **Point → Curve → Point**

A point determines **where** the path passes.

A curve determines **how** the path travels between points.

The curve can be reshaped directly.

Internally, each point may still have tangent information:

```text
Point
├── position
├── incoming tangent
├── outgoing tangent
└── continuity mode
```

But this is implementation-level geometry.

The UI exposes these properties progressively through intuitive interactions.

---

# 4. Basic Path Creation

## 4.1 Creating a point

Clicking an empty area creates a point.

```text
●
```

Clicking elsewhere creates another point:

```text
●────────────────●
```

The path remains active and the newest point becomes the active endpoint.

---

## 4.2 Continuing the path

After creating:

```text
A────B────C
```

the user can continue creating:

```text
A────B────C────D────E
```

without entering another tool.

---

# 5. Moving Existing Points While Drawing

The defining feature of the redesigned Pen tool is that **existing points remain editable while the path is open**.

Suppose the user has:

```text
A────B────C────D
```

and realizes B is in the wrong location.

They simply grab B and drag it.

```text
A───B'────C────D
```

The path updates immediately.

The active endpoint remains D.

The user can immediately continue adding points.

There is no need to:

* Finish the path.
* Switch to a selection tool.
* Switch to a node tool.
* Re-enter Pen mode.

---

# 6. Curve Pulling

The primary curve-editing interaction is **curve pulling**.

Instead of manipulating visible Bézier handles, the user can grab the curve itself and pull it.

Straight segment:

```text
●────────────────────●
```

Pull upward:

```text
●────────╮
         ╰──────────●
```

Pull downward:

```text
●────────╯
         ╭──────────●
```

The user is manipulating the **shape of the path directly**, rather than manipulating the mathematical control points.

This should feel similar to pulling a flexible piece of material.

---

# 7. The Ctrl Curvature Interaction

The most important modifier interaction is:

> **Normal drag = position.
> Ctrl + drag = curvature.**

When an anchor is being dragged normally, the anchor moves.

```text
●────────────────●
                 ↓
                drag
```

The point follows the cursor and the connected curve updates.

If the user presses **Ctrl while holding the mouse**, the meaning of the drag changes.

The point becomes fixed.

Instead, the drag modifies its curvature/tangent information.

```text
●────────────────●
                 ↑
              Alt + drag
```

The anchor does not move.

Only the curve changes.

When Ctrl is released, the newly established curvature is preserved, and normal dragging resumes.

This allows a single uninterrupted gesture to perform multiple operations:

1. Grab point.
2. Move point.
3. Hold Alt.
4. Adjust curvature.
5. Release Alt.
6. Continue moving point.

Example:

```text
Initial:

●────────────────●


Move:

●────────────────●
                 ↘


Hold Ctrl:

●────────╮
         ╰────────●


Release Ctrl:

●────────╮
         ╰────────●
                  ↘

Continue moving anchor.
```

The curvature remains attached to the point.

---

# 8. Curvature Is Not Simply "Roundness"

The system should avoid treating curvature as a single scalar property.

A cubic curve fundamentally has independent control over both ends:

```text
P0 ── H0 ───────── H1 ── P1
●                         ●
```

Therefore, a full curve must be capable of representing:

* How the curve leaves P0.
* How the curve approaches P1.
* The direction of each tangent.
* The magnitude of each tangent.

The simplified Alt interaction should provide intuitive access to these degrees of freedom without eliminating them.

---

# 9. Segment-Based Curvature Editing

A particularly important extension is that **where the user grabs the segment determines which part of the curve they influence**.

Consider:

```text
●────────────────────●
```

The segment can be conceptually divided into regions.

```text
●───────┬────────┬───────●
       left     right
```

### Grab near the left

```text
●───↑────────────────●
    cursor
```

Primarily modifies how the curve **leaves the left endpoint**.

### Grab near the center

```text
●────────↑───────────●
         cursor
```

Primarily modifies the **overall bulge**.

### Grab near the right

```text
●────────────────↑───●
                 cursor
```

Primarily modifies how the curve **approaches the right endpoint**.

This creates a spatial equivalent of Bézier-handle editing.

The user does not need to know that they are manipulating tangents.

They simply grab the part of the curve they want to change.

---

# 10. Direction and Magnitude

Curvature editing should use two intuitive dimensions.

### Distance from the curve

Controls the **amount of curvature**.

Closer:

```text
●────────────────●
```

Farther:

```text
●───────╭───────●
        ╰───────
```

### Direction around the curve

Controls the **direction of the bulge**.

Pull upward:

```text
●───────╮
        ╰──────●
```

Pull downward:

```text
●───────╯
        ╭──────●
```

Therefore:

> **Direction = where the curve bends.
> Distance = how strongly it bends.**

This provides a natural 2D curvature-control system.

---

# 11. Symmetric vs. Asymmetric Curves

The basic Alt interaction should be designed to make smooth, symmetric curves easy.

However, the system must not force symmetry.

For example, a curve may need to leave the first point gently:

```text
●────────╮
         │
         ╰──────●
```

but approach the second point sharply:

```text
●───────╮
        ╰───────╯
                ●
```

The user should be able to achieve this by grabbing different regions of the segment.

Conceptually:

```text
●───[departure]────[center]────[arrival]───●
```

Each region provides different control.

This gives asymmetric control without requiring visible handles.

---

# 12. Handles Are Still Available

Handles should **not** be eliminated from the underlying system.

Instead, handles become an advanced interface.

The default UI:

```text
●───────────────●
```

Advanced curve UI:

```text
●──────○────────○──────●
       ↑        ↑
      H0       H1
```

The same curve is being represented.

Nothing about the geometry changes when switching between the interfaces.

The user is simply choosing between:

### Simplified mode

Direct manipulation of:

* Points.
* Segments.
* Curvature.

### Advanced mode

Direct manipulation of:

* Incoming handles.
* Outgoing handles.

This ensures the redesigned Pen tool is both approachable and technically complete.

---

# 13. Continuity

Continuity should be a first-class property of a point.

Each point can have a continuity mode.

## 13.1 Corner

Incoming and outgoing tangents are independent.

```text
──────●
      \
       ─────
```

The curve can form a sharp corner.

---

## 13.2 Smooth

The incoming and outgoing tangents share the same direction.

```text
──────╮
      ╰──────
```

Moving or reshaping the point automatically maintains smoothness.

---

## 13.3 Symmetric

Incoming and outgoing tangents share both:

* Direction.
* Magnitude.

This produces balanced curvature.

```text
──────╮
      ╰──────
```

---

## 13.4 Free

Incoming and outgoing tangents are completely independent.

This provides maximum control without requiring the point to become a corner.

---

# 14. Continuity Must Be Preserved Automatically

This is critical.

If the user has:

```text
A──────B──────C
```

and B is smooth, moving B should not accidentally create:

```text
A──────╮
       ●──────C
```

Instead, the tangent relationship should update automatically.

The system should maintain the required continuity constraint while the point moves.

A discontinuity should only occur when the user explicitly chooses to create one.

---

# 15. Inserting Points

Points should be inserted directly into the curve.

When hovering over a segment:

```text
A────────────────B
          ○
```

A preview point appears.

Clicking inserts it:

```text
A────────●───────B
```

The new point inherits the curve's existing tangent/continuity characteristics.

The original shape should remain visually unchanged as much as mathematically possible.

The user can then immediately move or reshape the new point.

No separate "Add Anchor Point" tool is necessary.

---

# 16. Deleting Points

Deleting a point should preserve the surrounding curve as intelligently as possible.

Before:

```text
A────B────C
```

Delete B:

```text
A──────────C
```

The system recalculates the connecting segment to minimize the visual change.

The user should not be forced to manually repair the curve after removing an anchor.

V1 scope: naive connect-and-preserve-continuity. Deleting a point joins
its neighbors directly and preserves their continuity modes; visible shape
change is accepted for now. Shape-preserving refitting is explicitly
deferred to a later version.

---

# 17. Direct Segment Manipulation

The path itself should always be interactive.

Hovering over a segment should identify it as a manipulable object.

For example:

```text
●──────────────●
       ↑
      hover
```

The segment may subtly highlight.

Dragging it bends the segment.

Ctrl-dragging it enters more precise curvature manipulation.

This makes the path itself the primary editing surface.

---

# 18. Contextual Controls

Controls should appear only when relevant.

Normal state:

```text
●────────────────●
```

Hovering:

```text
●───────╎────────●
        ↑
     segment
```

Selecting a point:

```text
●───────●────────●
        ↑
      selected
```

Ctrl curvature editing:

```text
●───────╭────────●
        ↑
    curvature
```

Advanced mode:

```text
●──────○────────○──────●
       ↑        ↑
     handle   handle
```

The interface should avoid permanently displaying handles and control clutter.

---

# 19. Gesture Summary

| Gesture                       | Behavior                             |
| ----------------------------- | ------------------------------------ |
| Click empty space             | Create point                         |
| Click existing point          | Select/grab point                    |
| Clean click on start anchor   | Close the path                       |
| Drag point                    | Move point                           |
| Hold Ctrl while dragging point| Freeze point and edit curvature      |
| Drag segment                  | Pull/bend curve                      |
| Ctrl + drag segment           | Precisely modify curvature           |
| Ctrl + Shift + drag           | Advanced/asymmetric curvature        |
| Shift + drag                  | Constrain gesture                    |
| Alt + drag                    | Disable snapping (unchanged)         |
| Grab near segment start       | Modify departure tangent             |
| Grab segment center           | Modify overall bulge                 |
| Grab near segment end         | Modify arrival tangent               |
| Hover segment                 | Preview insertion/control            |
| Click segment                 | Insert point                         |
| Delete selected point         | Remove point and repair curve (naive V1) |
| Double-click point            | Toggle point continuity              |
| Advanced Curve mode           | Expose exact Bézier controls         |

A clean click means press and release with almost no cursor movement;
pressing down on a point and moving always moves the point instead.

---

# 20. Modifier Philosophy

Modifiers should **temporarily change the meaning of the current gesture**, rather than switching the user into an entirely different tool.

For example:

```text
Drag
↓
Position
```

Hold Ctrl:

```text
Drag + Ctrl
↓
Curvature
```

Release Ctrl:

```text
Drag
↓
Position again
```

The user never loses the current object or operation.

This is preferable to requiring explicit tool changes.

---

# 21. Advanced Control Without Visual Handles

The system should maintain a separation between:

### Mathematical representation

Potentially:

```text
P0
T0
P1
T1
```

where:

* P = position
* T = tangent

and potentially tangent magnitude, continuity constraints, weighting, etc.

### Interaction representation

```text
Point
Curve
Cursor
Modifier
```

The user manipulates the geometry through natural spatial interactions.

This means the application can preserve the mathematical precision of Bézier curves while presenting a substantially simpler interface.

---

# 22. Optional Explicit Handle Mode

For professional users who need exact control, an "Advanced Curve" mode can expose handles.

Example:

```text
           H0
           ○
          /
         /
●───────/────────╲──────●
                  \
                   ○
                   H1
```

Dragging H0 or H1 gives direct tangent control.

Continuity constraints remain visible.

The user can therefore transition between:

> **Easy mode → Advanced mode**

without changing the underlying geometry.

---

# 23. Why This Is Better Than Traditional Pen Tools

Traditional Pen tools make the user manipulate the **construction mechanism**.

The redesigned Pen tool lets the user manipulate the **resulting geometry**.

Traditional:

> "Move this Bézier handle."

Redesigned:

> "Make this part of the curve bend more."

Traditional:

> "I need to switch to the node tool."

Redesigned:

> "Grab the point."

Traditional:

> "I accidentally broke the tangent."

Redesigned:

> "The smooth point stays smooth."

Traditional:

> "I need to add an anchor point."

Redesigned:

> "Click the path where I want another point."

---

# 24. Underlying Geometry

Locked for V1: cubic Bézier segments. Exact insertion (De Casteljau
subdivision) and predictable editing matter more here than keeping the
representation open, and the interaction model abstracts the handles away
without sacrificing their degrees of freedom.

```text
B(t) =
(1-t)^3 P0
+ 3(1-t)^2t P1
+ 3(1-t)t^2 P2
+ t^3 P3
```

where P0/P3 are endpoints and P1/P2 represent tangent control.

The interaction layer must not fundamentally depend on users seeing P1/P2
as handles. Other spline representations may still be supported later, but
V1 commits to cubics.

---

# 25. Suggested Internal Point Model

A point could conceptually contain:

```text
Point {
    position

    incoming {
        direction
        magnitude
    }

    outgoing {
        direction
        magnitude
    }

    continuity:
        corner
        smooth
        symmetric
        free
}
```

The UI then maps interactions onto these values.

For example:

### Drag point

```text
position ← cursor
```

### Ctrl + drag point

```text
incoming/outgoing curvature ← cursor
```

depending on continuity mode.

### Ctrl + drag near left side of segment

```text
outgoing tangent of left point
```

### Ctrl + drag near right side

```text
incoming tangent of right point
```

### Ctrl + drag center

```text
distributed adjustment across both tangents
```

### Ctrl + Shift + drag

```text
one side only ← cursor
```

Advanced/asymmetric mode: only the grabbed side's tangent follows the
cursor (departure side near the start, arrival side near the end, the
nearest side in the center); the other side stays frozen.

The exact interpolation can be tuned experimentally.

---

# 26. Important Principle: No Loss of Degrees of Freedom

The redesigned Pen tool must not become a "dumbed-down Pen tool."

The requirement is:

> **Every curve that can be created with traditional Bézier handles should remain creatable with the redesigned system.**

The difference is only how those degrees of freedom are accessed.

The default workflow should be easier.

The advanced workflow should remain exact.

---

# 27. Example Workflow

Suppose the user wants to draw a complex curved shape.

They begin:

```text
●──────●──────●
```

They realize the second point is misplaced.

They grab it:

```text
●───●'────●
```

The path remains active.

They want the curve to become rounder.

They hold Ctrl and drag:

```text
●────╮
     ╰────●
```

They release Ctrl.

The curvature remains.

They continue extending:

```text
●────╮
     ╰────●────●
```

They decide the final segment approaches the last point too sharply.

Instead of exposing handles, they grab near the end of the segment:

```text
●───────╮
        ↑────●
```

and reshape that region.

The rest of the curve remains mostly unaffected.

They insert another point:

```text
●────╮────●────●
      ↑
```

and continue.

At no point did they leave Pen mode.

---

# 28. The Core UX Philosophy

The redesigned Pen tool should feel less like operating a vector graphics editor and more like **physically shaping a flexible curve**.

The user should think:

> **"Grab the thing I want to change."**

Not:

> "Which tool, node, handle, modifier, or control point do I need?"

The application should infer the appropriate underlying geometry from the user's spatial interaction.

---

# 29. Final Interaction Model

The entire system can ultimately be summarized as:

```text
                 PATH
                   │
        ┌──────────┴──────────┐
        │                     │
       POINT                 SEGMENT
         │                     │
      Drag → Move          Drag → Bend
         │                     │
   Ctrl + Drag            Ctrl + Drag
         │                     │
    Change curvature      Change curvature
        │                     │
        └──────────┬──────────┘
                   │
          Advanced Curve Mode
                   │
                   ↓
             Exact handles
```

The default experience is simple.

The underlying system remains powerful.

And the user never has to sacrifice continuity or leave Pen mode simply because they decided that an earlier point needs to change.

---

# 30. Design Principle

> **The Pen tool should not be a tool for placing anchors. It should be a tool for continuously shaping a path.**

Anchors, handles, tangents, and Bézier control points are implementation concepts.

The user's interaction should be centered around:

**Place. Grab. Pull. Shape. Continue.**

---

# 31. Finishing and Closing Paths

An open path is finished by leaving it, not by a commit gesture: pressing
Esc or switching tools ends placement and leaves the path open.

A path is closed by a clean click (press and release with almost no cursor
movement) on its start anchor. Pressing down on any anchor and moving
always moves that anchor instead — so closing and moving can never be
confused. The pending link tracks the live anchor position throughout, so
a close target stays valid even after mid-draw edits.

---

# 32. Welding Rule

The Pen tool creates topology; constraints express intent.

When the cursor snap-locks onto existing geometry while drawing, the new
anchor **shares that point's ID**. There is only ever one point, so no
coincident constraint — and no chip — is created. Closing a path onto its
start anchor works the same way: one shared ID, a genuinely closed loop.

Merely crossing existing geometry mid-segment never welds; crossing is not
intent. Coincident constraints (and their chips) come only from deliberate
glue gestures: the drag-drop bond menu, the coincident tool, point-on-edge.
Those stay deletable, because ungluing them is a real operation.
