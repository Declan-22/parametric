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
        // Object
        GroupObjects,
        UngroupObjects,
        TransformObjects,
        // Arrange
        BringToFront,
        BringForward,
        SendBackward,
        SendToBack,
        // Constraints
        AddConstraint,
        ToggleConstraintsPanel,
        // Help
        ShowKeybindings,
        About,
    ]
);
