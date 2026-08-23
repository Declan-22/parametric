use std::path::PathBuf;

// Filesystem locations for user-facing files.
//
// Documents are the user's own files, so they live in Documents\Parametric
// (like how design tools default to a visible, synced-friendly folder).
// App-internal state (prefs, recents) will use %APPDATA%\Parametric later.

pub fn documents_dir() -> std::io::Result<PathBuf> {
    let base = dirs::document_dir()
        .ok_or_else(|| std::io::Error::other("cannot resolve user Documents folder"))?;
    let dir = base.join("Parametric");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

// App-internal state: registry DB, prefs, thumbnails.
pub fn app_data_dir() -> std::io::Result<PathBuf> {
    let base = dirs::data_dir()
        .ok_or_else(|| std::io::Error::other("cannot resolve app data folder"))?;
    let dir = base.join("Parametric");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

pub fn app_db_path() -> std::io::Result<PathBuf> {
    Ok(app_data_dir()?.join("app.db"))
}

pub fn new_document_path(title: &str) -> std::io::Result<PathBuf> {
    let dir = documents_dir()?;
    let mut n = 0;
    loop {
        let name = if n == 0 {
            format!("{title}.parametric")
        } else {
            format!("{title} {n}.parametric")
        };
        let path = dir.join(name);
        if !path.exists() {
            return Ok(path);
        }
        n += 1;
    }
}
