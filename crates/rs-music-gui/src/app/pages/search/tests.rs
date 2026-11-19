use crate::i18n::Language;
use rs_music_storage::DownloadSettings;

use super::view::{search_enter, show_search_toolbar};
use super::*;
use std::time::Duration;

#[test]
fn enter_submits_search_without_intercepting_ime_confirmation() {
    let enter = egui::Event::Key {
        key: egui::Key::Enter,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    let mut composing = false;
    assert!(search_enter(std::slice::from_ref(&enter), &mut composing));
    assert!(!search_enter(
        &[egui::Event::Ime(egui::ImeEvent::Preedit("zhong".into()))],
        &mut composing
    ));
    assert!(!search_enter(std::slice::from_ref(&enter), &mut composing));
    assert!(!search_enter(
        &[
            enter.clone(),
            egui::Event::Ime(egui::ImeEvent::Commit("中".into())),
        ],
        &mut composing
    ));
    assert!(search_enter(&[enter], &mut composing));
}

fn script(name: &str, platform: &str, handler: &str) -> SavedSource {
    SavedSource {
        name: name.into(),
        script: true,
        contents: format!(
            "/** @name {name} */
                lx.on('request', async ({{ info }}) => {{ {handler} }});
                lx.send('inited', {{ sources: {{
                    {platform}: {{ type: 'music', actions: ['musicUrl'], qualitys: ['128k'] }}
                }} }});"
        ),
        platform: None,
    }
}

#[test]
fn saved_default_survives_failed_initialization_and_can_be_replaced() {
    let ctx = egui::Context::default();
    let working = script("Working source", "kg", "return undefined;");
    let mut unavailable = script("Unavailable source", "kg", "return undefined;");
    unavailable.contents = "/** @name Unavailable source */ throw Error('offline');".into();
    let configured = SourceSelection {
        source: Some(1),
        platform: "kg".into(),
        quality: "320k".into(),
        ..SourceSelection::default()
    };
    let state = rs_music_storage::State {
        sources: vec![working, unavailable],
        source_selection: Some(configured.clone()),
        ..rs_music_storage::State::default()
    };
    let mut app = MusicApp::from_state(&ctx, state, None, None);
    let finish_loading = |app: &mut MusicApp| {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while app.hall.importing.is_some() {
            assert!(std::time::Instant::now() < deadline);
            poll(app, &ctx);
            std::thread::sleep(Duration::from_millis(1));
        }
    };

    assert!(app.hall.selection() == configured);
    finish_loading(&mut app);
    assert!(matches!(app.hall.source, Source::ITunes));
    assert!(matches!(app.hall.error, Some((Message::SourceFailed, _))));
    assert!(app.hall.selection() == configured);

    app.hall.load_saved(0, false, &ctx);
    assert_eq!(app.hall.selection().source, Some(0));
    finish_loading(&mut app);
    assert_eq!(app.hall.source.name(), "Working source");
    assert_eq!(app.hall.selection().source, Some(0));
    assert_eq!(app.hall.selection().platform, "kg");
    assert_eq!(app.hall.selection().quality, "128k");
}

#[test]
fn source_list_switches_directly_between_saved_and_builtin_sources() {
    let ctx = egui::Context::default();
    let state = rs_music_storage::State {
        sources: vec![script("Example", "kg", "return undefined;")],
        source_selection: Some(SourceSelection::default()),
        ..Default::default()
    };
    let mut app = MusicApp::from_state(&ctx, state, None, None);
    let click = |app: &mut MusicApp, label: &str| {
        let mut frame = |events| {
            ctx.run(
                egui::RawInput {
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| show_source_settings(app, ui));
                },
            )
        };
        let output = frame(vec![]);
        let pos = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.job.text == label => {
                    Some(text.galley.rect.translate(text.pos.to_vec2()).center())
                }
                _ => None,
            })
            .expect("source is visible without opening a menu");
        let button = |pressed| egui::Event::PointerButton {
            pos,
            pressed,
            button: egui::PointerButton::Primary,
            modifiers: egui::Modifiers::NONE,
        };
        frame(vec![egui::Event::PointerMoved(pos), button(true)]);
        frame(vec![button(false)]);
    };
    click(&mut app, "Example (LX)");
    assert_eq!(app.hall.selection().source, Some(0));
    click(&mut app, "iTunes");
    assert_eq!(app.hall.selection().source, None);
    assert!(app.hall.importing.is_none());
}

#[test]
fn enlarged_search_field_leaves_room_for_controls_in_a_narrow_panel() {
    let ctx = egui::Context::default();
    let mut hall = Hall::new(Vec::new(), Some(SourceSelection::default()), true, &ctx);
    let source = script("Search toolbar", "kg", "return undefined;");
    hall.select(Source::from_document(true, &source.contents).unwrap());

    let _ = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(540.0, 480.0),
            )),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let panel = ui.available_rect_before_wrap();
                show_search_toolbar(&mut hall, Language::English, ui);
                let field = ctx
                    .read_response(egui::Id::new("online_search_box"))
                    .unwrap();
                assert!(panel.contains_rect(field.rect));
                assert!(field.rect.height() >= 44.0);
                assert!(field.rect.width() > panel.width() / 2.0);
                assert!(panel.right() - field.rect.right() >= 112.0);
            });
        },
    );
}

