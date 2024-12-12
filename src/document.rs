use std::pin::Pin;

use anyhow::{ensure, Result};
use futures_util::{join, Stream, StreamExt};
use gtk::{
    gio,
    glib::{self, clone},
    prelude::*,
    subclass::prelude::*,
};
use gtk_source::{prelude::*, subclass::prelude::*};

use crate::{cancelled::Cancelled, utils};

/// Unmarks the document as busy on drop.
struct MarkBusyGuard<'a> {
    document: &'a Document,
}

impl Drop for MarkBusyGuard<'_> {
    fn drop(&mut self) {
        self.document.set_busy_progress(1.0);
    }
}

const FILE_IO_PRIORITY: glib::Priority = glib::Priority::DEFAULT_IDLE;
const FILE_SAVER_FLAGS: gtk_source::FileSaverFlags =
    gtk_source::FileSaverFlags::IGNORE_INVALID_CHARS
        .union(gtk_source::FileSaverFlags::IGNORE_MODIFICATION_TIME);

mod imp {
    use std::{
        cell::{Cell, RefCell},
        marker::PhantomData,
    };

    use glib::subclass::Signal;
    use once_cell::sync::Lazy;

    use super::*;

    #[derive(Default, glib::Properties)]
    #[properties(wrapper_type = super::Document)]
    pub struct Document {
        #[property(get = Self::file, set = Self::set_file, explicit_notify)]
        pub(super) file: PhantomData<Option<gio::File>>,
        #[property(get = Self::title)]
        pub(super) title: PhantomData<String>,
        #[property(get = Self::is_modified)]
        pub(super) is_modified: PhantomData<bool>,
        #[property(get, default_value = 1.0, minimum = 0.0, maximum = 1.0)]
        pub(super) busy_progress: Cell<f64>,
        #[property(get)]
        pub(super) is_busy: Cell<bool>,

        pub(super) source_file: gtk_source::File,
        pub(super) load_cancellable: RefCell<Option<gio::Cancellable>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Document {
        const NAME: &'static str = "DelineateDocument";
        type Type = super::Document;
        type ParentType = gtk_source::Buffer;
    }

    #[glib::derived_properties]
    impl ObjectImpl for Document {
        fn constructed(&self) {
            self.parent_constructed();

            let obj = self.obj();
            obj.set_busy_progress(1.0);

            let language_manager = gtk_source::LanguageManager::default();
            if let Some(language) = language_manager.language("dot") {
                obj.set_language(Some(&language));
                obj.set_highlight_syntax(true);
            }

            // FIXME Disable when https://gitlab.gnome.org/World/Rust/sourceview5-rs/-/issues/11 is fixed
            obj.set_highlight_matching_brackets(false);

            obj.connect_loading_notify(clone!(
                #[weak]
                obj,
                move |_| {
                    obj.notify_is_modified();
                }
            ));

            let style_manager = adw::StyleManager::default();
            style_manager.connect_dark_notify(clone!(
                #[weak]
                obj,
                move |_| {
                    obj.update_style_scheme();
                }
            ));

            obj.update_style_scheme();
        }

        fn signals() -> &'static [Signal] {
            static SIGNALS: Lazy<Vec<Signal>> =
                Lazy::new(|| vec![Signal::builder("text-changed").build()]);

            SIGNALS.as_ref()
        }
    }

    impl TextBufferImpl for Document {
        fn modified_changed(&self) {
            self.parent_modified_changed();

            self.obj().notify_is_modified();
        }

        fn insert_text(&self, iter: &mut gtk::TextIter, new_text: &str) {
            self.parent_insert_text(iter, new_text);

            let obj = self.obj();

            if obj.file().is_none() {
                obj.notify_title();
            }

            if !obj.is_loading() {
                obj.emit_text_changed();
            }
        }

        fn delete_range(&self, start: &mut gtk::TextIter, end: &mut gtk::TextIter) {
            self.parent_delete_range(start, end);

            let obj = self.obj();

            if obj.file().is_none() {
                obj.notify_title();
            }

            if !obj.is_loading() {
                obj.emit_text_changed();
            }
        }
    }

    impl BufferImpl for Document {}

    impl Document {
        fn file(&self) -> Option<gio::File> {
            // FIXME mark the binding method nullable upstream
            self.source_file.property("location")
        }

        fn set_file(&self, file: Option<&gio::File>) {
            let obj = self.obj();

            self.source_file.set_location(file);
            obj.notify_file();
        }

        fn title(&self) -> String {
            let obj = self.obj();

            if let Some(file) = obj.file() {
                utils::display_file_stem(&file)
            } else {
                obj.parse_title()
            }
        }

        fn is_modified(&self) -> bool {
            let obj = self.obj();

            // This must not also be loading to be considered modified.
            gtk::TextBuffer::is_modified(obj.upcast_ref()) && !obj.is_loading()
        }
    }
}

glib::wrapper! {
    pub struct Document(ObjectSubclass<imp::Document>)
        @extends gtk::TextBuffer, gtk_source::Buffer;
}

impl Document {
    pub fn new() -> Self {
        glib::Object::new()
    }

    pub fn for_file(file: gio::File) -> Self {
        glib::Object::builder().property("file", file).build()
    }

    pub fn is_safely_discardable(&self) -> bool {
        self.is_draft() && !self.is_modified()
    }

    pub fn is_draft(&self) -> bool {
        self.file().is_none()
    }

    pub fn contents(&self) -> glib::GString {
        self.text(&self.start_iter(), &self.end_iter(), true)
    }

