#![allow(clippy::new_without_default)]
#![warn(
    rust_2018_idioms,
    clippy::items_after_statements,
    clippy::needless_pass_by_value,
    clippy::semicolon_if_nothing_returned,
    clippy::match_wildcard_for_single_variants,
    clippy::inefficient_to_string,
    clippy::map_unwrap_or,
    clippy::implicit_clone,
    clippy::struct_excessive_bools,
    clippy::unreadable_literal,
    clippy::if_not_else,
    clippy::doc_markdown,
    clippy::unused_async,
    clippy::default_trait_access,
    clippy::unnecessary_wraps,
    clippy::unused_self,
    clippy::dbg_macro,
    clippy::todo,
    clippy::map_unwrap_or,
    clippy::or_fun_call,
    clippy::print_stdout
)]

mod about;
mod application;
mod cancelled;
mod config;
mod document;
mod drag_overlay;
mod error_gutter_renderer;
mod export_format;
mod graph_view;
mod i18n;
mod page;
mod recent_filter;
mod recent_item;
mod recent_list;
mod recent_popover;
mod recent_row;
mod recent_sorter;
mod save_changes_dialog;
mod session;
mod utils;
mod window;

use std::{fs, path::PathBuf};

use gettextrs::LocaleCategory;
use gtk::{gio, glib};
use once_cell::sync::Lazy;

use self::{
    application::Application,
    config::{APP_ID, GETTEXT_PACKAGE, LOCALEDIR, RESOURCES_FILE},
};

static APP_DATA_DIR: Lazy<PathBuf> = Lazy::new(|| {
    let mut path = glib::user_data_dir();
    path.push(APP_ID);
    path
});

fn main() -> glib::ExitCode {
    tracing_subscriber::fmt::init();

    gtk::init().unwrap();
    gtk_source::init();

    gettextrs::setlocale(LocaleCategory::LcAll, "");
    gettextrs::bindtextdomain(GETTEXT_PACKAGE, LOCALEDIR).expect("Unable to bind the text domain");
    gettextrs::textdomain(GETTEXT_PACKAGE).expect("Unable to switch to the text domain");

    glib::set_application_name(&utils::application_name());

    let res = gio::Resource::load(RESOURCES_FILE).expect("Could not load gresource file");
    gio::resources_register(&res);

    fs::create_dir_all(APP_DATA_DIR.as_path()).unwrap();

    let app = Application::new();
    app.run()
}
