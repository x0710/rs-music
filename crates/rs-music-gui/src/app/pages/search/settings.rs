use super::{Hall, sources::load_document};
use crate::{
    app::{Message, MusicApp, tasks::spawn},
    i18n::Language,
};
use eframe::egui;
use rs_music_core::Source;

pub(in crate::app) fn show_source_settings(app: &mut MusicApp, ui: &mut egui::Ui) {
    let language = app.language();
    let hall = &mut app.hall;
    let before = hall.selection();

    let mut changed = ui
        .checkbox(
            &mut hall.auto_switch_source,
            language.text(Message::AutoSwitchSource),
        )
        .changed();
    ui.add_space(8.0);
    ui.horizontal_wrapped(|ui| {
        if ui
            .add_enabled(
                hall.importing.is_none(),
                egui::Button::new(language.text(Message::ImportSource)),
            )
            .clicked()
        {
            if let Some(path) = rfd::FileDialog::new()
                .add_filter("LX JavaScript", &["js"])
                .add_filter("JSON / LX export", &["json"])
                .pick_file()
            {
                tracing::info!("Source file selected, starting import");
                hall.error = None;
                hall.importing = Some(spawn(ui.ctx(), move || load_document(&path)));
            } else {
                tracing::info!("Source file picker returned no file");
            }
        }
        if hall.importing.is_some() {
            ui.spinner();
        }
    });

    ui.add_space(8.0);
    ui.label(language.text(Message::Source));
    if ui.radio(before.source.is_none(), "iTunes").clicked() {
        hall.importing = None;
        hall.select(Source::ITunes);
        hall.search_first_page(ui.ctx());
    }
    if hall.saved.is_empty() {
        ui.label(language.text(Message::NoImportedSources));
    }
    let mut selected = None;
    let mut deleted = None;
    for (index, source) in hall.saved.iter().enumerate() {
        let kind = if source.script { "LX" } else { "JSON" };
        ui.horizontal_wrapped(|ui| {
            if ui
                .radio(
                    before.source == Some(index),
                    format!("{} ({kind})", source.name),
                )
                .clicked()
            {
                selected = Some(index);
            }
            if hall.source_index == Some(index) {
                ui.label(language.text(Message::ActiveSource));
            }
            if hall.catalog_index == Some(index) {
                ui.label(language.text(Message::ActiveCatalog));
            }
            if ui
                .add_enabled(
                    hall.importing.is_none(),
                    egui::Button::new(language.text(Message::DeleteSource)),
                )
                .clicked()
            {
                deleted = Some(index);
            }
        });
    }
    if let Some(index) = selected {
        hall.load_saved(index, false, ui.ctx());
    }
    if let Some(index) = deleted {
        hall.delete_saved(index, ui.ctx());
        changed = true;
    }

    if matches!(hall.source, Source::Lx(_)) {
        ui.add_space(8.0);
        ui.separator();
        ui.add_space(8.0);
        ui.add_enabled_ui(hall.importing.is_none(), |ui| {
            show_catalog_settings(hall, language, ui);
        });
    }
    if let Some((Message::SourceFailed, error)) = &hall.error {
        ui.colored_label(
            ui.visuals().error_fg_color,
            language.text(Message::SourceFailed),
        )
        .on_hover_text(error);
    }
    app.dirty |= changed || before != hall.selection();
}

fn show_catalog_settings(hall: &mut Hall, language: Language, ui: &mut egui::Ui) {
    let builtin_catalog = Source::builtin_catalog(&hall.platform);
    ui.horizontal_wrapped(|ui| {
        ui.label(format!(
            "{} · {}",
            language.text(Message::Platform),
            hall.platform
        ));
        let mut selected = None;
        let mut select_builtin = false;
        let has_saved = hall
            .saved
            .iter()
            .any(|source| source.platform.as_deref() == Some(&hall.platform));
        let catalog = hall
            .catalog
            .as_ref()
            .filter(|source| source.platform() == Some(&hall.platform))
            .or(builtin_catalog.as_ref());
        if has_saved || catalog.is_some() {
            egui::ComboBox::from_label(language.text(Message::SearchCatalog))
                .selected_text(
                    catalog.map_or(language.text(Message::NoSearchCatalog), Source::name),
                )
                .show_ui(ui, |ui| {
                    if let Some(source) = &builtin_catalog
                        && ui.button(source.name()).clicked()
                    {
                        select_builtin = true;
                        ui.close();
                    }
                    for (index, source) in hall.saved.iter().enumerate() {
                        if source.platform.as_deref() == Some(&hall.platform)
                            && ui.button(&source.name).clicked()
                        {
                            selected = Some(index);
                            ui.close();
                        }
                    }
                });
        }
        if select_builtin {
            hall.catalog = None;
            hall.catalog_index = None;
            hall.search_first_page(ui.ctx());
        }
        if let Some(index) = selected {
            hall.load_saved(index, true, ui.ctx());
        }
        if ui
            .add_enabled(
                hall.importing.is_none(),
                egui::Button::new(language.text(Message::ImportCatalog)),
            )
            .clicked()
            && let Some(path) = rfd::FileDialog::new()
                .add_filter("JSON", &["json"])
                .pick_file()
        {
            let platform = hall.platform.clone();
            hall.importing = Some(spawn(ui.ctx(), move || {
                let result = load_document(&path)?;
                if result.0.platform() != Some(&platform) {
                    return Err(format!(
                        "Expected a JSON search catalog with platform: {platform}"
                    ));
                }
                Ok(result)
            }));
        }
    });
}
