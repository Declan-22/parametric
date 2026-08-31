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
        // Snap-bond choice menu (points dropped onto points)
        BondCoincident,
        BondCombinePoints,
        BondDismiss,
        // Tools
        ToolMove,
        ToolPan,
        ToolDimension,
        ToolRuler,
        ToolLine,
        ToolRectangle,
        ToolCircle,
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
