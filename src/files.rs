use std::{
    fs::{self, File},
    io,
    path::{Component, Path, PathBuf},
};

use sha2::{Digest, Sha256};

use crate::model::{DiskDocument, FileKind, ProjectFile};

const TEXT_EXTENSIONS: &[&str] = &[
    "tex", "ltx", "bib", "sty", "cls", "bst", "bbx", "cbx", "cfg", "def", "clo", "ist", "glsdefs",
    "txt", "md", "canvas", "csv", "tsv", "dat", "tikz", "pgf", "asy", "json", "yaml", "yml", "toml", "eps",
];
const ASSET_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "webp", "gif", "svg", "pdf", "html", "htm", "drawio",
];
pub const EXCLUDED_DIRECTORIES: &[&str] = &[
    ".git",
    ".pi",
    "node_modules",
    ".idea",
    ".vscode",
    ".folio",
    "build",
    "dist",
    "target",
    "runtime",
];

pub fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(hi), Some(lo)) = (hex_digit(bytes[i + 1]), hex_digit(bytes[i + 2]))
        {
            out.push((hi << 4) | lo);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|err| String::from_utf8_lossy(err.as_bytes()).into_owned())
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

pub fn is_drawio(path: &Path) -> bool {
    let lower = path.to_string_lossy().to_ascii_lowercase();
    lower.ends_with(".drawio") || lower.ends_with(".drawio.svg")
}

pub fn is_relevant(path: &Path) -> bool {
    path.file_name().is_some_and(|name| name == ".latexmkrc")
        || is_drawio(path)
        || extension_in(path, TEXT_EXTENSIONS)
        || extension_in(path, ASSET_EXTENSIONS)
}

fn extension_in(path: &Path, extensions: &[&str]) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            let extension = extension.to_ascii_lowercase();
            extensions.contains(&extension.as_str())
        })
}

pub(crate) fn is_text(path: &Path) -> bool {
    !is_drawio(path)
        && (path.file_name().is_some_and(|name| name == ".latexmkrc")
            || extension_in(path, TEXT_EXTENSIONS))
}

pub fn is_creatable(path: &Path) -> bool {
    is_text(path) || is_drawio(path)
}

pub fn project_path(root: &Path, relative: &str) -> io::Result<PathBuf> {
    let relative_path = Path::new(relative);
    if relative.is_empty()
        || relative_path.is_absolute()
        || relative_path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            crate::i18n::workspace::ERR_OUTSIDE_PROJECT.get(),
        ));
    }
    let canonical_root = dunce::canonicalize(root)?;
    let target = dunce::canonicalize(canonical_root.join(relative_path))?;
    if !target.starts_with(&canonical_root) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            crate::i18n::workspace::ERR_OUTSIDE_PROJECT.get(),
        ));
    }
    Ok(target)
}

pub fn validate_folder(folder: &Path) -> io::Result<PathBuf> {
    let resolved = dunce::canonicalize(folder)?;
    if !resolved.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            crate::i18n::workspace::ERR_SELECT_FOLDER.get(),
        ));
    }
    Ok(resolved)
}

pub fn list_files(root: &Path) -> io::Result<Vec<ProjectFile>> {
    fn visit(root: &Path, current: &Path, files: &mut Vec<ProjectFile>) -> io::Result<()> {
        for entry in fs::read_dir(current)? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir()
                && !EXCLUDED_DIRECTORIES.contains(&entry.file_name().to_string_lossy().as_ref())
            {
                visit(root, &entry.path(), files)?;
            } else if file_type.is_file() && is_relevant(&entry.path()) {
                let path = entry
                    .path()
                    .strip_prefix(root)
                    .expect("walked from project root")
                    .to_string_lossy()
                    .replace('\\', "/");
                files.push(ProjectFile {
                    kind: if is_text(&entry.path()) {
                        FileKind::Text
                    } else {
                        FileKind::Asset
                    },
                    path,
                });
            }
        }
        Ok(())
    }

    let root = dunce::canonicalize(root)?;
    let mut files = Vec::new();
    visit(&root, &root, &mut files)?;
    files.sort_by(|left, right| {
        left.path
            .to_lowercase()
            .cmp(&right.path.to_lowercase())
            .then_with(|| left.path.cmp(&right.path))
    });
    Ok(files)
}

