# Parametric Design App — Folder Structure

This document defines the **architectural organization** of the project.

The folders describe major areas of responsibility. Individual Rust files should only be created when they are actually needed. The structure should evolve with the application rather than creating dozens of empty or speculative files in V1.

The most important rule is:

> **The design engine should be independent from the GPUI interface.**

---

# Project Structure

```text
parametric/
├── assets/
│
└── src/
    ├── main.rs
    ├── app.rs
    │
    ├── core/
    ├── editor/
    ├── tools/
    ├── renderer/
    ├── ui/
    ├── theme/
    ├── commands/
    └── persistence/
```

Not every folder needs to exist on day one. A folder should be introduced when its responsibilities become substantial enough to justify separation.

---

# `assets/`

Contains resources bundled with the application that aren't Rust source code.

Could eventually contain:

* Application icons
* UI icons
* Fonts
* Images
* Cursor graphics
* Other visual resources
* Default templates or assets

The folder should contain **static resources**, not application logic.

---

# `src/`

Contains all Rust source code.

The major folders separate the application into different systems rather than individual UI screens or random collections of functionality.

---

# `main.rs`

Not a folder, but intentionally kept extremely small.

Its only real responsibility is **starting the application**.

It should eventually be little more than:

```text
Start program
    ↓
Initialize application
```

Application logic, UI, geometry, and configuration should live elsewhere.

---

# `app.rs`

Application-level startup and orchestration.

This is where the different major systems can eventually be brought together.

Could contain things such as:

* Starting GPUI
* Creating the application
* Opening the main window
* Initializing global application state
* Registering global commands
* Initializing the theme
* Setting up application-wide services
* Connecting the document/editor/UI systems

`app.rs` should **coordinate systems**, not become a dumping ground for application logic.

---

# `core/`

The **actual parametric design engine**.

This is arguably the most important folder in the entire project.

It represents what the application fundamentally *is*, independently of how it is displayed.

Could contain:

### Geometry

Fundamental mathematical/vector geometry:

* Points
* Vectors
* Lines
* Curves
* Arcs
* Circles
* Beziers
* Transforms
* Intersections
* Bounding boxes
* Geometric calculations

### Shapes

Higher-level design objects:

* Paths
* Rectangles
* Ellipses
* Groups
* Compound shapes
* Other vector objects

### Constraints

The relationships that make the application parametric.

Potential categories:

* Geometric constraints
* Dimensional constraints
* Alignment constraints
* Spacing constraints
* Distribution constraints
* Symmetry constraints
* Proportional constraints
* Design-specific constraints

### Constraint solving

The system responsible for taking geometry + constraints and determining the resulting geometry.

### Document model

The underlying representation of a design document:

* Objects
* Hierarchy
* Layers
* Groups
* Constraints
* Metadata
* Document settings

---

## What should NOT be in `core/`

`core/` should not depend on GPUI.

It should not contain:

* Buttons
* Panels
* Mouse cursors
* UI styling
* GPUI elements
* Sidebar code
* Window management

Ideally, the core could eventually be tested or used without launching the application at all.

---

# `editor/`

Represents the **current editing session** rather than the permanent design itself.

The distinction is:

```text
core/
"What exists in the document?"

editor/
"What is the user currently doing with it?"
```

Could eventually contain:

* Current document state
* Current selection
* Active tool
* Camera/viewport state
* Zoom and pan
* Hover state
* Dragging/manipulation state
* Snapping state
* Guides
* Temporary editing state
* Editor-specific preferences
* Undo/redo integration
* Interaction state

For example, a rectangle existing in the document belongs conceptually to `core`.

The fact that the user is **currently dragging its bottom-right corner** belongs to `editor`.

---

# `tools/`

Contains the application's **interactive design tools**.

Tools translate user actions into operations on the editor and document.

Potential tools could include:

* Selection
* Pen/path creation
* Rectangle
* Ellipse
* Line
* Shape creation
* Transform
* Node editing
* Constraint creation
* Measurement
* Text
* Hand/pan
* Zoom

As V1 develops, only the tools that actually exist need to be implemented.

A tool should generally answer:

> "What should happen when the user interacts with the canvas using this tool?"

It should not contain the underlying mathematical implementation of the design system.

---

# `renderer/`

Responsible for turning the application's design state into something that can be visually displayed.

Could eventually contain:

* Vector geometry rendering
* Shape rendering
* Path rendering
* Selection visuals
* Transform handles
* Anchor points
* Constraint indicators
* Snapping indicators
* Guides
* Measurements
* Tool previews
* Canvas overlays
* Rendering optimizations

The renderer should primarily answer:

> "Given the document and current editor state, what should be drawn?"

It should not own the document or constraint system.

---

# `ui/`

Contains the **GPUI interface**.

This is everything the user directly interacts with outside of the actual canvas rendering system.

The UI could eventually contain areas such as:

### Application shell

The overall application layout:

* Titlebar
* Menus
* Main content area
* Global navigation
* Status bar

### Canvas UI

The GPUI component surrounding/hosting the design canvas.

Could handle:

* Canvas viewport
* Canvas interaction routing
* Zoom controls
* Canvas overlays
* Coordinate indicators

The actual geometric rendering can remain in `renderer/`.

### Sidebars

The application's left and right panels.

Potentially:

**Left side:**

* Layers
* Pages
* Assets
* Document structure
* Tools

**Right side:**

* Inspector
* Properties
* Constraints
* Appearance
* Transform controls

The exact organization can evolve.

### Toolbar

Could contain:

* Selection tools
* Drawing tools
* Shape tools
* Constraint tools
* View controls

### Menus

Application menus such as:

