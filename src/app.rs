//! The egui user interface.

use crate::booster::{self, Finish, Item, Pack, PackCard, SetData};
use crate::definition::{self, Definition, Source};
use crate::scryfall::{self, SetInfo};
use eframe::egui::{self, Color32, RichText, Vec2};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

const CARD_SIZE: Vec2 = Vec2::new(200.0, 279.0);
const PREVIEW_SIZE: Vec2 = Vec2::new(360.0, 502.0);
/// Set types that are sold in randomized boosters.
const BOOSTER_SET_TYPES: [&str; 4] = ["expansion", "core", "masters", "draft_innovation"];
/// Sets of those types that were never sold as a booster product of their
/// own, or only in one-off formats this app does not simulate.
const EXCLUDED_SETS: [&str; 8] = ["plst", "slx", "h1r", "h2r", "clu", "mb2", "mbc", "acr"];

enum Msg {
    Sets(Result<Vec<SetInfo>, String>),
    Pack(Result<(Arc<SetData>, Pack), String>),
}

pub struct App {
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    all_sets: Arc<Vec<SetInfo>>,
    /// Sets offered in the picker, newest first.
    sets: Vec<SetInfo>,
    filter: String,
    selected: Option<SetInfo>,
    defs: Arc<Vec<Definition>>,
    /// Index of the chosen booster among the selected set's options.
    choice: usize,
    cache: HashMap<String, Arc<SetData>>,
    pack: Option<Pack>,
    busy: bool,
    loading_sets: bool,
    error: Option<String>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        // Decoding comes from egui_extras; fetching goes through the disk cache.
        egui_extras::install_image_loaders(&cc.egui_ctx);
        cc.egui_ctx.add_bytes_loader(Arc::new(crate::images::CachedImageLoader::default()));
        let (tx, rx) = channel();
        let (defs, defs_error) = match definition::load() {
            Ok(defs) => (defs, None),
            Err(e) => {
                log::error!("Booster definitions: {e}");
                (Vec::new(), Some(format!("Booster definitions: {e}")))
            }
        };
        let mut app = App {
            tx,
            rx,
            all_sets: Arc::default(),
            sets: Vec::new(),
            filter: String::new(),
            selected: None,
            defs: Arc::new(defs),
            choice: 0,
            cache: HashMap::new(),
            pack: None,
            busy: false,
            loading_sets: false,
            error: None,
        };
        app.load_sets(&cc.egui_ctx);
        app.error = defs_error;
        app
    }

    fn load_sets(&mut self, ctx: &egui::Context) {
        self.loading_sets = true;
        self.error = None;
        let (tx, ctx) = (self.tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(Msg::Sets(scryfall::fetch_sets()));
            ctx.request_repaint();
        });
    }

    fn open_booster(&mut self, ctx: &egui::Context) {
        let Some(info) = self.selected.clone() else { return };
        let options = definition::options_for(&self.defs, &info);
        let Some(def) = options.get(self.choice).map(|d| (*d).clone()) else { return };
        let with_guests = options.iter().any(|d| d.uses(Source::SpecialGuest));
        self.busy = true;
        self.error = None;
        let cached = self.cache.get(&info.code).cloned();
        log::info!(
            "Opening a {} for {} ({}){}",
            def.name,
            info.name,
            info.code,
            if cached.is_some() { ", card data already in memory" } else { "" }
        );
        let (tx, ctx, all) = (self.tx.clone(), ctx.clone(), self.all_sets.clone());
        std::thread::spawn(move || {
            let result = match cached {
                Some(data) => Ok(data),
                None => booster::load_set(&info, &all, with_guests).map(Arc::new),
            }
            .map(|data| {
                let list_card = || scryfall::random("e:plst").ok().flatten();
                let pack = booster::generate(&data, &def, &list_card);
                (data, pack)
            });
            let _ = tx.send(Msg::Pack(result));
            ctx.request_repaint();
        });
    }

    fn handle_messages(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::Sets(Ok(all)) => {
                    self.loading_sets = false;
                    log::info!("Scryfall lists {} sets", all.len());
                    let mut sets: Vec<SetInfo> = all
                        .iter()
                        .filter(|s| {
                            !s.digital
                                && s.card_count > 0
                                && BOOSTER_SET_TYPES.contains(&s.set_type.as_str())
                                && !EXCLUDED_SETS.contains(&s.code.as_str())
                        })
                        .cloned()
                        .collect();
                    sets.sort_by(|a, b| b.released_at.cmp(&a.released_at));
                    log::debug!("{} of them are sold in boosters and shown in the picker", sets.len());
                    self.sets = sets;
                    self.all_sets = Arc::new(all);
                }
                Msg::Sets(Err(e)) => {
                    log::error!("Loading the set list failed: {e}");
                    self.loading_sets = false;
                    self.error = Some(e);
                }
                Msg::Pack(Ok((data, pack))) => {
                    self.busy = false;
                    self.cache.insert(data.info.code.clone(), data);
                    self.pack = Some(pack);
                }
                Msg::Pack(Err(e)) => {
                    log::error!("Opening the booster failed: {e}");
                    self.busy = false;
                    self.error = Some(e);
                }
            }
        }
    }

    fn set_picker(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        ui.heading("1. Choose an expansion");
        ui.add_space(4.0);
        ui.add(
            egui::TextEdit::singleline(&mut self.filter)
                .hint_text("Search by name or code…")
                .desired_width(f32::INFINITY),
        );
        ui.add_space(4.0);

        if self.loading_sets {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Loading sets from Scryfall…");
            });
            return;
        }
        if self.sets.is_empty() {
            if ui.button("Retry loading sets").clicked() {
                self.load_sets(ui.ctx());
            }
            return;
        }

        let needle = self.filter.trim().to_lowercase();
        let mut clicked = None;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            for set in &self.sets {
                if !needle.is_empty()
                    && !set.name.to_lowercase().contains(&needle)
                    && set.code != needle
                {
                    continue;
                }
                let year = set.released_at.as_deref().map_or("", |d| &d[..d.len().min(4)]);
                let text = format!("{}  ·  {} {}", set.name, set.code.to_uppercase(), year);
                let is_selected = self.selected.as_ref().is_some_and(|s| s.code == set.code);
                if ui.selectable_label(is_selected, text).clicked() && !is_selected {
                    clicked = Some(set.clone());
                }
            }
        });
        if let Some(set) = clicked {
            log::info!("Selected {} ({}), released {:?}", set.name, set.code, set.released_at);
            self.choice = 0;
            self.selected = Some(set);
        }
    }

    fn controls(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        let Some(set) = self.selected.clone() else {
            ui.heading("Pick an expansion on the left to get started.");
            ui.add_space(6.0);
            return;
        };
        ui.heading(format!("2. Choose a booster for {}", set.name));
        ui.add_space(4.0);
        let defs = self.defs.clone();
        let options = definition::options_for(&defs, &set);
        if options.is_empty() {
            ui.label("No booster definition covers this set.");
            ui.add_space(6.0);
            return;
        }
        ui.horizontal(|ui| {
            for (i, def) in options.iter().enumerate() {
                let hover = [def.description.clone(), def.availability_text()]
                    .into_iter()
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
                    .join("\n");
                ui.radio_value(&mut self.choice, i, &def.name).on_hover_text(hover);
            }
            ui.add_space(12.0);
            let button = egui::Button::new(RichText::new("Open booster").strong());
            if ui.add_enabled(!self.busy, button).clicked() {
                self.open_booster(ui.ctx());
            }
            if self.busy {
                ui.spinner();
                let cached = self.cache.contains_key(&set.code);
                ui.label(if cached { "Opening…" } else { "Fetching card list from Scryfall…" });
            }
        });
        if let Some(err) = &self.error {
            ui.colored_label(Color32::from_rgb(230, 90, 90), err);
        }
        ui.add_space(6.0);
    }

    fn pack_view(&self, ui: &mut egui::Ui) {
        let Some(pack) = &self.pack else {
            ui.centered_and_justified(|ui| {
                ui.label(RichText::new("No booster opened yet.").weak());
            });
            return;
        };
        ui.horizontal(|ui| {
            ui.strong(format!("{} — {}", pack.set_name, pack.booster_name));
            ui.label(format!("{} cards", pack.cards.len()));
            ui.label(format!("Estimated value ${:.2}", pack.total_price()));
        });
        ui.add_space(4.0);
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = Vec2::new(10.0, 12.0);
                for card in &pack.cards {
                    card_view(ui, card);
                }
            });
        });
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.handle_messages();
        egui::Panel::left("sets")
            .resizable(true)
            .default_size(300.0)
            .show(ui, |ui| self.set_picker(ui));
        egui::Panel::top("controls").show(ui, |ui| self.controls(ui));
        egui::CentralPanel::default().show(ui, |ui| self.pack_view(ui));
    }
}

