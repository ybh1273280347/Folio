//! Workspace lifecycle for the knowledge index, OCR jobs and sidebar state.

use super::{
    Workspace,
    panels::{KbPanelState, PdfOcrPhase},
};
use crate::rag::{
    DirtyDocument, RagError,
    client::EmbeddingClient,
    collect_eligible_paths,
    ocr::{self, OcrProgressEvent},
    pipeline::{KbSearchCache, SyncOptions, sync_project},
    store::{PdfLibraryItem, RagStore, StoreStats},
};
use gpui_kit::{AppContext, Context, RenderImage, Window};

use std::{
    collections::{BTreeSet, HashMap, HashSet},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

const SYNC_DEBOUNCE: Duration = Duration::from_millis(2500);

enum OcrRunnerEvent {
    Progress(OcrProgressEvent),
    ItemDone {
        rel_path: String,
        stats: Option<StoreStats>,
        pdf_items: Option<Vec<PdfLibraryItem>>,
    },
    ItemFailed {
        rel_path: String,
        message: String,
        stats: Option<StoreStats>,
        pdf_items: Option<Vec<PdfLibraryItem>>,
    },
    ItemCancelled {
        rel_path: String,
    },
    BatchFinished {
        project_key: String,
    },
}

struct OcrRunnerHandle {
    events: mpsc::Receiver<OcrRunnerEvent>,
    global_cancel: Arc<AtomicBool>,
    item_cancels: HashMap<String, Arc<AtomicBool>>,
}

/// Project switches discard pending work and keep late job results off the active sidebar.
#[derive(Default)]
pub(super) struct RagSyncState {
    pending_deadline: Option<Instant>,
    in_flight: Option<(u64, String, Arc<AtomicBool>)>,
    next_sync_id: u64,
    kb_search_cache: KbSearchCache,
    ocr_runner: Option<OcrRunnerHandle>,
}

impl Workspace {
    pub(super) fn active_rag_root(&self) -> Option<(String, PathBuf)> {
        let project = self.projects.directory()?;
        let root = dunce::canonicalize(&project.folder).ok()?;
        let key = crate::ai::project_key(&root);
        Some((key, root))
    }

    pub(super) fn rag_eligible_paths(&self) -> Vec<String> {
        collect_eligible_paths(self.tree.files().iter().map(|file| file.path.as_str()))
    }

    /// Snapshot unsaved text so the background worker can search it alongside the index.
    pub(super) fn dirty_rag_documents(&self) -> Vec<DirtyDocument> {
        self.documents
            .open_buffers()
            .filter(|(_, buffer)| buffer.is_dirty() && !buffer.deleted)
            .map(|(path, buffer)| DirtyDocument {
                rel_path: path.to_owned(),
                text: buffer.text.clone(),
            })
            .collect()
    }

    pub(in crate::workspace) fn schedule_rag_sync(&mut self) {
        let Some((_, root)) = self.active_rag_root() else {
            return;
        };
        if !root.join(".folio").join("rag.sqlite3").exists() {
            return;
        }
        self.rag_sync.pending_deadline = Some(Instant::now() + SYNC_DEBOUNCE);
    }

    fn live_project_paths(&self) -> HashSet<String> {
        self.tree
            .files()
            .iter()
            .map(|file| file.path.clone())
            .collect()
    }

    pub(in crate::workspace) fn cancel_in_flight_sync(&mut self) {
        if let Some((_, _, cancelled)) = self.rag_sync.in_flight.take() {
            cancelled.store(true, Ordering::Relaxed);
        }
    }

    pub(in crate::workspace) fn relocate_rag_paths(
        &mut self,
        moves: &[(String, String)],
        is_folder: bool,
        cx: &mut Context<Self>,
    ) {
        if moves.is_empty() {
            return;
        }
        self.right_panel.kb.relocate_paths(moves, is_folder);
        if let Some(handle) = &self.rag_sync.ocr_runner {
            for (path, cancel) in &handle.item_cancels {
                if let Some(next) =
                    KbPanelState::remap_rel_path(path, moves, is_folder)
                {
                    cancel.store(true, Ordering::Relaxed);
                    self.right_panel.kb.ocr_progress.remove(&next);
                    self.right_panel.kb.ocr_progress.remove(path);
                }
            }
        }
        self.cancel_in_flight_sync();
        if let Some((_, root)) = self.active_rag_root()
            && root.join(".folio").join("rag.sqlite3").exists()
        {
            self.rag_sync.pending_deadline =
                Some(Instant::now() + Duration::from_millis(400));
        }
        cx.notify();
    }

    pub(in crate::workspace) fn reconcile_rag_on_tree_change(
        &mut self,
        previous: &BTreeSet<String>,
    ) {
        let current = self.live_project_paths();
        let mut removed = Vec::new();
        for old in previous {
            if !current.contains(old) {
                removed.push(old.clone());
            }
        }
        let added = current.iter().any(|path| !previous.contains(path));
        if removed.is_empty() && !added {
            return;
        }
        if let Some(handle) = &self.rag_sync.ocr_runner {
            for old in &removed {
                if let Some(cancel) = handle.item_cancels.get(old) {
                    cancel.store(true, Ordering::Relaxed);
                }
            }
        }
        self.right_panel.kb.prune_missing_paths(&current);
        if self.rag_sync.in_flight.is_some() {
            self.cancel_in_flight_sync();
        }
    }

    pub(in crate::workspace) fn cancel_rag_sync(&mut self) {
        self.rag_sync.pending_deadline = None;
        self.rag_sync.kb_search_cache = KbSearchCache::default();
        self.cancel_in_flight_sync();
        self.cancel_all_pdf_ocr_silent();
    }

    fn cancel_all_pdf_ocr_silent(&mut self) {
        if let Some(handle) = self.rag_sync.ocr_runner.take() {
            handle.global_cancel.store(true, Ordering::Relaxed);
            for cancel in handle.item_cancels.values() {
                cancel.store(true, Ordering::Relaxed);
            }
        }
        self.right_panel.kb.ocr_progress.clear();
    }

    pub(in crate::workspace) fn cancel_all_pdf_ocr(&mut self, cx: &mut Context<Self>) {
        self.cancel_all_pdf_ocr_silent();
        cx.notify();
    }

    pub(in crate::workspace) fn cancel_pdf_ocr_item(
        &mut self,
        rel_path: &str,
        cx: &mut Context<Self>,
    ) {
        if let Some(handle) = &self.rag_sync.ocr_runner
            && let Some(cancel) = handle.item_cancels.get(rel_path)
        {
            cancel.store(true, Ordering::Relaxed);
        }
        self.right_panel.kb.ocr_progress.remove(rel_path);
        cx.notify();
    }

    pub(in crate::workspace) fn kb_enabled(&self) -> bool {
        self.active_rag_root()
            .is_some_and(|(_, root)| root.join(".folio").join("rag.sqlite3").exists())
    }

    pub(in crate::workspace) fn kb_syncing(&self) -> bool {
        self.rag_sync.in_flight.is_some()
    }

    pub(in crate::workspace) fn kb_badge(&self) -> Option<(String, bool)> {
        self.active_rag_root()?;
        if self.kb_syncing() {
            return Some((crate::i18n::right_panel::BADGE_SYNCING.get().into(), false));
        }
        if !self.kb_enabled() {
            return None;
        }
        let red_count = self.right_panel.kb.red_dot_count();
        if red_count > 0 {
            return Some((crate::i18n::right_panel::kb_ocr_badge(red_count), true));
        }
        let stats = self.right_panel.kb.stats.as_ref()?;
        Some((crate::i18n::right_panel::kb_doc_count(stats.indexed_files), false))
    }

    pub(in crate::workspace) fn reset_rag_sync_on_project_switch(&mut self, cx: &mut Context<Self>) {
        self.cancel_rag_sync();
        self.right_panel.kb.reset_for_project();
        let Some((_, root)) = self.active_rag_root() else {
            cx.notify();
            return;
        };
        if !root.join(".folio").join("rag.sqlite3").exists() {
            cx.notify();
            return;
        }
        self.refresh_kb_snapshot(cx);
        self.rag_sync.pending_deadline = Some(Instant::now() + Duration::from_millis(500));
    }

    pub(in crate::workspace) fn trigger_rag_sync_now(&mut self, cx: &mut Context<Self>) {
        self.run_kb_sync(false, None, Vec::new(), cx);
    }

    pub(in crate::workspace) fn refresh_kb_snapshot(&mut self, cx: &mut Context<Self>) {
        let Some((project_key, root)) = self.active_rag_root() else {
            self.right_panel.kb.stats = None;
            self.right_panel.kb.pdf_items.clear();
            cx.notify();
            return;
        };
        let config = self.settings.rag.clone();
        let db_path = root.join(".folio").join("rag.sqlite3");
        let accurate_configured = !ocr::resolve_ocr_token(&config).is_empty();
        self.right_panel.kb.accurate_configured = accurate_configured;
        if !db_path.exists() {
            self.right_panel.kb.stats = None;
            self.right_panel.kb.pdf_items.clear();
            cx.notify();
            return;
        }

        let root_bg = root.clone();
        let task = cx.background_spawn(async move {
            let snapshot = RagStore::open(&db_path, &config.embed_model, config.embed_dim)
                .and_then(|store| {
                    let stats = store.stats()?;
                    let mut items = store.pdf_library_items()?;
                    ocr::hydrate_pdf_library_items(&root_bg, &mut items);
                    Ok((stats, items))
                })
                .ok();
            (snapshot, accurate_configured)
        });

        cx.spawn(async move |this, cx| {
            let (snapshot, accurate_configured) = task.await;
            let _ = this.update(cx, |app, cx| {
                if app
                    .active_rag_root()
                    .is_some_and(|(active_key, _)| active_key == project_key)
                {
                    app.right_panel.kb.accurate_configured = accurate_configured;
                    if let (Some((stats, items)), _) = (&snapshot, ()) {
                        let live = app.live_project_paths();
                        app.right_panel.kb.apply_snapshot(stats.clone(), items.clone());
                        app.right_panel.kb.prune_missing_paths(&live);
                        if app.right_panel.kb.pdf_manager_open {
                            app.ensure_pdf_covers_loaded(cx);
                        }
                    }
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Lazily rasterize first-page cover thumbnails in memory for visible PDFs.
    pub(in crate::workspace) fn ensure_pdf_covers_loaded(&mut self, cx: &mut Context<Self>) {
        let Some((project_key, root)) = self.active_rag_root() else {
            return;
        };
        let pending: Vec<(String, String)> = self
            .right_panel
            .kb
            .pdf_items
            .iter()
            .filter(|item| {
                !self.right_panel.kb.pdf_covers.contains_key(&item.content_hash)
                    && !self.right_panel.kb.cover_loading.contains(&item.content_hash)
            })
            .take(16)
            .map(|item| (item.rel_path.clone(), item.content_hash.clone()))
            .collect();
        if pending.is_empty() {
            return;
        }
        for (_, hash) in &pending {
            self.right_panel.kb.cover_loading.insert(hash.clone());
        }

        let task = cx.background_spawn(async move {
            let mut rendered = Vec::with_capacity(pending.len());
            for (rel_path, hash) in pending {
                let abs = root.join(rel_path.replace('/', "\\"));
                let thumb = std::fs::read(&abs).ok().and_then(|bytes| {
                    let doc = gpui_pdf::parse(Arc::new(bytes)).ok()?;
                    let (page_w, _) = gpui_pdf::page_dims(&doc).into_iter().next()?;
                    let scale = (240.0 / page_w.max(1.0)).clamp(0.18, 0.65);
                    gpui_pdf::render_page(&doc, 0, scale).ok()
                });
                rendered.push((hash, thumb));
            }
            rendered
        });

        cx.spawn(async move |this, cx| {
            let rendered: Vec<(String, Option<Arc<RenderImage>>)> = task.await;
            let _ = this.update(cx, |app, cx| {
                if !app
                    .active_rag_root()
                    .is_some_and(|(active_key, _)| active_key == project_key)
                {
                    return;
                }
                for (hash, image) in rendered {
                    app.right_panel.kb.cover_loading.remove(&hash);
                    app.right_panel.kb.pdf_covers.insert(hash, image);
                }
                if app.right_panel.kb.pdf_manager_open {
                    app.ensure_pdf_covers_loaded(cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(in crate::workspace) fn run_kb_sync(
        &mut self,
        force_rebuild: bool,
        accurate_mode: Option<crate::rag::AccuratePdfMode>,
        accurate_paths: Vec<String>,
        cx: &mut Context<Self>,
    ) {
        let Some((project_key, root)) = self.active_rag_root() else {
            return;
        };
        self.rag_sync.pending_deadline = None;
        self.cancel_in_flight_sync();

        let db_path = root.join(".folio").join("rag.sqlite3");
        let config = self.settings.rag.clone();
        let accurate_configured = !ocr::resolve_ocr_token(&config).is_empty();
        self.right_panel.kb.accurate_configured = accurate_configured;
        self.right_panel.kb.sync_error = None;
        let eligible_paths = self.rag_eligible_paths();
        let sync_id = self.rag_sync.next_sync_id.wrapping_add(1);
        self.rag_sync.next_sync_id = sync_id;
        let cancelled = Arc::new(AtomicBool::new(false));
        self.rag_sync.in_flight = Some((sync_id, project_key.clone(), cancelled.clone()));
        cx.notify();

        let task = cx.background_spawn(async move {
            let embed_client = EmbeddingClient::from_config(&config).ok();
            let mut store = RagStore::open(&db_path, &config.embed_model, config.embed_dim)
                .map_err(|e| RagError::new("store_error", e))?;
            let _report = sync_project(
                &root,
                &eligible_paths,
                &mut store,
                SyncOptions {
                    config: &config,
                    embed_client: embed_client.as_ref(),
                    force_rebuild,
                    accurate_mode,
                    accurate_paths: &accurate_paths,
                },
                &cancelled,
            )?;
            let stats = store.stats().map_err(|e| RagError::new("store_error", e))?;
            let mut items = store
                .pdf_library_items()
                .map_err(|e| RagError::new("store_error", e))?;
            ocr::hydrate_pdf_library_items(&root, &mut items);
            Ok::<_, RagError>((stats, items))
        });

        cx.spawn(async move |this, cx| {
            let outcome = task.await;
            let _ = this.update(cx, |app, cx| {
                let is_current = app
                    .rag_sync
                    .in_flight
                    .as_ref()
                    .is_some_and(|(id, k, _)| *id == sync_id && *k == project_key);
                if !is_current {
                    return;
                }
                app.rag_sync.in_flight = None;
                if app
                    .active_rag_root()
                    .is_some_and(|(active_key, _)| active_key == project_key)
                {
                    match outcome {
                        Ok((stats, items)) => {
                            let live = app.live_project_paths();
                            app.right_panel.kb.apply_snapshot(stats, items);
                            app.right_panel.kb.prune_missing_paths(&live);
                            app.right_panel.kb.sync_error = None;
                            if app.right_panel.kb.pdf_manager_open {
                                app.ensure_pdf_covers_loaded(cx);
                            }
                        }
                        Err(err) if err.code == "cancelled" => {}
                        Err(err) => {
                            app.right_panel.kb.sync_error = Some(err.message);
                        }
                    }
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(in crate::workspace) fn run_pdf_ocr_batch(
        &mut self,
        paths: Vec<String>,
        fill_missing_only: bool,
        cx: &mut Context<Self>,
    ) {
        if paths.is_empty() {
            return;
        }
        let Some((project_key, root)) = self.active_rag_root() else {
            return;
        };
        let config = self.settings.rag.clone();
        let accurate_configured = !ocr::resolve_ocr_token(&config).is_empty();
        self.right_panel.kb.accurate_configured = accurate_configured;
        if !accurate_configured {
            self.right_panel.kb.ocr_error = Some(
                "尚未配置 PaddleOCR Token，请在「设置 → API 服务」填写后重试。".to_owned(),
            );
            cx.notify();
            return;
        }

        self.cancel_all_pdf_ocr_silent();
        self.right_panel.kb.ocr_error = None;
        let by_path: HashMap<String, String> = self
            .right_panel
            .kb
            .pdf_items
            .iter()
            .map(|item| (item.rel_path.clone(), item.content_hash.clone()))
            .collect();
        let mut queue: Vec<(String, String, Arc<AtomicBool>)> = Vec::new();
        let mut item_cancels = HashMap::new();

        for rel_path in paths {
            let Some(content_hash) = by_path.get(&rel_path) else {
                continue;
            };
            let cancel = Arc::new(AtomicBool::new(false));
            item_cancels.insert(rel_path.clone(), cancel.clone());
            self.right_panel
                .kb
                .ocr_progress
                .insert(rel_path.clone(), PdfOcrPhase::Queued);
            queue.push((rel_path, content_hash.clone(), cancel));
        }
        if queue.is_empty() {
            return;
        }

        let db_path = root.join(".folio").join("rag.sqlite3");
        let global_cancel = Arc::new(AtomicBool::new(false));
        let global_cancel_worker = global_cancel.clone();
        let (tx, rx) = mpsc::channel();
        let project_key_worker = project_key.clone();

        self.rag_sync.ocr_runner = Some(OcrRunnerHandle {
            events: rx,
            global_cancel,
            item_cancels,
        });
        cx.notify();

        std::thread::spawn(move || {
            let Ok(mut store) = RagStore::open(&db_path, &config.embed_model, config.embed_dim)
            else {
                let _ = tx.send(OcrRunnerEvent::BatchFinished {
                    project_key: project_key_worker,
                });
                return;
            };

            for (rel_path, hash, item_cancel) in queue {
                if global_cancel_worker.load(Ordering::Relaxed)
                    || item_cancel.load(Ordering::Relaxed)
                {
                    let _ = tx.send(OcrRunnerEvent::ItemCancelled { rel_path });
                    continue;
                }

                let outcome = ocr::process_single_pdf_with_progress(
                    &root,
                    &mut store,
                    &config,
                    &rel_path,
                    &hash,
                    fill_missing_only,
                    &item_cancel,
                    &mut |ev| {
                        let _ = tx.send(OcrRunnerEvent::Progress(ev));
                    },
                );

                match outcome {
                    Ok(()) => {
                        let stats = store.stats().ok();
                        let pdf_items = store.pdf_library_items().ok().map(|mut items| {
                            ocr::hydrate_pdf_library_items(&root, &mut items);
                            items
                        });
                        let _ = tx.send(OcrRunnerEvent::ItemDone {
                            rel_path,
                            stats,
                            pdf_items,
                        });
                    }
                    Err(err) if err.code == "cancelled" => {
                        let _ = tx.send(OcrRunnerEvent::ItemCancelled { rel_path });
                    }
                    Err(err) => {
                        let stats = store.stats().ok();
                        let pdf_items = store.pdf_library_items().ok().map(|mut items| {
                            ocr::hydrate_pdf_library_items(&root, &mut items);
                            items
                        });
                        let _ = tx.send(OcrRunnerEvent::ItemFailed {
                            rel_path,
                            message: err.message,
                            stats,
                            pdf_items,
                        });
                    }
                }
            }

            let _ = tx.send(OcrRunnerEvent::BatchFinished {
                project_key: project_key_worker,
            });
        });
    }

    pub(in crate::workspace) fn export_pdf_markdown_item(
        &mut self,
        rel_path: &str,
        content_hash: &str,
        cx: &mut Context<Self>,
    ) {
        let Some((_, root)) = self.active_rag_root() else {
            return;
        };
        match ocr::export_pdf_markdown_to_sibling(&root, rel_path, content_hash) {
            Ok(out_rel) => {
                let _ = self.tree.reload_watched_listing(&root);
                self.schedule_rag_sync();
                self.report(crate::i18n::right_panel::kb_exported_md(&out_rel), cx);
            }
            Err(err) => {
                self.report(err, cx);
            }
        }
    }

    pub(in crate::workspace) fn export_selected_pdf_markdowns(&mut self, cx: &mut Context<Self>) {
        let Some((_, root)) = self.active_rag_root() else {
            return;
        };
        let targets: Vec<(String, String)> = self
            .right_panel
            .kb
            .pdf_items
            .iter()
            .filter(|item| {
                self.right_panel.kb.selected_pdfs.contains(&item.rel_path) && item.is_complete
            })
            .map(|item| (item.rel_path.clone(), item.content_hash.clone()))
            .collect();
        if targets.is_empty() {
            return;
        }
        let mut exported = 0usize;
        let mut last_err: Option<String> = None;
        for (rel_path, hash) in targets {
            match ocr::export_pdf_markdown_to_sibling(&root, &rel_path, &hash) {
                Ok(_) => exported += 1,
                Err(err) => {
                    last_err = Some(err);
                }
            }
        }
        if exported > 0 {
            let _ = self.tree.reload_watched_listing(&root);
            self.schedule_rag_sync();
            self.report(crate::i18n::right_panel::kb_exported_count(exported), cx);
        } else if let Some(err) = last_err {
            self.report(err, cx);
        }
    }

    pub(in crate::workspace) fn disable_kb(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.cancel_rag_sync();
        if let Some((_, root)) = self.active_rag_root() {
            let db = root.join(".folio").join("rag.sqlite3");
            for suffix in ["", "-wal", "-shm"] {
                let path = PathBuf::from(format!("{}{suffix}", db.display()));
                let _ = std::fs::remove_file(path);
            }
        }
        self.right_panel.kb.reset_for_project();
        self.right_panel.kb.flush_pending_clear(window, cx);
        cx.notify();
    }

    pub(in crate::workspace) fn submit_kb_search(&mut self, cx: &mut Context<Self>) {
        let query = self.right_panel.kb.search.read(cx).value().trim().to_owned();
        if query.is_empty() {
            self.right_panel.kb.clear_search_results();
            cx.notify();
            return;
        }
        let Some((project_key, root)) = self.active_rag_root() else {
            return;
        };
        let db_path = root.join(".folio").join("rag.sqlite3");
        if !db_path.exists() {
            return;
        }

        let search_id = self.right_panel.kb.next_search_id();
        self.right_panel.kb.last_query = query.clone();
        self.right_panel.kb.searching = true;
        self.right_panel.kb.search_error = None;
        cx.notify();

        let config = self.settings.rag.clone();
        let dirty_docs = self.dirty_rag_documents();
        let mut cache = self.rag_sync.kb_search_cache.clone();
        let task = cx.background_spawn(async move {
            let store =
                RagStore::open_readonly_for_search(&db_path, &config.embed_model, config.embed_dim)
                    .map_err(|e| RagError::new("store_error", e))?;
            let buckets = cache.search(&store, query, dirty_docs)
                .map_err(|e| RagError::new("store_error", e))?;
            Ok::<_, RagError>((buckets, cache))
        });

        cx.spawn(async move |this, cx| {
            let outcome = task.await;
            let _ = this.update(cx, |app, cx| {
                if !app.right_panel.kb.is_current_search(search_id)
                    || !app
                        .active_rag_root()
                        .is_some_and(|(active_key, _)| active_key == project_key)
                {
                    return;
                }
                app.right_panel.kb.searching = false;
                match outcome {
                    Ok((buckets, cache)) => {
                        let live = app.live_project_paths();
                        app.right_panel.kb.snapshot = (*buckets).clone();
                        app.rag_sync.kb_search_cache = cache;
                        app.right_panel.kb.search_error = None;
                        app.right_panel.kb.prune_missing_paths(&live);
                    }
                    Err(err) => {
                        app.right_panel.kb.search_error = Some(err.message);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(in crate::workspace) fn clear_kb_search(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.right_panel.kb.clear_search_results();
        self.right_panel
            .kb
            .search
            .update(cx, |input, cx| input.set_value("", window, cx));
        cx.notify();
    }

    pub(in crate::workspace) fn open_kb_api_settings(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.preferences.update(cx, |panel, cx| {
            panel.sync(&self.settings, window, cx);
            panel.select_api_tab(cx);
        });
        self.show_preferences = true;
        cx.notify();
    }

    fn poll_ocr_runner(&mut self, cx: &mut Context<Self>) {
        let mut drained = Vec::new();
        let mut finished = false;
        if let Some(handle) = &mut self.rag_sync.ocr_runner {
            while let Ok(ev) = handle.events.try_recv() {
                if matches!(ev, OcrRunnerEvent::BatchFinished { .. }) {
                    finished = true;
                }
                drained.push(ev);
            }
        }
        if finished {
            self.rag_sync.ocr_runner = None;
        }
        let has_events = !drained.is_empty();
        for ev in drained {
            match ev {
                OcrRunnerEvent::Progress(OcrProgressEvent::Started {
                    rel_path,
                    total_pages,
                }) => {
                    self.right_panel.kb.ocr_progress.insert(
                        rel_path,
                        PdfOcrPhase::Running {
                            extracted_pages: 0,
                            total_pages,
                        },
                    );
                }
                OcrRunnerEvent::Progress(OcrProgressEvent::PageProgress {
                    rel_path,
                    extracted_pages,
                    total_pages,
                }) => {
                    self.right_panel.kb.ocr_progress.insert(
                        rel_path,
                        PdfOcrPhase::Running {
                            extracted_pages,
                            total_pages,
                        },
                    );
                }
                OcrRunnerEvent::ItemDone {
                    rel_path,
                    stats,
                    pdf_items,
                } => {
                    self.right_panel.kb.ocr_progress.remove(&rel_path);
                    self.right_panel.kb.selected_pdfs.remove(&rel_path);
                    if let (Some(stats), Some(items)) = (stats, pdf_items) {
                        let live = self.live_project_paths();
                        self.right_panel.kb.apply_snapshot(stats, items);
                        self.right_panel.kb.prune_missing_paths(&live);
                    }
                    self.schedule_rag_sync();
                }
                OcrRunnerEvent::ItemFailed {
                    rel_path,
                    message,
                    stats,
                    pdf_items,
                } => {
                    self.right_panel.kb.ocr_progress.remove(&rel_path);
                    let live = self.live_project_paths();
                    if live.contains(&rel_path) {
                        self.report(message, cx);
                    }
                    if let (Some(stats), Some(items)) = (stats, pdf_items) {
                        self.right_panel.kb.apply_snapshot(stats, items);
                        self.right_panel.kb.prune_missing_paths(&live);
                    }
                }
                OcrRunnerEvent::ItemCancelled { rel_path } => {
                    self.right_panel.kb.ocr_progress.remove(&rel_path);
                }
                OcrRunnerEvent::BatchFinished { project_key } => {
                    self.right_panel.kb.ocr_progress.clear();
                    if self
                        .active_rag_root()
                        .is_some_and(|(active, _)| active == project_key)
                    {
                        self.refresh_kb_snapshot(cx);
                    }
                }
            }
        }
        if has_events || self.rag_sync.ocr_runner.is_some() {
            cx.notify();
        }
    }

    pub(in crate::workspace) fn poll_rag(&mut self, cx: &mut Context<Self>) {
        self.poll_ocr_runner(cx);
        if let Some(deadline) = self.rag_sync.pending_deadline
            && Instant::now() >= deadline
            && self.rag_sync.in_flight.is_none()
        {
            self.trigger_rag_sync_now(cx);
        }
    }
}