* File
* Edit
* View
* Object
* Arrange
* Constraints
* Help

### Reusable UI components

Generic components that are used throughout the application:

* Buttons
* Icon buttons
* Inputs
* Dropdowns
* Menus
* Tooltips
* Tabs
* Panels
* Dividers
* Toggles
* Sliders

These should only be extracted into reusable components when there is actually a reason to reuse them.

---

# `theme/`

Contains the application's visual design system.

This is where the Paper-inspired visual language should live.

Could eventually contain:

* Light theme
* Dark theme
* Color definitions
* Typography
* Spacing
* Border radii
* Shadows
* UI sizing
* Component styling constants
* Design tokens

The theme should use **semantic names** rather than tying the entire application to specific colors.

For example:

```text
background
surface
surface-hover
foreground
foreground-muted
border
accent
selection
danger
```

Rather than:

```text
beige
dark-beige
red
green
```

That way the entire visual identity can be changed without rewriting the UI.

---

# `commands/`

Contains operations that can be performed on the document/application.

This folder becomes particularly useful because this is a design application with **undo/redo**.

Potential commands:

* Create shape
* Delete shape
* Move shape
* Resize shape
* Add constraint
* Remove constraint
* Change property
* Group objects
* Ungroup objects
* Change layer
* Transform geometry

Commands can provide a consistent way to represent:

```text
User action
    ↓
Command
    ↓
Document change
    ↓
History
```

This can eventually become the foundation for reliable undo/redo.

This folder does **not** need to exist in V1 until the command/history architecture actually becomes useful.

---

# `persistence/`

Everything related to saving and loading documents.

Could eventually contain:

* Saving documents
* Loading documents
* Document serialization
* Document deserialization
* File format definitions
* Versioning
* Migration of older document formats
* Autosave
* Recovery files
* Import/export

For example, if the application eventually has its own file format:

```text
MyDesign.parametric
```

the logic for reading and writing that format belongs here.

The document itself belongs in `core/`; **how that document gets stored on disk belongs in `persistence/`.**

---

# How the Major Systems Relate

The intended architecture can roughly be thought of as:

```text
                       APPLICATION
                            │
                          app.rs
                            │
          ┌─────────────────┼─────────────────┐
          │                 │                 │
          ▼                 ▼                 ▼
         UI              EDITOR             CORE
          │                 │                 │
          │                 │          ┌──────┼──────┐
          │                 │          │      │      │
          │                 │       Geometry Shapes Constraints
          │                 │
          │                 ├──── Tools
          │                 ├──── Selection
          │                 ├──── Camera
          │                 └──── Snapping
          │
          └──────────── Renderer
```

And persistence/commands sit alongside these systems:

```text
                ┌──────────────┐
                │   Commands   │
                └──────┬───────┘
                       │
                       ▼
                    Document
                       ▲
                       │
                ┌──────┴───────┐
                │     Core     │
                └──────────────┘
                       │
                       ▼
                ┌──────────────┐
                │ Persistence  │
                └──────────────┘
```

---

# V1 Philosophy

The initial project should **not** implement the entire architecture immediately.

For V1, it is completely reasonable for the tree to start as something closer to:

```text
src/
├── main.rs
├── app.rs
│
├── core/
│
├── editor/
│
├── renderer/
│
├── ui/
│
└── theme/
```

Then, as functionality grows:

```text
core/
```

can naturally develop geometry, shapes, constraints, and document systems.

Likewise:

```text
ui/
```

can develop into the titlebar, canvas, sidebars, toolbar, inspector, and reusable components.

There is **no requirement that every conceptual system gets its own Rust file immediately**.

---

# Architectural Rules

## 1. Keep `main.rs` tiny

`main.rs` starts the program.

---

## 2. Keep `app.rs` as orchestration

`app.rs` initializes and connects systems.

It should not become the application's miscellaneous-code file.

---

## 3. Keep `core/` independent

The parametric design engine should not know about GPUI.

---

## 4. Separate permanent state from temporary editor state

```text
core/
    Document
    Geometry
    Shapes
    Constraints

editor/
    Selection
    Camera
    Interaction
    Snapping
```

---

## 5. Keep rendering separate from UI

The UI decides **where things are displayed and how the application is interacted with**.

The renderer determines **how the design itself is drawn**.

---

## 6. Don't create speculative files

Folders establish boundaries.

Files should be created when there is actual functionality that needs to be separated.

A V1 file tree should be small enough that you can understand the entire project at a glance.

---

## 7. Organize around systems, not individual screens

This isn't a collection of pages like a web application.

The major architectural concepts are:

```text
Document
Geometry
Constraints
Editor
Tools
Renderer
UI
```

Those should remain stable even as the interface evolves.

---

# Long-Term Direction

As the application grows, the architecture should allow it to evolve toward something like:

```text
                 ┌───────────────────────┐
                 │       GPUI UI         │
                 │                       │
                 │ Titlebar / Sidebars   │
                 │ Toolbar / Inspector   │
                 └───────────┬───────────┘
                             │
                             ▼
                 ┌───────────────────────┐
                 │        Editor         │
                 │                       │
                 │ Selection / Tools     │
                 │ Camera / Snapping     │
                 └───────────┬───────────┘
                             │
                             ▼
                 ┌───────────────────────┐
                 │         Core          │
                 │                       │
                 │ Geometry              │
                 │ Shapes                │
                 │ Constraints            │
                 │ Document              │
                 └───────────┬───────────┘
                             │
                    ┌────────┴────────┐
                    ▼                 ▼
              Persistence         Commands
```

The goal is not to predict every file the application will ever need.

The goal is to establish **clear boundaries now**, so that when the application becomes substantially more complicated, new code has an obvious place to live.