fn rarity_color(rarity: &str) -> Color32 {
    match rarity {
        "uncommon" => Color32::from_rgb(170, 190, 205),
        "rare" => Color32::from_rgb(214, 180, 90),
        "mythic" => Color32::from_rgb(235, 120, 50),
        _ => Color32::GRAY,
    }
}

fn card_view(ui: &mut egui::Ui, pc: &PackCard) {
    let foil = pc.finish != Finish::Nonfoil;
    ui.allocate_ui(CARD_SIZE + Vec2::new(0.0, 56.0), |ui| {
        ui.vertical(|ui| {
            ui.set_width(CARD_SIZE.x);
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.label(RichText::new(&pc.slot).small().weak());

            let (rect, response) = ui.allocate_exact_size(CARD_SIZE, egui::Sense::click());
            let url = match &pc.item {
                Item::Card(c) => c.image_url(),
                Item::Placeholder(_) => None,
            };
            let painter = ui.painter_at(rect);
            match url {
                Some(url) => {
                    egui::Image::new(url).corner_radius(9).paint_at(ui, rect);
                }
                None => {
                    painter.rect_filled(rect, 9, Color32::from_gray(45));
                    painter.text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        pc.name(),
                        egui::FontId::proportional(16.0),
                        Color32::from_gray(170),
                    );
                }
            }
            if foil {
                foil_overlay(&painter, rect);
            }

            let color = match &pc.item {
                Item::Card(c) => rarity_color(&c.rarity),
                Item::Placeholder(_) => Color32::GRAY,
            };
            ui.add(egui::Label::new(RichText::new(pc.name()).color(color)).truncate());
            let finish = match pc.finish {
                Finish::Nonfoil => "",
                Finish::Foil => "Foil  ",
                Finish::Etched => "Foil-etched  ",
            };
            let price = pc.price().map(|p| format!("${p:.2}")).unwrap_or_default();
            ui.label(RichText::new(format!("{finish}{price}")).small());

            if let Item::Card(c) = &pc.item {
                let mut response = response;
                if let Some(url) = url {
                    response = response.on_hover_ui(|ui| {
                        ui.add(egui::Image::new(url).fit_to_exact_size(PREVIEW_SIZE).corner_radius(16));
                        ui.strong(&c.name);
                        if let Some(t) = &c.type_line {
                            ui.label(t);
                        }
                        ui.label(
                            RichText::new(format!(
                                "{} #{} · {}",
                                c.set.to_uppercase(),
                                c.collector_number,
                                c.rarity
                            ))
                            .weak(),
                        );
                        if c.scryfall_uri.is_some() {
                            ui.label(RichText::new("Click to open on Scryfall").small().weak());
                        }
                    });
                }
                // Clicking a card opens its Scryfall page in the browser.
                if let Some(page) = &c.scryfall_uri {
                    if response.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                        log::info!("Opening {} on Scryfall: {page}", c.name);
                        ui.ctx().open_url(egui::OpenUrl::new_tab(page));
                    }
                }
            }
        });
    });
}

/// A translucent rainbow sheen and badge marking foil cards.
fn foil_overlay(painter: &egui::Painter, rect: egui::Rect) {
    let tint = |r, g, b| Color32::from_rgba_unmultiplied(r, g, b, 70);
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(rect.left_top(), tint(255, 60, 200));
    mesh.colored_vertex(rect.right_top(), tint(60, 220, 255));
    mesh.colored_vertex(rect.right_bottom(), tint(255, 230, 80));
    mesh.colored_vertex(rect.left_bottom(), tint(90, 255, 150));
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    painter.add(egui::Shape::mesh(mesh));

    let badge = egui::Rect::from_min_size(
        rect.left_bottom() + Vec2::new(8.0, -28.0),
        Vec2::new(44.0, 20.0),
    );
    painter.rect_filled(badge, 6, Color32::from_black_alpha(200));
    painter.text(
        badge.center(),
        egui::Align2::CENTER_CENTER,
        "FOIL",
        egui::FontId::proportional(12.0),
        Color32::from_rgb(255, 225, 120),
    );
}
