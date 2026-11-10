use std::{
    fs,
    io::{self, BufRead, BufReader, Read},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use crate::{
    files,
    model::{BuildStatus, Project},
};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

pub struct BuildResult {
    pub status: BuildStatus,
    pub elapsed: Duration,
    pub log: String,
    pub preview: Option<PathBuf>,
}

pub fn compile_tex(
    project: &Project,
    build_root: &Path,
    cancelled: Arc<AtomicBool>,
    on_log: impl FnMut(&str),
) -> io::Result<BuildResult> {
    if project.main_file.is_empty()
        || Path::new(&project.main_file)
            .extension()
            .is_none_or(|extension| extension.to_string_lossy().to_lowercase() != "tex")
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "主文件必须是 .tex 文件。",
        ));
    }
    let source = files::project_path(Path::new(&project.folder), &project.main_file)?;
    run_latexmk(
        project,
        &source,
        &source.with_extension("pdf"),
        build_root,
        cancelled,
        on_log,
    )
}

pub fn compile_markdown(
    project: &Project,
    relative_file: &str,
    build_root: &Path,
    cancelled: Arc<AtomicBool>,
    mut on_log: impl FnMut(&str),
) -> io::Result<BuildResult> {
    let source = files::project_path(Path::new(&project.folder), relative_file)?;
    if source
        .extension()
        .is_none_or(|extension| extension.to_string_lossy().to_lowercase() != "md")
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "请选择 Markdown 文件。",
        ));
    }
    let output = build_root.join(&project.id).join("work");
    fs::create_dir_all(&output)?;
    let started = Instant::now();
    if cancelled.load(Ordering::Relaxed) {
        return Ok(BuildResult {
            status: BuildStatus::Cancelled,
            elapsed: started.elapsed(),
            log: String::new(),
            preview: None,
        });
    }
    let content = fs::read_to_string(&source)?;
    let mut options = comrak::Options::default();
    options.extension.table = true;
    options.extension.tasklist = true;
    options.extension.strikethrough = true;
    options.extension.math_dollars = true;
    options.extension.math_code = true;
    let body = comrak::markdown_to_html(&crate::markdown::normalize_math(&content), &options);
    let browser = [
        std::env::var_os("ProgramFiles(x86)")
            .map(|dir| PathBuf::from(dir).join("Microsoft/Edge/Application/msedge.exe")),
        std::env::var_os("ProgramFiles")
            .map(|dir| PathBuf::from(dir).join("Microsoft/Edge/Application/msedge.exe")),
        std::env::var_os("ProgramFiles")
            .map(|dir| PathBuf::from(dir).join("Google/Chrome/Application/chrome.exe")),
    ]
    .into_iter()
    .flatten()
    .find(|path| path.is_file())
    .ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "Markdown PDF export needs Microsoft Edge or Google Chrome.",
        )
    })?;
    let katex = std::env::current_exe()?
        .parent()
        .expect("application has a directory")
        .join("katex");
    let katex = if katex.join("katex.min.css").is_file() {
        katex
    } else {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("project has a parent")
            .join("node_modules/katex/dist")
    };
    let html_path = output.join("folio-markdown-print.html");
    let browser_profile = output.join("folio-markdown-print-profile");
    let staged_pdf = output.join("folio-markdown-print.pdf");
    let base_url =
        url::Url::from_directory_path(source.parent().expect("Markdown file has a parent"))
            .expect("Markdown project directory is absolute");
    let katex_url = url::Url::from_directory_path(&katex).expect("KaTeX directory is absolute");
    fs::create_dir_all(&browser_profile)?;
    let html = format!(
        r#"<!doctype html><html><head><meta charset="utf-8"><base href="{base_url}"><link rel="stylesheet" href="{katex_url}katex.min.css"><style>
@page{{size:A4;margin:20mm}}body{{font:11pt/1.8 "Microsoft YaHei",sans-serif;color:#373b40}}h1,h2,h3{{line-height:1.35;margin-top:1.35em}}p{{margin-bottom:.55em}}blockquote{{margin-left:0;padding-left:1em;border-left:3px solid #d8e1eb;color:#747d87}}pre{{padding:14px 16px;white-space:pre-wrap;overflow-wrap:anywhere;background:#f5f6f7;border-radius:5px;break-inside:avoid}}code{{font-family:Consolas,monospace;font-size:.92em}}:not(pre)>code{{background:#f2f3f4;padding:.1em .3em;border-radius:3px}}a{{color:#527aa3}}table{{width:100%;border-collapse:collapse;margin:1em 0;break-inside:avoid}}td,th{{border:1px solid #dfe3e6;padding:6px 10px;vertical-align:top}}th{{background:#f6f7f8;text-align:left}}img{{max-width:100%;height:auto}}.katex-display{{overflow-x:auto}}input[type=checkbox]{{appearance:none;width:11px;height:11px;border:1px solid #7a7a7a;border-radius:2px;vertical-align:middle}}input[type=checkbox]:checked{{background:#5c7f9f}}
</style><script defer src="{katex_url}katex.min.js"></script><script defer src="{katex_url}contrib/auto-render.min.js"></script><script>document.addEventListener('DOMContentLoaded',()=>{{for(const node of document.querySelectorAll('[data-math-style]')){{const block=node.dataset.mathStyle==='display';const latex=node.textContent;if(block&&node.closest('pre')){{const display=document.createElement('div');display.textContent='$$'+latex+'$$';node.closest('pre').replaceWith(display)}}else node.textContent=(block?'$$':'$')+latex+(block?'$$':'$')}}renderMathInElement(document.body,{{delimiters:[{{left:'$$',right:'$$',display:true}},{{left:'\\[',right:'\\]',display:true}},{{left:'$',right:'$',display:false}},{{left:'\\(',right:'\\)',display:false}}],throwOnError:false}})}})</script></head><body>{body}</body></html>"#
    );
    fs::write(&html_path, html)?;
    let mut command = Command::new(browser);
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    let mut child = command
        .args([
            "--headless=new",
            "--disable-gpu",
            "--disable-extensions",
            "--no-first-run",
            "--no-default-browser-check",
            "--hide-scrollbars",
            "--no-pdf-header-footer",
            "--virtual-time-budget=1200",
        ])
        .arg(format!("--user-data-dir={}", browser_profile.display()))
        .arg(format!("--print-to-pdf={}", staged_pdf.display()))
        .arg(&html_path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let pid = child.id();
    let status = loop {
        if cancelled.load(Ordering::Relaxed) {
            terminate_process_tree(&mut child, pid);
            return Ok(BuildResult {
                status: BuildStatus::Cancelled,
                elapsed: started.elapsed(),
                log: String::new(),
                preview: None,
            });
        }
        if let Some(status) = child.try_wait()? {
            break status;
        }
        thread::sleep(Duration::from_millis(20));
    };
    if !status.success() || !staged_pdf.is_file() {
        let message = "Markdown PDF printing failed in Edge/Chrome.".to_owned();
        on_log(&message);
        return Ok(BuildResult {
            status: BuildStatus::Failed,
            elapsed: started.elapsed(),
            log: message,
            preview: None,
        });
    }
    let destination = source.with_extension("pdf");
    fs::copy(&staged_pdf, &destination)?;
    let preview = build_root.join(&project.id).join("preview.pdf");
    fs::copy(&staged_pdf, &preview)?;
    let log = format!(
        "Exported {} to the matching PDF beside the Markdown file.",
        source
            .file_name()
            .expect("Markdown has a filename")
            .to_string_lossy()
    );
    on_log(&log);
    Ok(BuildResult {
        status: BuildStatus::Success,
        elapsed: started.elapsed(),
        log,
        preview: Some(preview),
    })
}

fn run_latexmk(
    project: &Project,
    source: &Path,
    destination: &Path,
    build_root: &Path,
    cancelled: Arc<AtomicBool>,
    mut on_log: impl FnMut(&str),
) -> io::Result<BuildResult> {
    let output = build_root.join(&project.id).join("work");
    fs::create_dir_all(&output)?;
    let pdf_name = source
        .file_stem()
        .expect("TeX source has a file stem")
        .to_string_lossy()
        .to_string();
    let pdf = output.join(format!("{pdf_name}.pdf"));
    match fs::remove_file(&pdf) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let started = Instant::now();
    if cancelled.load(Ordering::Relaxed) {
        return Ok(BuildResult {
            status: BuildStatus::Cancelled,
            elapsed: Duration::ZERO,
            log: String::new(),
            preview: None,
        });
    }

    let mut command = Command::new("latexmk");
    #[cfg(windows)]
    command.creation_flags(0x08000000); // CREATE_NO_WINDOW, also for latexmk's child console.
    let mut child = command
        .args([
            project.engine.latexmk_flag(),
            "-interaction=nonstopmode",
            "-halt-on-error",
            "-file-line-error",
            "-no-shell-escape",
        ])
        .arg(format!("-outdir={}", output.display()))
        .arg(source)
        .current_dir(&project.folder)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let pid = child.id();
    let (sender, receiver) = mpsc::channel();
    let stdout = child.stdout.take().expect("stdout was piped");
    let stderr = child.stderr.take().expect("stderr was piped");
    let stdout_sender = sender.clone();
    let stdout_reader = thread::spawn(move || forward_output(stdout, stdout_sender));
    let stderr_reader = thread::spawn(move || forward_output(stderr, sender));
    let mut log = String::new();
    let exit = loop {
        for line in receiver.try_iter() {
            log.push_str(&line);
            on_log(&line);
        }
        if cancelled.load(Ordering::Relaxed) {
            terminate_process_tree(&mut child, pid);
            break None;
        }
        if let Some(status) = child.try_wait()? {
            break Some(status);
        }
        thread::sleep(Duration::from_millis(40));
    };
    stdout_reader
        .join()
        .expect("stdout reader thread panicked")?;
    stderr_reader
        .join()
        .expect("stderr reader thread panicked")?;
    for line in receiver.try_iter() {
        log.push_str(&line);
        on_log(&line);
    }
    if exit.is_none() {
        return Ok(BuildResult {
            status: BuildStatus::Cancelled,
            elapsed: started.elapsed(),
            log,
            preview: None,
        });
    }
    if !exit.expect("checked above").success() {
        return Ok(BuildResult {
            status: BuildStatus::Failed,
            elapsed: started.elapsed(),
            log,
            preview: None,
        });
    }

    let preview = build_root.join(&project.id).join("preview.pdf");
    fs::copy(&pdf, &preview)?;
    fs::copy(&pdf, destination)?;
    Ok(BuildResult {
        status: BuildStatus::Success,
        elapsed: started.elapsed(),
        log,
        preview: Some(preview),
    })
}

fn forward_output(mut output: impl Read, sender: mpsc::Sender<String>) -> io::Result<()> {
    let mut reader = BufReader::new(&mut output);
    let mut bytes = Vec::new();
    while reader.read_until(b'\n', &mut bytes)? > 0 {
        let line = String::from_utf8_lossy(&bytes).into_owned();
        if sender.send(line).is_err() {
            return Ok(());
        }
        bytes.clear();
    }
    Ok(())
}

fn terminate_process_tree(child: &mut Child, pid: u32) {
    if cfg!(target_os = "windows") {
        let mut command = Command::new("taskkill");
        #[cfg(windows)]
        command.creation_flags(0x08000000);
        let _ = command
            .args(["/pid", &pid.to_string(), "/T", "/F"])
            .status();
    } else {
        let _ = child.kill();
    }
    let _ = child.wait();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// This acceptance check runs real TeX engines; keep it opt-in so normal
    /// file-operation checks do not repeatedly rebuild documents.
    #[test]
    #[ignore = "requires installed TeX Live"]
    fn real_tex_and_markdown_publish_next_to_their_sources() {
        let temp = tempfile::tempdir().unwrap();
        let folder = temp.path().join("project with spaces");
        fs::create_dir_all(folder.join("nested")).unwrap();
        fs::write(
            folder.join("main.tex"),
            "\\documentclass{article}\n\\begin{document}Folio native compilation.\\end{document}",
        )
        .unwrap();
        fs::write(
            folder.join("nested/notes.md"),
            "# Folio\n\n中文导出与 English content.\n",
        )
        .unwrap();
        let mut project = Project {
            id: "smoke".into(),
            name: "smoke".into(),
            folder: folder.to_string_lossy().into_owned(),
            main_file: "main.tex".into(),
            engine: Engine::Xelatex,
        };
        let build = temp.path().join("builds");
        for engine in [Engine::Xelatex, Engine::Pdflatex, Engine::Lualatex] {
            project.engine = engine;
            let result =
                compile_tex(&project, &build, Arc::new(AtomicBool::new(false)), |_| {}).unwrap();
            assert_eq!(
                result.status,
                BuildStatus::Success,
                "{engine:?}: {}",
                result.log
            );
            assert!(
                fs::read(folder.join("main.pdf"))
                    .unwrap()
                    .starts_with(b"%PDF-")
            );
        }
        let result = compile_markdown(
            &project,
            "nested/notes.md",
            &build,
            Arc::new(AtomicBool::new(false)),
            |_| {},
        )
        .unwrap();
        assert_eq!(result.status, BuildStatus::Success, "{}", result.log);
        assert!(
            fs::read(folder.join("nested/notes.pdf"))
                .unwrap()
                .starts_with(b"%PDF-")
        );
        let published = fs::read(result.preview.unwrap()).unwrap();
        fs::write(
            folder.join("main.tex"),
            "\\documentclass{article}\\begin{document}\\undefinedfolio\\end{document}",
        )
        .unwrap();
        let failed =
            compile_tex(&project, &build, Arc::new(AtomicBool::new(false)), |_| {}).unwrap();
        assert_eq!(failed.status, BuildStatus::Failed);
        assert_eq!(
            fs::read(build.join("smoke/preview.pdf")).unwrap(),
            published
        );
    }
}