fn revision(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub fn read_document(root: &Path, relative: &str) -> io::Result<DiskDocument> {
    read_document_at(&project_path(root, relative)?)
}

/// Reads a document directly by its canonical path for files in the default space.
pub fn read_document_at(path: &Path) -> io::Result<DiskDocument> {
    let bytes = fs::read(path)?;
    Ok(DiskDocument {
        text: String::from_utf8_lossy(&bytes).into_owned(),
        revision: revision(&bytes),
    })
}

pub fn save_document(
    root: &Path,
    relative: &str,
    text: &str,
    expected_revision: &str,
) -> io::Result<Result<DiskDocument, DiskDocument>> {
    save_document_at(&project_path(root, relative)?, text, expected_revision)
}

/// Saves a default-space document only when its on-disk revision still matches.
pub fn save_document_at(
    path: &Path,
    text: &str,
    expected_revision: &str,
) -> io::Result<Result<DiskDocument, DiskDocument>> {
    let current = read_document_at(path)?;
    if current.revision != expected_revision {
        return Ok(Err(current));
    }
    fs::write(path, text.as_bytes())?;
    Ok(Ok(DiskDocument {
        text: text.to_owned(),
        revision: revision(text.as_bytes()),
    }))
}

pub fn create_file(root: &Path, relative: &str) -> io::Result<()> {
    let path = Path::new(relative);
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    if path.is_absolute()
        || !is_creatable(path)
        || name.is_empty()
        || name == "."
        || name == ".."
        || name.chars().any(|char| "<>:\"|?*".contains(char))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            crate::i18n::workspace::ERR_INVALID_PROJECT_FILENAME.get(),
        ));
    }
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map_or_else(
            || Ok(dunce::canonicalize(root)?),
            |parent| project_path(root, &parent.to_string_lossy()),
        )?;
    File::options()
        .write(true)
        .create_new(true)
        .open(parent.join(name))?;
    Ok(())
}

pub fn create_untitled_in(
    root: &Path,
    folder: Option<&str>,
    make_name: impl Fn(Option<usize>) -> String,
) -> io::Result<ProjectFile> {
    let prefix = folder
        .map(|folder| folder.trim().trim_matches(['/', '\\']))
        .filter(|folder| !folder.is_empty());
    let mut number = 1;
    loop {
        let name = make_name((number > 1).then_some(number));
        let relative = match prefix {
            Some(folder) => format!("{folder}/{name}"),
            None => name,
        };
        match create_file(root, &relative) {
            Ok(()) => {
                return Ok(ProjectFile {
                    kind: if is_text(Path::new(&relative)) {
                        FileKind::Text
                    } else {
                        FileKind::Asset
                    },
                    path: relative,
                });
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => number += 1,
            Err(error) => return Err(error),
        }
    }
}

pub fn create_markdown_in(root: &Path, folder: Option<&str>) -> io::Result<ProjectFile> {
    create_untitled_in(root, folder, crate::i18n::workspace::untitled_md)
}

pub fn create_drawio_in(root: &Path, folder: Option<&str>) -> io::Result<ProjectFile> {
    create_untitled_in(root, folder, crate::i18n::workspace::untitled_drawio)
}

pub fn create_canvas_in(root: &Path, folder: Option<&str>) -> io::Result<ProjectFile> {
    let created = create_untitled_in(root, folder, crate::i18n::workspace::untitled_canvas)?;
    if let Ok(path) = project_path(root, &created.path) {
        let _ = fs::write(path, "{\n  \"nodes\": [],\n  \"edges\": []\n}\n");
    }
    Ok(created)
}

pub fn create_named_canvas_in(
    root: &Path,
    folder: Option<&str>,
    raw_name: &str,
) -> io::Result<ProjectFile> {
    let trimmed = raw_name.trim();
    if trimmed.is_empty()
        || trimmed == "."
        || trimmed == ".."
        || trimmed
            .chars()
            .any(|ch| ch == '/' || ch == '\\' || "<>:\"|?*".contains(ch))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            crate::i18n::workspace::ERR_INVALID_FILENAME_EXT.get(),
        ));
    }
    let filename = if trimmed.to_ascii_lowercase().ends_with(".canvas") {
        trimmed.to_owned()
    } else {
        format!("{trimmed}.canvas")
    };
    let clean_folder = folder.map(str::trim).filter(|s| !s.is_empty());
    let rel = match clean_folder {
        Some(dir) => format!("{}/{}", dir.trim_end_matches('/'), filename),
        None => filename,
    };
    let full = project_path(root, &rel)?;
    if let Some(parent) = full.parent() {
        fs::create_dir_all(parent)?;
    }
    if !full.exists() {
        fs::write(&full, "{\n  \"nodes\": [],\n  \"edges\": []\n}\n")?;
    }
    Ok(ProjectFile {
        kind: FileKind::Text,
        path: rel,
    })
}

