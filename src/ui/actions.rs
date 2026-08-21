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
        // View
        ZoomIn,
        ZoomOut,
        ZoomToFit,
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
