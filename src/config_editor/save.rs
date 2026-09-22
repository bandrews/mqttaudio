// ABOUTME: Save and load logic for the config editor's sparse JSON document.
// ABOUTME: Validates via Config::validate(), backs up the target, and writes atomically.

use super::fields::ConfigDocument;
use crate::config::Config;
use std::path::{Path, PathBuf};

/// What happened when a document was loaded.
pub enum LoadOutcome {
    /// A file was found and parsed.
    Loaded { path: PathBuf, doc: ConfigDocument },
    /// No config file exists; start fresh.
    NotFound,
    /// A file exists but could not be read or parsed.
    Failed { path: PathBuf, error: String },
}

/// Load the document from `explicit_path` if given, else from the daemon's
/// config search paths (./mqttaudio.json, the user config dir, /etc/mqttaudio).
pub fn load_document(explicit_path: Option<&str>) -> LoadOutcome {
    let path = match explicit_path {
        Some(p) => PathBuf::from(p),
        None => match Config::find_config_file() {
            Some(p) => p,
            None => return LoadOutcome::NotFound,
        },
    };
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) => {
            if explicit_path.is_none() {
                return LoadOutcome::Failed {
                    path,
                    error: e.to_string(),
                };
            }
            // An explicit path that does not exist yet is a fresh start aimed
            // at that path; any other error is a real failure.
            if e.kind() == std::io::ErrorKind::NotFound {
                return LoadOutcome::NotFound;
            }
            return LoadOutcome::Failed {
                path,
                error: e.to_string(),
            };
        }
    };
    match ConfigDocument::parse(&text) {
        Ok(doc) => LoadOutcome::Loaded { path, doc },
        Err(error) => LoadOutcome::Failed { path, error },
    }
}

/// Candidate save paths, most specific first: the path the file was loaded
/// from (if any), then the daemon's search locations.
pub fn save_path_candidates(loaded_from: Option<&Path>) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(p) = loaded_from {
        candidates.push(p.to_path_buf());
    }
    for p in [
        PathBuf::from("./mqttaudio.json"),
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("mqttaudio")
            .join("config.json"),
        PathBuf::from("/etc/mqttaudio/config.json"),
    ] {
        if !candidates.contains(&p) {
            candidates.push(p);
        }
    }
    candidates
}

/// The result of a successful save.
#[derive(Debug)]
pub struct SaveOutcome {
    /// Path of the backup made of the previous file, if one existed.
    pub backup: Option<PathBuf>,
}

/// Validate the document and write it to `path` atomically (temp file + rename
/// in the target directory), backing up an existing file to `<path>.bak` first.
/// Refuses to write when the document does not deserialize or fails
/// `Config::validate()`, returning the error list.
pub fn save_document(doc: &ConfigDocument, path: &Path) -> Result<SaveOutcome, Vec<String>> {
    let config = doc.to_config().map_err(|e| vec![e])?;
    config.validate()?;

    let dir = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => PathBuf::from("."),
    };
    std::fs::create_dir_all(&dir)
        .map_err(|e| vec![format!("could not create {}: {}", dir.display(), e)])?;

    let mut backup = None;
    if path.exists() {
        let bak = path.with_extension(format!(
            "{}{}bak",
            path.extension()
                .map(|e| e.to_string_lossy().to_string())
                .unwrap_or_default(),
            if path.extension().is_some() { "." } else { "" }
        ));
        std::fs::copy(path, &bak)
            .map_err(|e| vec![format!("could not back up to {}: {}", bak.display(), e)])?;
        backup = Some(bak);
    }

    let tmp = dir.join(format!(
        ".{}.tmp",
        path.file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "mqttaudio.json".to_string())
    ));
    std::fs::write(&tmp, doc.to_pretty_string())
        .map_err(|e| vec![format!("could not write {}: {}", tmp.display(), e)])?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        vec![format!(
            "could not move into place at {}: {}",
            path.display(),
            e
        )]
    })?;

    Ok(SaveOutcome { backup })
}