pub fn rename_file(root: &Path, relative: &str, name: &str) -> io::Result<String> {
    if name.is_empty()
        || name == "."
        || name == ".."
        || name
            .chars()
            .any(|char| char == '/' || char == '\\' || "<>:\"|?*".contains(char))
        || !is_relevant(Path::new(name))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            crate::i18n::workspace::ERR_INVALID_FILENAME_EXT.get(),
        ));
    }
    let source = project_path(root, relative)?;
    let destination = source.with_file_name(name);
    fs::hard_link(&source, &destination)?;
    fs::remove_file(source)?;
    let parent = Path::new(relative)
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty());
    Ok(parent.map_or_else(
        || name.to_owned(),
        |parent| format!("{}/{}", parent.to_string_lossy().replace('\\', "/"), name),
    ))
}

pub fn list_folders(root: &Path) -> io::Result<Vec<String>> {
    fn visit(root: &Path, current: &Path, folders: &mut Vec<String>) -> io::Result<()> {
        for entry in fs::read_dir(current)? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir()
                && !EXCLUDED_DIRECTORIES.contains(&entry.file_name().to_string_lossy().as_ref())
            {
                let path = entry
                    .path()
                    .strip_prefix(root)
                    .expect("walked from project root")
                    .to_string_lossy()
                    .replace('\\', "/");
                folders.push(path);
                visit(root, &entry.path(), folders)?;
            }
        }
        Ok(())
    }

    let root = dunce::canonicalize(root)?;
    let mut folders = Vec::new();
    visit(&root, &root, &mut folders)?;
    folders.sort_by(|left, right| {
        left.to_lowercase()
            .cmp(&right.to_lowercase())
            .then_with(|| left.cmp(right))
    });
    Ok(folders)
}

pub fn create_folder(root: &Path, relative: &str) -> io::Result<()> {
    let relative = relative.trim().trim_matches(['/', '\\']);
    let path = Path::new(relative);
    if relative.is_empty()
        || path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
        || relative.chars().any(|char| "<>:\"|?*".contains(char))
        || path.components().any(|part| {
            EXCLUDED_DIRECTORIES
                .iter()
                .any(|name| part.as_os_str() == *name)
        })
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            crate::i18n::workspace::ERR_INVALID_FOLDER_NAME.get(),
        ));
    }
    let canonical_root = dunce::canonicalize(root)?;
    let target = canonical_root.join(path);
    if target.exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            crate::i18n::workspace::ERR_NAME_ALREADY_EXISTS.get(),
        ));
    }
    fs::create_dir_all(target)
}