    pub async fn load(&self) -> Result<()> {
        ensure!(!self.is_busy(), "Document must not be busy");
        ensure!(!self.is_draft(), "Document must not be a draft");

        let imp = self.imp();

        let _guard = self.mark_busy();

        let cancellable = gio::Cancellable::new();
        imp.load_cancellable.replace(Some(cancellable.clone()));

        let loader = gtk_source::FileLoader::new(self, &imp.source_file);
        let (io_fut, progress_stream) = loader.load_future(FILE_IO_PRIORITY);
        let (io_res, _) = join!(
            gio::CancellableFuture::new(io_fut, cancellable),
            self.handle_progress_stream(progress_stream)
        );
        io_res.map_err(|_| Cancelled)??;

        self.emit_text_changed();

        Ok(())
    }

    pub async fn save(&self) -> Result<()> {
        ensure!(!self.is_busy(), "Document must not be busy");
        ensure!(!self.is_draft(), "Document must not be a draft");

        let imp = self.imp();

        let _guard = self.mark_busy();

        let saver = gtk_source::FileSaver::builder()
            .buffer(self)
            .file(&imp.source_file)
            .flags(FILE_SAVER_FLAGS)
            .build();
        let (io_fut, progress_stream) = saver.save_future(FILE_IO_PRIORITY);
        let (io_res, _) = join!(io_fut, self.handle_progress_stream(progress_stream));
        io_res?;

        self.set_modified(false);

        Ok(())
    }

    pub async fn save_as(&self, file: &gio::File) -> Result<()> {
        ensure!(!self.is_busy(), "Document must not be busy");

        let imp = self.imp();

        let _guard = self.mark_busy();

        imp.source_file.set_location(Some(file));

        let saver = gtk_source::FileSaver::builder()
            .buffer(self)
            .file(&imp.source_file)
            .flags(FILE_SAVER_FLAGS)
            .build();
        let (io_fut, progress_stream) = saver.save_future(FILE_IO_PRIORITY);
        let (io_res, _) = join!(io_fut, self.handle_progress_stream(progress_stream));
        io_res?;

        self.notify_file();
        self.notify_title();

        self.set_modified(false);

        Ok(())
    }

    pub async fn discard_changes(&self) -> Result<()> {
        ensure!(!self.is_busy(), "Document must not be busy");

        if self.is_draft() {
            let _guard = self.mark_busy();

            self.delete(&mut self.start_iter(), &mut self.end_iter());
            self.set_modified(false);
        } else {
            self.load().await?;
        }

        Ok(())
    }

    fn emit_text_changed(&self) {
        self.emit_by_name::<()>("text-changed", &[]);
    }

    fn set_busy_progress(&self, busy_progress: f64) {
        let imp = self.imp();

        if busy_progress == self.busy_progress() {
            return;
        }

        imp.busy_progress.set(busy_progress);
        self.notify_busy_progress();

        let is_busy = busy_progress != 1.0;

        if is_busy == self.is_busy() {
            return;
        }

        imp.is_busy.set(is_busy);
        self.notify_is_busy();
    }

    fn mark_busy(&self) -> MarkBusyGuard<'_> {
        self.set_busy_progress(0.0);

        MarkBusyGuard { document: self }
    }

    fn parse_title(&self) -> String {
        let start = self.start_iter();

        let mut second_word_end = start;
        second_word_end.forward_word_end();
        second_word_end.forward_word_end();

        let search_flags = gtk::TextSearchFlags::CASE_INSENSITIVE
            | gtk::TextSearchFlags::TEXT_ONLY
            | gtk::TextSearchFlags::VISIBLE_ONLY;

        // Second word is either the `digraph`/`graph` keyword or the title.
        let search_match = start
            .forward_search("digraph", search_flags, Some(&second_word_end))
            .or_else(|| start.forward_search("graph", search_flags, Some(&second_word_end)));

        let Some((match_start, match_end)) = search_match else {
            return "".to_string();
        };

        // `digraph` and `graph` must be a standalone word.
        if !match_start.starts_word() || !match_end.ends_word() {
            return "".to_string();
        }

        let mut title_end = match_end;
        title_end.forward_word_end();

        // If we go forward a word and we go past `{`, title is empty.
        if title_end.backward_search("{", search_flags, None).is_some() {
            return "".to_string();
        }

        let mut title_start = title_end;
        title_start.backward_word_start();

        // If we go back a word and it's `digraph`/`graph`, title is empty.
        if title_start == match_start {
            return "".to_string();
        }

        title_start.visible_text(&title_end).to_string()
    }

    async fn handle_progress_stream(
        &self,
        mut progress_stream: Pin<Box<dyn Stream<Item = (i64, i64)>>>,
    ) {
        while let Some((current_n_bytes, total_n_bytes)) = progress_stream.next().await {
            let progress = if total_n_bytes == 0 || current_n_bytes > total_n_bytes {
                1.0
            } else {
                current_n_bytes as f64 / total_n_bytes as f64
            };
            self.set_busy_progress(progress);
        }
    }

    fn update_style_scheme(&self) {
        let style_manager = adw::StyleManager::default();
        let style_scheme_manager = gtk_source::StyleSchemeManager::default();

        let style_scheme = if style_manager.is_dark() {
            style_scheme_manager
                .scheme("Adwaita-dark")
                .or_else(|| style_scheme_manager.scheme("classic-dark"))
        } else {
            style_scheme_manager
                .scheme("Adwaita")
                .or_else(|| style_scheme_manager.scheme("classic"))
        };

        self.set_style_scheme(style_scheme.as_ref());
    }
}
