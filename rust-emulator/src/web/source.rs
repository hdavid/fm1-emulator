// SPDX-License-Identifier: GPL-3.0-only
//! The files a web editor is served from: a `X-ui.zip` sidecar next to the
//! firmware `X.fwsc`, or a directory (`--ui DIR`). Request paths are
//! reduced to plain relative segments first; anything else is refused.
use std::{
    collections::HashMap,
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
};

/// Largest UI zip accepted (the editors are well under 1 MiB).
const MAX_ZIP: u64 = 64 << 20;

pub enum Source {
    /// Every file of the zip, read once at start.
    Zip(HashMap<String, Vec<u8>>),
    /// A directory, canonical, read per request.
    Dir(PathBuf),
}

/// `X-ui.zip` next to firmware `X.fwsc` (or `X.elf`, `X.bin`), if present.
pub fn sidecar_for(firmware: &Path) -> Option<PathBuf> {
    let stem = firmware.file_stem()?.to_str()?;
    let zip = firmware.with_file_name(format!("{stem}-ui.zip"));
    zip.is_file().then_some(zip)
}

/// A request path (already percent-decoded) as safe relative segments, or
/// None when it tries to leave the root or holds odd characters. The
/// empty path and directories mean their `index.html`.
pub fn clean_path(path: &str) -> Option<String> {
    let mut segments = Vec::new();
    for segment in path.split('/') {
        match segment {
            "" | "." => continue,
            ".." => return None,
            s if s.contains(['\\', ':', '\0']) || s.chars().any(char::is_control) => return None,
            s => segments.push(s),
        }
    }
    if segments.is_empty() || path.ends_with('/') {
        segments.push("index.html");
    }
    Some(segments.join("/"))
}

/// Percent-decoding of a URL path; None for broken escapes or non-UTF-8.
pub fn percent_decode(path: &str) -> Option<String> {
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

pub fn mime_type(path: &str) -> &'static str {
    let extension = path.rsplit_once('.').map_or("", |(_, e)| e);
    match extension.to_ascii_lowercase().as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" | "map" => "application/json",
        "webmanifest" => "application/manifest+json",
        "txt" => "text/plain; charset=utf-8",
        "md" => "text/markdown; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "wasm" => "application/wasm",
        "wav" => "audio/wav",
        _ => "application/octet-stream",
    }
}

impl Source {
    /// A `.zip` file or a directory.
    pub fn open(path: &Path) -> io::Result<Self> {
        if path.is_dir() {
            return Ok(Self::Dir(path.canonicalize()?));
        }
        let file = fs::File::open(path)?;
        if file.metadata()?.len() > MAX_ZIP {
            return Err(io::Error::other("UI zip larger than 64 MiB"));
        }
        let mut archive = zip::ZipArchive::new(file).map_err(io::Error::other)?;
        let mut files = HashMap::new();
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i).map_err(io::Error::other)?;
            if !entry.is_file() {
                continue;
            }
            // Names that would leave the root (zip slip) are skipped.
            let Some(name) = entry.enclosed_name().and_then(|p| zip_name(&p)) else {
                continue;
            };
            let mut data = Vec::new();
            entry.by_ref().take(MAX_ZIP).read_to_end(&mut data)?;
            files.insert(name, data);
        }
        Ok(Self::Zip(strip_single_folder(files)))
    }

    /// The bytes of a clean relative path (see `clean_path`).
    pub fn get(&self, path: &str) -> Option<Vec<u8>> {
        match self {
            Self::Zip(files) => files.get(path).cloned(),
            Self::Dir(root) => {
                let mut file = root.join(path);
                if file.is_dir() {
                    file = file.join("index.html");
                }
                // Symbolic links must not lead outside the root either.
                let file = file.canonicalize().ok()?;
                file.starts_with(root).then(|| fs::read(file).ok())?
            }
        }
    }

    /// Whether the source has a page to open.
    pub fn has_index(&self) -> bool {
        self.get("index.html").is_some()
    }
}

fn zip_name(path: &Path) -> Option<String> {
    let parts: Option<Vec<&str>> = path.iter().map(|p| p.to_str()).collect();
    clean_path(&parts?.join("/"))
}

/// A zip made of one folder (`editor/index.html`, ...) serves that folder.
fn strip_single_folder(files: HashMap<String, Vec<u8>>) -> HashMap<String, Vec<u8>> {
    if files.contains_key("index.html") {
        return files;
    }
    let Some(folder) = files.keys().find_map(|k| k.strip_suffix("/index.html")) else {
        return files;
    };
    let prefix = format!("{folder}/");
    if !files.keys().all(|k| k.starts_with(&prefix)) {
        return files;
    }
    files
        .into_iter()
        .map(|(k, v)| (k[prefix.len()..].to_owned(), v))
        .collect()
}