/// Move a whole directory within one canonical project root, without merging or replacing a target.
pub fn move_folder_into(root: &Path, relative: &str, folder: &str) -> io::Result<String> {
    let root = dunce::canonicalize(root)?;
    let source = project_path(&root, relative)?;
    let destination = project_path(&root, if folder.is_empty() { "." } else { folder })?;
    if source == root || !source.is_dir() || !destination.is_dir() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, crate::i18n::workspace::ERR_SELECT_PROJECT_FOLDER.get()));
    }
    if destination.starts_with(&source) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, crate::i18n::workspace::ERR_MOVE_FOLDER_INTO_SELF.get()));
    }
    // Both existing paths were resolved inside root; appending one filename keeps the target inside it too.
    let target = destination.join(source.file_name().expect("non-root folder has a name"));
    if source == target {
        return Ok(relative.to_owned());
    }
    if target.try_exists()? {
        return Err(io::Error::new(io::ErrorKind::AlreadyExists, crate::i18n::workspace::ERR_TARGET_EXISTS_UNMOVED.get()));
    }
    fs::rename(&source, &target)?;
    Ok(target.strip_prefix(&root).expect("validated project destination").to_string_lossy().replace('\\', "/"))
}

pub fn rename_folder(root: &Path, relative: &str, name: &str) -> io::Result<String> {
    let name = name.trim();
    if name.is_empty()
        || name == "."
        || name == ".."
        || name
            .chars()
            .any(|char| char == '/' || char == '\\' || "<>:\"|?*".contains(char))
        || EXCLUDED_DIRECTORIES.contains(&name)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            crate::i18n::workspace::ERR_INVALID_FOLDER_NAME.get(),
        ));
    }
    let source = project_path(root, relative)?;
    if !source.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            crate::i18n::workspace::ERR_SELECT_PROJECT_FOLDER.get(),
        ));
    }
    let current_name = source
        .file_name()
        .and_then(|part| part.to_str())
        .unwrap_or_default();
    let parent = Path::new(relative)
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty());
    let renamed = parent.map_or_else(
        || name.to_owned(),
        |parent| format!("{}/{}", parent.to_string_lossy().replace('\\', "/"), name),
    );
    if current_name == name {
        return Ok(renamed);
    }
    let destination = source.with_file_name(name);
    if !current_name.eq_ignore_ascii_case(name) && destination.exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            crate::i18n::workspace::ERR_NAME_ALREADY_EXISTS.get(),
        ));
    }
    fs::rename(&source, &destination)?;
    Ok(renamed)
}