#[test]
fn platform_search_results_can_use_lx_fallback_with_a_non_lx_primary() {
    let ctx = egui::Context::default();
    let mut hall = Hall::new(
        vec![script("backup", "kg", "return undefined;")],
        Some(SourceSelection::default()),
        true,
        &ctx,
    );
    hall.select_platform("kg", &ctx);
    let mut track = Track::default();
    assert!(!hall.can_play_search_result(&track));
    track.id = Some("test-hash".into());
    assert!(hall.can_play_search_result(&track));
    let online = hall.online_track(track.clone());
    assert!(online.source.is_none());
    hall.start_platform_playback(
        online,
        &DownloadSettings {
            lyrics: false,
            ..DownloadSettings::default()
        },
        &ctx,
    );
    let error = hall
        .download
        .take()
        .unwrap()
        .task
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .err()
        .expect("the test script deliberately returns no URL");
    assert!(
        error.starts_with("backup: LX musicUrl rejected (kg, 128k)"),
        "{error}"
    );

    hall.auto_switch_source = false;
    assert!(!hall.can_play_search_result(&track));
    track.audio_url = Some("https://example.org/song.mp3".parse().unwrap());
    assert!(hall.can_play_search_result(&track));
}

#[test]
fn tries_saved_sources_in_order_when_primary_does_not_support_kugou() {
    let earlier = script(
        "earlier",
        "kg",
        "throw Error('track ' + info.musicInfo.hash + ' at ' + info.type);",
    );
    let primary = script("primary", "wy", "return undefined;");
    let later = script("later", "kg", "return undefined;");
    let source = Source::from_document(true, &primary.contents).unwrap();
    let ctx = egui::Context::default();
    let mut hall = Hall::new(
        vec![earlier, primary, later],
        Some(SourceSelection::default()),
        true,
        &ctx,
    );
    hall.select(source);
    hall.source_index = Some(1);
    hall.select_platform("kg", &ctx);
    assert!(hall.can_play_lx("kg"));
    assert_eq!(hall.catalog().unwrap().platform(), Some("kg"));
    let mut track = Track::default();
    track.id = Some("test-hash".into());
    hall.start_platform_playback(
        hall.online_track(track.clone()),
        &DownloadSettings {
            lyrics: false,
            ..DownloadSettings::default()
        },
        &ctx,
    );
    let error = hall
        .download
        .as_ref()
        .unwrap()
        .task
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .err()
        .expect("all sources failed");
    assert!(
        error.find("later:").unwrap() < error.find("earlier:").unwrap(),
        "{error}"
    );
    assert!(error.contains("track test-hash at 128k"), "{error}");
    assert_eq!(hall.selection().source, Some(1));

    hall.source_successes
        .insert(hall.saved[0].fingerprint(), 12);
    hall.source_successes
        .insert(hall.saved[1].fingerprint(), 30);
    hall.source_successes.insert(hall.saved[2].fingerprint(), 3);
    hall.start_platform_playback(
        hall.online_track(track.clone()),
        &DownloadSettings {
            lyrics: false,
            ..DownloadSettings::default()
        },
        &ctx,
    );
    let error = hall
        .download
        .as_ref()
        .unwrap()
        .task
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .err()
        .expect("all sources failed");
    assert!(error.starts_with("primary:"), "{error}");
    assert!(
        error.find("earlier:").unwrap() < error.find("later:").unwrap(),
        "{error}"
    );

    hall.auto_switch_source = false;
    assert!(!hall.can_play_lx("kg"));
    hall.start_platform_playback(
        hall.online_track(track),
        &DownloadSettings {
            lyrics: false,
            ..DownloadSettings::default()
        },
        &ctx,
    );
    let error = hall
        .download
        .as_ref()
        .unwrap()
        .task
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .err()
        .expect("primary source failed");
    assert_eq!(error, "primary: Unsupported LX platform or quality");
}

#[test]
fn deleting_sources_reindexes_selection_and_restores_builtin_search() {
    let ctx = egui::Context::default();
    let unused = script("unused", "wy", "return undefined;");
    let source = script("player", "kg", "return undefined;");
    let catalog = SavedSource {
        name: "custom".into(),
        script: false,
        platform: Some("kg".into()),
        contents: r#"{
                "name":"custom", "platform":"kg", "search_url":"https://example.org",
                "query_parameter":"q", "results":"/tracks", "title":"/name",
                "artist":"/artist", "track_id":"/hash"
            }"#
        .into(),
    };
    let mut hall = Hall::new(
        vec![unused, source.clone(), catalog.clone()],
        Some(SourceSelection::default()),
        true,
        &ctx,
    );
    hall.select(Source::from_document(true, &source.contents).unwrap());
    hall.source_index = Some(1);
    hall.catalog = Some(Source::from_document(false, &catalog.contents).unwrap());
    hall.catalog_index = Some(2);

    hall.delete_saved(0, &ctx);
    assert_eq!(hall.selection().source, Some(0));
    assert_eq!(hall.selection().catalog, Some(1));
    hall.delete_saved(1, &ctx);
    assert_eq!(hall.selection().catalog, None);
    assert_eq!(hall.catalog().unwrap().platform(), Some("kg"));
    hall.delete_saved(0, &ctx);
    assert!(hall.saved.is_empty());
    assert_eq!(hall.selection().source, None);
    assert!(matches!(hall.source, Source::ITunes));
}
