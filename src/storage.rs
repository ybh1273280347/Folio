use std::{
    fs, io,
    path::{Path, PathBuf},
};

use uuid::Uuid;

use crate::{
    files,
    model::{AppSettings, Engine, Project, ProjectConfig, WorkspaceSession},
};

pub fn data_directory() -> io::Result<PathBuf> {
    if let Some(path) = std::env::var_os("FOLIO_USER_DATA") {
        return Ok(PathBuf::from(path));
    }
    dirs::config_dir()
        // The native migration must never write Electron's application data.
        .map(|directory| directory.join("folio-gpui"))
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, crate::i18n::workspace::ERR_DATA_DIR_NOT_FOUND.get()))
}

pub(crate) fn read_json<T: serde::de::DeserializeOwned + Default>(path: &Path) -> io::Result<T> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(io::Error::other),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(T::default()),
        Err(error) => Err(error),
    }
}

pub(crate) fn write_json(path: &Path, value: &impl serde::Serialize) -> io::Result<()> {
    fs::create_dir_all(path.parent().expect("application data file has a parent"))?;
    let bytes = serde_json::to_vec_pretty(value).map_err(io::Error::other)?;
    fs::write(path, bytes)
}

pub fn load_projects() -> io::Result<ProjectConfig> {
    read_json(&data_directory()?.join("projects.json"))
}

pub fn save_projects(config: &ProjectConfig) -> io::Result<()> {
    write_json(&data_directory()?.join("projects.json"), config)
}

pub fn load_workspace_sessions() -> io::Result<std::collections::BTreeMap<String, WorkspaceSession>>
{
    read_json(&data_directory()?.join("workspace-sessions.json"))
}

pub fn save_workspace_sessions(
    sessions: &std::collections::BTreeMap<String, WorkspaceSession>,
) -> io::Result<()> {
    write_json(&data_directory()?.join("workspace-sessions.json"), sessions)
}

pub fn load_settings() -> io::Result<AppSettings> {
    let mut values: serde_json::Map<String, serde_json::Value> =
        read_json(&data_directory()?.join("preferences.json"))?;
    // Older files shared the interface family with both panels and its size with AI.
    // An explicit empty family already means that the panel uses the system font.
    if let Some(family) = values.get("interfaceFont").cloned() {
        values.entry("markdownFont").or_insert_with(|| family.clone());
        values.entry("aiFont").or_insert(family);
    }
    if let Some(size) = values.get("interfaceFontSize").and_then(serde_json::Value::as_u64) {
        values.entry("aiFontSize").or_insert_with(|| size.clamp(11, 24).into());
    }
    let mut settings: AppSettings = serde_json::from_value(values.into()).map_err(io::Error::other)?;
    settings.theme.migrate_builtin_defaults();
    settings.background_image_opacity = settings.background_image_opacity.min(100);
    settings.text_color_presets =
        crate::model::sanitize_text_color_presets(std::mem::take(&mut settings.text_color_presets));
    settings.extra_text_extensions =
        files::sanitize_extra_text_extensions(std::mem::take(&mut settings.extra_text_extensions));
    Ok(settings)
}

pub fn save_settings(settings: &AppSettings) -> io::Result<()> {
    write_json(&data_directory()?.join("preferences.json"), settings)
}

pub fn add_project(config: &mut ProjectConfig, folder: &Path) -> io::Result<Project> {
    let folder = files::validate_folder(folder)?;
    let normalized = folder.to_string_lossy().to_string();
    if let Some(existing) = config
        .projects
        .iter()
        .find(|project| project.folder.eq_ignore_ascii_case(&normalized))
    {
        return Ok(existing.clone());
    }
    let files = files::list_files(&folder)?;
    let tex_files = files
        .iter()
        .filter(|file| file.path.to_lowercase().ends_with(".tex"));
    let main_file = tex_files
        .clone()
        .find(|file| {
            let lower = file.path.to_lowercase();
            ["main.tex", "index.tex", "paper.tex", "thesis.tex"]
                .iter()
                .any(|preferred| lower.ends_with(preferred))
        })
        .map(|file| file.path.clone())
        .or_else(|| tex_files.into_iter().next().map(|file| file.path.clone()))
        .unwrap_or_default();
    let project = Project {
        id: Uuid::new_v4().to_string(),
        name: folder
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
        folder: normalized,
        main_file,
        engine: Engine::default(),
    };
    config.projects.push(project.clone());
    config.active_id = Some(project.id.clone());
    Ok(project)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn first_latex_file_becomes_main_and_existing_folder_is_not_duplicated() {
        let temp = tempdir().unwrap();
        fs::write(temp.path().join("chapter.tex"), "").unwrap();
        fs::write(temp.path().join("thesis.tex"), "").unwrap();
        let mut config = ProjectConfig::default();
        let project = add_project(&mut config, temp.path()).unwrap();
        assert_eq!(project.main_file, "thesis.tex");
        assert_eq!(
            add_project(&mut config, temp.path()).unwrap().id,
            project.id
        );
        assert_eq!(config.projects.len(), 1);
    }
}