#[cfg(target_os = "windows")]
fn files_app_executable() -> Option<PathBuf> {
    use std::os::windows::process::CommandExt;
    use std::process::Command;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let is_files_default = [
        r"HKCU\Software\Classes\Folder\shell\open\command",
        r"HKCU\Software\Classes\Directory\shell\open\command",
    ]
    .into_iter()
    .any(|key| {
        Command::new("reg")
            .args(["query", key, "/ve"])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .ok()
            .filter(|output| output.status.success())
            .is_some_and(|output| {
                let stdout = String::from_utf8_lossy(&output.stdout).to_ascii_lowercase();
                stdout.contains("files.app.launcher.exe")
                    || stdout.contains("files-stable")
                    || stdout.contains("files-preview")
                    || stdout.contains("files-dev")
            })
    });
    if !is_files_default {
        return None;
    }

    let local_app_data = PathBuf::from(std::env::var_os("LOCALAPPDATA")?);
    let windows_apps = local_app_data.join("Microsoft").join("WindowsApps");

    if let Ok(branch) = fs::read_to_string(local_app_data.join("Files").join("Branch.txt")) {
        let alias = branch.trim();
        if !alias.is_empty() {
            let candidate = windows_apps.join(format!("{alias}.exe"));
            if candidate.exists() {
                return Some(candidate);
            }
        }
    }

    for alias in ["files-stable.exe", "files-preview.exe", "files-dev.exe"] {
        let candidate = windows_apps.join(alias);
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

#[cfg(target_os = "windows")]
pub fn reveal_path_windows(path: &Path) {
    use std::os::windows::process::CommandExt;
    use std::process::Command;

    if path.as_os_str().is_empty() {
        return;
    }
    let target = path.to_path_buf();
    std::thread::spawn(move || {
        let normalized = dunce::canonicalize(&target)
            .unwrap_or_else(|_| PathBuf::from(target.to_string_lossy().replace('/', "\\")));
        let has_parent = normalized.parent().is_some();

        if let Some(files_exe) = files_app_executable() {
            let mut command = Command::new(&files_exe);
            if has_parent {
                command.arg("-select").arg(&normalized);
            } else {
                command.arg(&normalized);
            }
            if command.spawn().is_ok() {
                return;
            }
        }

        let mut explorer = Command::new("explorer.exe");
        if has_parent {
            let escaped = normalized.to_string_lossy().replace('"', "");
            explorer.raw_arg(format!("/select,\"{escaped}\""));
        } else {
            explorer.arg(&normalized);
        }
        let _ = explorer.spawn();
    });
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn quick_notes_number_without_overwriting_existing_content() {
        let temp = tempdir().unwrap();
        fs::write(temp.path().join("未命名.md"), "existing").unwrap();
        fs::write(temp.path().join("未命名3.md"), "third").unwrap();
        assert_eq!(create_markdown_in(temp.path(), None).unwrap().path, "未命名2.md");
        assert_eq!(create_markdown_in(temp.path(), None).unwrap().path, "未命名4.md");
        assert_eq!(fs::read_to_string(temp.path().join("未命名.md")).unwrap(), "existing");
    }

    #[test]
    fn renamed_markdown_keeps_body_and_accepts_unsaved_draft() {
        let temp = tempdir().unwrap();
        let note = create_markdown_in(temp.path(), None).unwrap();
        fs::write(temp.path().join(&note.path), "# Independent heading\n\nbody").unwrap();
        let disk = read_document(temp.path(), &note.path).unwrap();
        let renamed = rename_file(temp.path(), &note.path, "读书笔记.md").unwrap();
        assert_eq!(read_document(temp.path(), &renamed).unwrap().text, disk.text);
        save_document(temp.path(), &renamed, "unsaved draft", &disk.revision).unwrap().unwrap();
        assert_eq!(fs::read_to_string(temp.path().join(&renamed)).unwrap(), "unsaved draft");
        assert!(!temp.path().join(&note.path).exists());
        fs::write(temp.path().join("occupied.md"), "keep").unwrap();
        assert!(rename_file(temp.path(), &renamed, "occupied.md").is_err());
        assert_eq!(fs::read_to_string(temp.path().join("occupied.md")).unwrap(), "keep");
        assert!(temp.path().join(renamed).exists());
    }

    #[test]
    fn listing_filters_build_directories_and_sorts_nested_supported_files() {
        let temp = tempdir().unwrap();
        fs::create_dir_all(temp.path().join("chapters")).unwrap();
        fs::create_dir_all(temp.path().join("build")).unwrap();
        fs::write(temp.path().join("chapters/intro.tex"), "source").unwrap();
        fs::write(temp.path().join("image.png"), "asset").unwrap();
        fs::write(temp.path().join("build/ignored.tex"), "generated").unwrap();
        fs::write(temp.path().join("notes.exe"), "unsupported").unwrap();

        let files = list_files(temp.path()).unwrap();
        assert_eq!(
            files
                .iter()
                .map(|file| (file.path.as_str(), file.kind))
                .collect::<Vec<_>>(),
            [
                ("chapters/intro.tex", FileKind::Text),
                ("image.png", FileKind::Asset),
            ]
        );
    }

    #[test]
    fn save_compares_the_current_disk_revision_before_writing() {
        let temp = tempdir().unwrap();
        fs::write(temp.path().join("main.tex"), "first").unwrap();
        let original = read_document(temp.path(), "main.tex").unwrap();
        fs::write(temp.path().join("main.tex"), "external").unwrap();

        let conflict = save_document(temp.path(), "main.tex", "draft", &original.revision)
            .unwrap()
            .unwrap_err();
        assert_eq!(conflict.text, "external");
        assert_eq!(
            fs::read_to_string(temp.path().join("main.tex")).unwrap(),
            "external"
        );
        assert_eq!(
            save_document(temp.path(), "main.tex", "draft", &conflict.revision)
                .unwrap()
                .unwrap()
                .text,
            "draft"
        );
    }

    #[test]
    fn paths_cannot_escape_the_selected_project() {
        let temp = tempdir().unwrap();
        let outside = temp.path().join("outside.tex");
        fs::write(&outside, "secret").unwrap();
        let project = temp.path().join("project");
        fs::create_dir(&project).unwrap();
        assert_eq!(
            project_path(&project, "../outside.tex").unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        assert!(project_path(&project, "outside.tex").is_err());
    }

    #[test]
    fn folder_rename_preserves_contents_and_rejects_invalid_names() {
        let temp = tempdir().unwrap();
        fs::create_dir_all(temp.path().join("parent/old_dir")).unwrap();
        fs::create_dir_all(temp.path().join("parent/occupied")).unwrap();
        fs::write(temp.path().join("parent/old_dir/note.md"), "hello").unwrap();

        let renamed = rename_folder(temp.path(), "parent/old_dir", "new_dir").unwrap();
        assert_eq!(renamed, "parent/new_dir");
        assert!(!temp.path().join("parent/old_dir").exists());
        assert_eq!(
            fs::read_to_string(temp.path().join("parent/new_dir/note.md")).unwrap(),
            "hello"
        );
        assert!(rename_folder(temp.path(), &renamed, "occupied").is_err());
        assert!(rename_folder(temp.path(), &renamed, "bad/name").is_err());
        assert!(rename_folder(temp.path(), &renamed, ".git").is_err());
    }

    #[test]
    fn percent_decode_decodes_utf8_and_preserves_plus() {
        assert_eq!(percent_decode("hello%20world+1.md"), "hello world+1.md");
        assert_eq!(percent_decode("%E7%AC%94%E8%AE%B0.md"), "笔记.md");
        assert_eq!(percent_decode("bad%ZZ%2"), "bad%ZZ%2");
    }

    #[test]
    fn drawio_and_drawio_svg_are_relevant_assets() {
        let temp = tempdir().unwrap();
        let created = create_drawio_in(temp.path(), None).unwrap();
        assert_eq!(created.kind, FileKind::Asset);
        assert!(created.path.ends_with(".drawio"));
        assert!(is_relevant(Path::new("diagram.drawio")));
        assert!(is_relevant(Path::new("diagram.drawio.svg")));
        assert!(!is_text(Path::new("diagram.drawio")));
        assert!(!is_text(Path::new("diagram.drawio.svg")));
        assert!(is_relevant(Path::new("plain.svg")));
        assert!(!is_text(Path::new("plain.svg")));
    }

    #[test]
    fn create_file_rejects_readonly_assets_and_allows_editable_types() {
        let temp = tempdir().unwrap();
        assert!(create_file(temp.path(), "note.md").is_ok());
        assert!(create_file(temp.path(), "paper.tex").is_ok());
        assert!(create_file(temp.path(), "chart.drawio").is_ok());
        assert!(create_file(temp.path(), "chart.drawio.svg").is_ok());
        assert!(create_file(temp.path(), ".latexmkrc").is_ok());

        for readonly in ["doc.pdf", "photo.png", "cover.jpg", "page.html", "index.htm", "bad.exe"] {
            assert_eq!(
                create_file(temp.path(), readonly).unwrap_err().kind(),
                io::ErrorKind::InvalidInput,
                "expected {readonly} to be rejected"
            );
            assert!(!temp.path().join(readonly).exists());
        }
    }

    #[test]
    fn untitled_preset_creation_numbers_without_overwriting() {
        let temp = tempdir().unwrap();
        let first = create_untitled_in(temp.path(), None, |n| {
            crate::i18n::workspace::untitled_ext("tex", n)
        })
        .unwrap();
        let second = create_untitled_in(temp.path(), None, |n| {
            crate::i18n::workspace::untitled_ext("tex", n)
        })
        .unwrap();
        assert_eq!(first.path, "未命名.tex");
        assert_eq!(first.kind, FileKind::Text);
        assert_eq!(second.path, "未命名2.tex");
        assert_eq!(second.kind, FileKind::Text);
    }
}
