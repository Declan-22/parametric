use gpui::actions;

actions!(
    parametric,
    [
        Quit,
        ToggleTheme,
        // File
        NewDocument,
        OpenDocument,
        SaveDocument,
        SaveDocumentAs,
        ExportDocument,
        // Edit
        Undo,
        Redo,
        Cut,
        Copy,
        Paste,
        DeleteSelection,
        SelectAll,
        // Global escape cascade.
        BondDismiss,
        // Tools
        ToolMove,
        ToolPan,
        ToolDimension,
        ToolRuler,
        ToolLine,
        ToolRectangle,
        ToolCircle,
        ToolPen,
        PenLine,
        PenBezier,
        PenArc,
        DimWidth,
        DimHeight,
        DimDisplacement,
        DimDistance,
        // View
        ZoomIn,
        ZoomOut,
        ZoomToFit,
        ZoomToSelection,
        // Arrange
        BringToFront,
        BringForward,
        SendBackward,
        SendToBack,
        // Help
        ShowKeybindings,
        About,
    ]
);
