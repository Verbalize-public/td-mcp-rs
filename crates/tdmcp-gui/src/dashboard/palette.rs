//! Palette page — the `.tox` component library as a browsable tree.
//!
//! Left: categories and components, each with the thumbnail rendered during a
//! probe. Right: the selected component's preview, interface, card, and the
//! copy actions that hand it to an agent. State, DTOs and the background jobs
//! live in [`crate::palette`]; this module is only pixels.

use eframe::egui::{self, Color32};

use super::widgets::card_with_header;
use crate::app::{DashboardApp, SnackTone};
use crate::palette::{
    self as pal, AnalyseState, PaletteRow, RowState, StatusFilter, Step, StepState,
};
use crate::theme::{
    action_button, badge, chip, empty_state, font_label, font_meta, font_mono, font_title,
    ghost_button, sp, status_led, ActionTone, BadgeKind, ACCENT, BG_ACTIVE, BG_HOVER, BG_ROW,
    BORDER, ERR, OK, RADIUS_SM, TEXT, TEXT_DIM, TEXT_FAINT, WARN,
};

/// Tree column width (px). Wide enough for `cartesianToPolar` plus its dot.
const TREE_W: f32 = 316.0;
/// Uniform row height for the virtualized tree (headers and items alike).
const PROW_H: f32 = 28.0;
/// Thumbnail edge in a tree row (px).
const TILE: f32 = 20.0;
/// Preview edge in the detail pane (px).
const PREVIEW: f32 = 128.0;
/// Left inset shared by every tree row.
const INSET: f32 = 10.0;
/// Right inset for a tree row's trailing mark — clears the scrollbar gutter.
const RIGHT_INSET: f32 = 18.0;

/// One line of the flattened tree — the shape `ScrollArea::show_rows` needs.
enum Node {
    Group {
        name: String,
        count: usize,
        collapsed: bool,
    },
    Item(usize),
}

/// Deferred mutations. Drawing holds `&mut app` through closures, so clicks are
/// recorded here and applied once the borrows are released — the same ferry
/// idiom the Overview activity card uses for click-to-copy.
#[derive(Default)]
struct Actions {
    select: Option<String>,
    toggle_group: Option<String>,
    copy: Option<(String, &'static str)>,
    reveal: Option<String>,
    set_ignored: Option<(String, bool)>,
    rescan: bool,
    open_analyse: bool,
}

pub(crate) fn palette(app: &mut DashboardApp, ui: &mut egui::Ui) {
    // First paint of the tab pulls the roster; after that it is only refreshed
    // by an explicit action, so browsing costs no HTTP at all.
    if !app.palette.loaded && app.palette.job.is_none() {
        pal::load_roster(app);
    }

    let mut act = Actions::default();
    toolbar(app, ui, &mut act);
    ui.add_space(sp::SM);

    // With no roster there is nothing to select, so the split would be two
    // empty panels arguing with each other. One guidance block instead.
    if app.palette.rows.is_empty() {
        empty_page(app, ui, &mut act);
        apply(app, ui.ctx().clone(), act);
        return;
    }

    let full = ui.available_rect_before_wrap();
    let status_h = 20.0;
    let body = egui::Rect::from_min_max(
        full.min,
        egui::pos2(full.max.x, (full.max.y - status_h - sp::SM).max(full.min.y)),
    );
    let split = body.min.x + TREE_W;

    let tree_rect = egui::Rect::from_min_max(body.min, egui::pos2(split, body.max.y));
    let detail_rect = egui::Rect::from_min_max(egui::pos2(split + sp::MD, body.min.y), body.max);

    let mut tree_ui = ui.new_child(
        egui::UiBuilder::new()
            .id_salt("palette_tree")
            .max_rect(tree_rect)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    tree(app, &mut tree_ui, &mut act);

    let mut detail_ui = ui.new_child(
        egui::UiBuilder::new()
            .id_salt("palette_detail")
            .max_rect(detail_rect)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    detail(app, &mut detail_ui, &mut act);

    let mut status_ui = ui.new_child(
        egui::UiBuilder::new()
            .id_salt("palette_status")
            .max_rect(egui::Rect::from_min_max(
                egui::pos2(full.min.x, body.max.y + sp::SM),
                full.max,
            ))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    status_line(app, &mut status_ui);

    apply(app, ui.ctx().clone(), act);
}

// ---------------------------------------------------------------------------
// Toolbar
// ---------------------------------------------------------------------------

fn toolbar(app: &mut DashboardApp, ui: &mut egui::Ui, act: &mut Actions) {
    let busy = app.palette.job.is_some();
    crate::theme::row_between(
        ui,
        24.0,
        |ui| {
            ui.add_space(2.0);
            let search = egui::TextEdit::singleline(&mut app.palette.search)
                .hint_text("search name, summary or tag")
                .font(font_label())
                .desired_width(190.0);
            ui.add(search);
            if !app.palette.search.is_empty() && ghost_button(ui, "×", TEXT_FAINT, TEXT).clicked()
            {
                app.palette.search.clear();
            }
            ui.add_space(sp::SM);
            for f in StatusFilter::ALL {
                if chip(ui, f.label(), app.palette.filter == f).clicked() {
                    app.palette.filter = f;
                }
            }
        },
        |ui| {
            // RTL: first added lands rightmost.
            ui.add_enabled_ui(!busy, |ui| {
                if action_button(ui, "Analyse…", ActionTone::Accent)
                    .on_hover_text(
                        "Rescan, probe the current slice for interface evidence, and render \
                         thumbnails — then hand the card writing to an agent",
                    )
                    .clicked()
                {
                    act.open_analyse = true;
                }
                ui.add_space(sp::SM);
                if action_button(ui, "Rescan", ActionTone::Neutral)
                    .on_hover_text("Reconcile the roster against the .tox files on disk")
                    .clicked()
                {
                    act.rescan = true;
                }
            });
            if busy {
                ui.add_space(sp::SM);
                ui.add(egui::Spinner::new().size(12.0));
            }
        },
    );
}

fn status_line(app: &DashboardApp, ui: &mut egui::Ui) {
    let s = &app.palette.stats;
    let text = if let Some(job) = app.palette.job {
        format!("{}…", job.word())
    } else if let Some(err) = &app.palette.error {
        crate::wire::clip_line(err, 120)
    } else if s.total == 0 {
        "nothing indexed yet".to_owned()
    } else {
        let scanned = s
            .scanned_at
            .as_deref()
            .map(|t| format!(" · scanned {}", t.split('T').next().unwrap_or(t)))
            .unwrap_or_default();
        format!(
            "{} indexed · {} carded · {} ignored{}",
            s.total,
            s.described + s.stale,
            s.ignored,
            scanned
        )
    };
    let color = if app.palette.error.is_some() {
        ERR
    } else {
        TEXT_FAINT
    };
    ui.add_space(2.0);
    ui.label(egui::RichText::new(text).font(font_meta()).color(color));

    // Coverage is the honest headline: a roster of 281 with 4 cards is not a
    // usable library yet, and the bar says so at a glance.
    if s.total > 0 && app.palette.job.is_none() && app.palette.error.is_none() {
        ui.add_space(sp::MD);
        ui.label(
            egui::RichText::new("coverage")
                .font(font_meta())
                .color(TEXT_FAINT),
        );
        let carded = s.described + s.stale;
        let (rect, resp) = ui.allocate_exact_size(egui::vec2(64.0, 4.0), egui::Sense::hover());
        ui.painter().rect_filled(rect, 2.0, BG_ROW);
        let frac = carded as f32 / s.total.max(1) as f32;
        if frac > 0.0 {
            ui.painter().rect_filled(
                egui::Rect::from_min_size(
                    rect.min,
                    egui::vec2((rect.width() * frac).max(2.0), rect.height()),
                ),
                2.0,
                ACCENT,
            );
        }
        resp.on_hover_text(format!(
            "{carded} of {} components have a card — the rest are names and categories only",
            s.total
        ));
    }
}

// ---------------------------------------------------------------------------
// Tree
// ---------------------------------------------------------------------------

/// The whole-page state before anything has been scanned — or while the first
/// roster load is still in flight.
fn empty_page(app: &DashboardApp, ui: &mut egui::Ui, act: &mut Actions) {
    ui.add_space(ui.available_height() * 0.28);
    if app.palette.job.is_some() {
        ui.vertical_centered(|ui| {
            ui.add(egui::Spinner::new().size(16.0));
            ui.add_space(sp::SM);
            ui.label(
                egui::RichText::new("reading the palette roster…")
                    .font(font_meta())
                    .color(TEXT_DIM),
            );
        });
        return;
    }
    let subtitle = match &app.palette.error {
        Some(e) => crate::wire::clip_line(e, 140),
        None => "Scan the TouchDesigner install and your own palette folder to build the \
                 roster. Nothing is loaded into TouchDesigner by a scan — it only reads \
                 the .tox files on disk."
            .to_owned(),
    };
    if empty_state(
        ui,
        "No palette components indexed",
        &subtitle,
        Some("Rescan"),
    ) {
        act.rescan = true;
    }
}

fn tree(app: &mut DashboardApp, ui: &mut egui::Ui, act: &mut Actions) {
    let nodes = flatten(app);
    if nodes.is_empty() {
        ui.add_space(sp::XL);
        ui.vertical_centered(|ui| {
            ui.label(
                egui::RichText::new("( nothing matches this filter )")
                    .font(font_meta())
                    .color(TEXT_FAINT),
            );
        });
        return;
    }

    let selected = app.palette.selected.clone();
    egui::ScrollArea::vertical().auto_shrink(false).show_rows(
        ui,
        PROW_H,
        nodes.len(),
        |ui, range| {
            // Textures are resolved for the visible slice only, before drawing,
            // so the mutable cache borrow never overlaps the row borrows.
            let visible: Vec<(usize, Option<egui::TextureHandle>)> = nodes[range.clone()]
                .iter()
                .filter_map(|n| match n {
                    Node::Item(i) => Some(*i),
                    Node::Group { .. } => None,
                })
                .map(|i| {
                    let row = app.palette.rows[i].clone();
                    let tex = app.palette.thumb(ui.ctx(), &row);
                    (i, tex)
                })
                .collect();

            for node in &nodes[range] {
                match node {
                    Node::Group {
                        name,
                        count,
                        collapsed,
                    } => {
                        if group_row(ui, name, *count, *collapsed).clicked() {
                            act.toggle_group = Some(name.clone());
                        }
                    }
                    Node::Item(i) => {
                        let tex = visible
                            .iter()
                            .find(|(j, _)| j == i)
                            .and_then(|(_, t)| t.clone());
                        let row = &app.palette.rows[*i];
                        let is_sel = selected.as_deref() == Some(row.palette_id.as_str());
                        if item_row(ui, row, tex.as_ref(), is_sel).clicked() {
                            act.select = Some(row.palette_id.clone());
                        }
                    }
                }
            }
        },
    );
}

/// Categories in order, each followed by its components unless folded away.
fn flatten(app: &DashboardApp) -> Vec<Node> {
    let visible = app.palette.visible_rows();
    let mut nodes = Vec::new();
    let mut current: Option<String> = None;
    let mut group_start = 0usize;

    // Index lookup: `visible_rows` hands back references into `rows`, and the
    // draw loop needs positions to re-borrow them one at a time.
    let position = |target: &PaletteRow| -> usize {
        app.palette
            .rows
            .iter()
            .position(|r| r.palette_id == target.palette_id)
            .unwrap_or(0)
    };

    for row in &visible {
        let group = row.group().to_owned();
        if current.as_deref() != Some(group.as_str()) {
            current = Some(group.clone());
            group_start = nodes.len();
            nodes.push(Node::Group {
                name: group.clone(),
                count: 0,
                collapsed: app.palette.collapsed.contains(&group),
            });
        }
        if let Some(Node::Group {
            count, collapsed, ..
        }) = nodes.get_mut(group_start)
        {
            *count += 1;
            let folded = *collapsed;
            if !folded {
                let idx = position(row);
                nodes.push(Node::Item(idx));
            }
        }
    }
    nodes
}

fn group_row(ui: &mut egui::Ui, name: &str, count: usize, collapsed: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), PROW_H),
        egui::Sense::click(),
    );
    if response.hovered() {
        ui.painter().rect_filled(rect, RADIUS_SM, BG_HOVER);
    }
    let p = ui.painter();
    p.text(
        egui::pos2(rect.left() + INSET, rect.center().y),
        egui::Align2::LEFT_CENTER,
        if collapsed { "▶" } else { "▾" },
        font_meta(),
        TEXT_FAINT,
    );
    p.text(
        egui::pos2(rect.left() + INSET + 14.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        name.to_uppercase(),
        font_meta(),
        TEXT_DIM,
    );
    p.text(
        egui::pos2(rect.right() - RIGHT_INSET, rect.center().y),
        egui::Align2::RIGHT_CENTER,
        count.to_string(),
        font_meta(),
        TEXT_FAINT,
    );
    response
}

fn item_row(
    ui: &mut egui::Ui,
    row: &PaletteRow,
    tex: Option<&egui::TextureHandle>,
    selected: bool,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), PROW_H),
        egui::Sense::click(),
    );
    let fill = if selected {
        BG_ACTIVE
    } else if response.hovered() {
        BG_HOVER
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, RADIUS_SM, fill);
    if selected {
        // Same 3px accent rail the sidebar uses for the active nav item.
        ui.painter().rect_filled(
            egui::Rect::from_min_size(
                rect.left_top() + egui::vec2(0.0, 4.0),
                egui::vec2(3.0, rect.height() - 8.0),
            ),
            2.0,
            ACCENT,
        );
    }

    let tile = egui::Rect::from_min_size(
        egui::pos2(rect.left() + INSET + 6.0, rect.center().y - TILE * 0.5),
        egui::vec2(TILE, TILE),
    );
    draw_tile(ui, tile, row, tex);

    let text_x = tile.right() + sp::SM;
    let dot_x = rect.right() - RIGHT_INSET - 2.0;
    let avail = (dot_x - text_x - sp::SM).max(20.0);
    let galley = ui.painter().layout(
        row.name.clone(),
        font_label(),
        if row.ignored { TEXT_FAINT } else { TEXT },
        avail,
    );
    // One line only: the tree is a picker, not a reader.
    ui.painter().galley(
        egui::pos2(text_x, rect.center().y - galley.size().y.min(PROW_H) * 0.5),
        galley,
        TEXT,
    );

    ui.painter().circle_filled(
        egui::pos2(dot_x, rect.center().y),
        3.0,
        state_color(row.state()),
    );

    response.on_hover_text(format!(
        "{}\n{}{}",
        row.palette_id,
        row.state().word(),
        row.summary
            .as_deref()
            .map(|s| format!(" — {s}"))
            .unwrap_or_default()
    ))
}

fn state_color(state: RowState) -> Color32 {
    match state {
        RowState::Carded => OK,
        RowState::Stale => WARN,
        RowState::Failed => ERR,
        RowState::Undescribed | RowState::Ignored => TEXT_FAINT,
    }
}

/// The thumbnail, or a monogram tile standing in for one that has not been
/// rendered yet. The placeholder is deliberately a designed object rather than
/// an empty hole — most of the roster will wear it until a thumbnail pass runs.
fn draw_tile(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    row: &PaletteRow,
    tex: Option<&egui::TextureHandle>,
) {
    match tex {
        Some(t) => {
            let radius = egui::CornerRadius::same((rect.width() * 0.16) as u8);
            ui.painter().rect_filled(rect, radius, BG_ROW);
            egui::Image::new(t)
                .fit_to_exact_size(rect.size())
                .corner_radius(radius)
                .paint_at(ui, rect);
        }
        None => {
            let tint = category_tint(row.group());
            ui.painter().rect_filled(
                rect,
                egui::CornerRadius::same((rect.width() * 0.16) as u8),
                tint,
            );
            let initials: String = row
                .name
                .chars()
                .filter(|c| c.is_ascii_alphanumeric())
                .take(2)
                .collect::<String>()
                .to_uppercase();
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                initials,
                egui::FontId::new(
                    (rect.width() * 0.42).max(8.0),
                    egui::FontFamily::Proportional,
                ),
                TEXT_DIM,
            );
        }
    }
}

/// A stable, quiet fill per category so the placeholder grid still reads as
/// grouped. Hashed rather than tabulated — the category list is the user's
/// folder tree, not a fixed set this code could enumerate.
fn category_tint(group: &str) -> Color32 {
    let mut hash: u32 = 0x811c_9dc5;
    for b in group.as_bytes() {
        hash ^= u32::from(*b);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    // Narrow band around the panel fill: visible as grouping, never as color.
    let base = 0x22u8;
    let spread = (hash % 3) as u8 * 4;
    Color32::from_rgb(
        base + spread + (hash >> 8) as u8 % 5,
        base + spread,
        base + spread + (hash >> 16) as u8 % 7,
    )
}

// ---------------------------------------------------------------------------
// Detail pane
// ---------------------------------------------------------------------------

fn detail(app: &mut DashboardApp, ui: &mut egui::Ui, act: &mut Actions) {
    let Some(row) = app.palette.selected_row().cloned() else {
        ui.add_space(ui.available_height() * 0.3);
        let _ = empty_state(
            ui,
            "Nothing selected",
            "Pick a component to see its preview, interface and card.",
            None,
        );
        return;
    };
    let detail = app.palette.detail.clone();
    let card = detail.as_ref().and_then(|d| d.card.clone());

    egui::ScrollArea::vertical()
        .auto_shrink(false)
        .show(ui, |ui| {
            header_card(ui, &row, app.palette.thumb(ui.ctx(), &row).as_ref());
            actions_card(app, ui, &row, detail.as_ref(), act);

            if let Some(body) = &card {
                card_with_header(ui, "CARD", None, |_| {}, |ui| card_text(ui, body));
            } else if let Some(err) = detail.as_ref().and_then(|d| d.card_error.clone()) {
                card_with_header(
                    ui,
                    "CARD",
                    Some(WARN),
                    |_| {},
                    |ui| {
                        ui.label(
                            egui::RichText::new(format!("card file unreadable — {err}"))
                                .font(font_meta())
                                .color(TEXT_DIM),
                        );
                    },
                );
            } else {
                card_with_header(
                    ui,
                    "CARD",
                    None,
                    |_| {},
                    |ui| {
                        ui.label(
                            egui::RichText::new(
                                "Not described yet. A card is written by an agent from probe \
                                 evidence — nothing generates one. Analyse… collects the \
                                 evidence and hands off the writing.",
                            )
                            .font(font_meta())
                            .color(TEXT_DIM),
                        );
                    },
                );
            }
        });
}

fn header_card(ui: &mut egui::Ui, row: &PaletteRow, tex: Option<&egui::TextureHandle>) {
    card_with_header(
        ui,
        "COMPONENT",
        None,
        |ui| {
            let _ = badge(
                ui,
                row.state().word(),
                match row.state() {
                    RowState::Carded => BadgeKind::Ok,
                    RowState::Stale => BadgeKind::Warn,
                    RowState::Failed => BadgeKind::Error,
                    RowState::Undescribed | RowState::Ignored => BadgeKind::Neutral,
                },
            );
            if row.is_user() {
                let _ = badge(ui, "yours", BadgeKind::Accent)
                    .on_hover_text("From your own palette folder, not the TouchDesigner install");
            }
        },
        |ui| {
            ui.horizontal(|ui| {
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(PREVIEW, PREVIEW), egui::Sense::hover());
                draw_tile(ui, rect, row, tex);
                ui.add_space(sp::MD);
                ui.vertical(|ui| {
                    ui.label(
                        egui::RichText::new(&row.name)
                            .font(font_title())
                            .color(TEXT),
                    );
                    ui.add_space(2.0);
                    ui.label(
                        egui::RichText::new(&row.palette_id)
                            .font(font_mono())
                            .color(TEXT_FAINT),
                    );
                    ui.add_space(sp::SM);
                    if let Some(summary) = &row.summary {
                        ui.label(
                            egui::RichText::new(summary)
                                .font(font_label())
                                .color(TEXT_DIM),
                        );
                    }
                    if !row.tags.is_empty() {
                        ui.add_space(sp::XS);
                        ui.horizontal_wrapped(|ui| {
                            for tag in &row.tags {
                                let _ = badge(ui, tag, BadgeKind::Neutral);
                            }
                        });
                    }
                    if tex.is_none() {
                        ui.add_space(sp::XS);
                        ui.label(
                            egui::RichText::new("no preview rendered yet")
                                .font(font_meta())
                                .color(TEXT_FAINT),
                        );
                    }
                });
            });
        },
    );
}

fn actions_card(
    app: &DashboardApp,
    ui: &mut egui::Ui,
    row: &PaletteRow,
    detail: Option<&pal::PaletteDetail>,
    act: &mut Actions,
) {
    let tox = detail.map(|d| d.tox_path.clone()).unwrap_or_default();
    card_with_header(
        ui,
        "USE IT",
        Some(ACCENT),
        |_| {},
        |ui| {
            ui.horizontal(|ui| {
                if action_button(ui, "Copy reference", ActionTone::Accent)
                    .on_hover_text(
                        "Id, summary, pins and the mutate_nodes place step — paste straight \
                         into an agent",
                    )
                    .clicked()
                {
                    act.copy = Some((pal::reference_brief(row, detail), "Reference copied"));
                }
                ui.add_space(sp::SM);
                if action_button(ui, "Copy place step", ActionTone::Neutral)
                    .on_hover_text("Just the mutate_nodes step")
                    .clicked()
                {
                    act.copy = Some((pal::place_step(row), "Place step copied"));
                }
                ui.add_space(sp::SM);
                if ghost_button(ui, "id", TEXT_DIM, ACCENT)
                    .on_hover_text(row.palette_id.clone())
                    .clicked()
                {
                    act.copy = Some((row.palette_id.clone(), "Id copied"));
                }
                if let Some(card) = detail.and_then(|d| d.card.clone()) {
                    if ghost_button(ui, "card", TEXT_DIM, ACCENT)
                        .on_hover_text("Copy the full card markdown")
                        .clicked()
                    {
                        act.copy = Some((card, "Card copied"));
                    }
                }
                if !tox.is_empty()
                    && ghost_button(ui, "reveal", TEXT_DIM, ACCENT)
                        .on_hover_text(tox.clone())
                        .clicked()
                {
                    act.reveal = Some(tox.clone());
                }
                let busy = app.palette.job.is_some();
                ui.add_enabled_ui(!busy, |ui| {
                    let (label, hint, target) = if row.ignored {
                        (
                            "unblacklist",
                            "Probe this component again on the next run",
                            false,
                        )
                    } else {
                        (
                            "blacklist",
                            "Skip this component in bulk probe runs — for one that wedges TD",
                            true,
                        )
                    };
                    if ghost_button(ui, label, TEXT_FAINT, WARN)
                        .on_hover_text(hint)
                        .clicked()
                    {
                        act.set_ignored = Some((row.palette_id.clone(), target));
                    }
                });
            });
        },
    );
}

// ---------------------------------------------------------------------------
// Card rendering
// ---------------------------------------------------------------------------

/// Render the Markdown subset a palette card is actually written in.
///
/// Cards follow one authored template (`tdmcp://docs/palette-scan`): headings,
/// `**Bold:**` lead-ins, inline code, bullets, and fenced blocks — the last of
/// which carries the OpSketch and is the reason this exists at all. A generic
/// Markdown widget would be a dependency and a worse fit; anything outside the
/// subset falls through as plain text rather than as syntax.
fn card_text(ui: &mut egui::Ui, body: &str) {
    let mut fence: Option<Vec<String>> = None;
    for raw in body.lines() {
        let line = raw.trim_end();
        if line.trim_start().starts_with("```") {
            match fence.take() {
                Some(block) => code_block(ui, &block),
                None => fence = Some(Vec::new()),
            }
            continue;
        }
        if let Some(block) = fence.as_mut() {
            block.push(line.to_owned());
            continue;
        }
        if line.trim().is_empty() {
            ui.add_space(sp::XS);
            continue;
        }
        if let Some(heading) = line.trim_start().strip_prefix("# ") {
            ui.label(
                egui::RichText::new(heading.trim())
                    .font(font_title())
                    .color(TEXT),
            );
            continue;
        }
        let (bullet, rest) = match line.trim_start().strip_prefix("- ") {
            Some(r) => (true, r),
            None => (false, line.trim_start()),
        };
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 3.0;
            if bullet {
                ui.label(
                    egui::RichText::new("·")
                        .font(font_label())
                        .color(TEXT_FAINT),
                );
            }
            for (text, kind) in inline_spans(rest) {
                let rich = match kind {
                    Span::Bold => egui::RichText::new(text)
                        .font(font_label())
                        .color(TEXT)
                        .strong(),
                    Span::Code => egui::RichText::new(text).font(font_mono()).color(ACCENT),
                    Span::Plain => egui::RichText::new(text).font(font_label()).color(TEXT_DIM),
                };
                ui.label(rich);
            }
        });
    }
    // An unterminated fence still renders — a truncated card is not a reason
    // to show the reader nothing.
    if let Some(block) = fence {
        code_block(ui, &block);
    }
}

fn code_block(ui: &mut egui::Ui, lines: &[String]) {
    ui.add_space(sp::XS);
    egui::Frame::NONE
        .fill(BG_ROW)
        .stroke(egui::Stroke::new(1.0, BORDER))
        .corner_radius(egui::CornerRadius::same(RADIUS_SM as u8))
        .inner_margin(egui::Margin::same(sp::SM as i8))
        .show(ui, |ui| {
            // Its own horizontal scroll: an OpSketch line is wide and must not
            // wrap into nonsense or push the page sideways.
            egui::ScrollArea::horizontal()
                .id_salt(("palette_code", lines.len(), lines.first().cloned()))
                .max_height(240.0)
                .show(ui, |ui| {
                    ui.vertical(|ui| {
                        for line in lines {
                            ui.label(egui::RichText::new(line).font(font_mono()).color(TEXT_DIM));
                        }
                    });
                });
        });
    ui.add_space(sp::XS);
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Span {
    Plain,
    Bold,
    Code,
}

/// Split a line into `**bold**`, `` `code` `` and plain runs.
fn inline_spans(line: &str) -> Vec<(String, Span)> {
    let mut out = Vec::new();
    let mut buf = String::new();
    let mut rest = line;
    while !rest.is_empty() {
        let bold = rest.find("**");
        let code = rest.find('`');
        let next = match (bold, code) {
            (Some(b), Some(c)) => Some(b.min(c)),
            (Some(b), None) => Some(b),
            (None, Some(c)) => Some(c),
            (None, None) => None,
        };
        let Some(at) = next else {
            buf.push_str(rest);
            break;
        };
        buf.push_str(&rest[..at]);
        let is_bold = rest[at..].starts_with("**");
        let (marker, span) = if is_bold {
            ("**", Span::Bold)
        } else {
            ("`", Span::Code)
        };
        let after = &rest[at + marker.len()..];
        match after.find(marker) {
            Some(end) => {
                if !buf.is_empty() {
                    out.push((std::mem::take(&mut buf), Span::Plain));
                }
                out.push((after[..end].to_owned(), span));
                rest = &after[end + marker.len()..];
            }
            // An unclosed marker is literal text, not a broken span.
            None => {
                buf.push_str(&rest[at..]);
                break;
            }
        }
    }
    if !buf.is_empty() {
        out.push((buf, Span::Plain));
    }
    out
}

// ---------------------------------------------------------------------------
// Analyse modal
// ---------------------------------------------------------------------------

/// Connected TouchDesigner pids, newest listing order preserved.
fn connected_pids(app: &DashboardApp) -> Vec<(u32, String)> {
    serde_json::from_str::<crate::wire::FleetView>(&app.fleet_json)
        .map(|f| f.processes)
        .unwrap_or_default()
        .into_iter()
        .filter(|p| p.bridge.as_str() == Some("connected"))
        .map(|p| (p.pid, p.title.unwrap_or_default()))
        .collect()
}

/// Throwaway project a thumbnail/probe pass can safely load components into.
fn probe_project(app: &DashboardApp) -> std::path::PathBuf {
    app.data_dir.join("palette_probe.toe")
}

pub(crate) fn analyse_modal(app: &mut DashboardApp, ctx: &egui::Context) {
    if !app.palette.analyse_open {
        return;
    }
    let mut close = false;
    let mut start_pid: Option<u32> = None;
    let mut spawn = false;
    let mut cancel = false;
    let mut copy: Option<String> = None;

    let pids = connected_pids(app);
    let running = app.palette.analyse.running;
    let finished = app.palette.analyse.finished;

    super::widgets::modal_shell(ctx, "palette_analyse", |ui| {
        crate::theme::row_between(
            ui,
            20.0,
            |ui| {
                ui.label(
                    egui::RichText::new("ANALYSE PALETTE")
                        .font(font_meta())
                        .color(TEXT_FAINT),
                );
            },
            |ui| {
                let mut line = format!("slice: {}", app.palette.analyse.slice);
                if let Some(pid) = app.palette.analyse.pid {
                    line.push_str(&format!(" · pid {pid}"));
                }
                ui.label(egui::RichText::new(line).font(font_meta()).color(TEXT_DIM));
            },
        );
        ui.add_space(sp::MD);

        if !running && !finished {
            ui.label(
                egui::RichText::new(
                    "Rescans the roster, probes this slice for interface evidence, and renders \
                     a thumbnail per component. Components are loaded into a scratch COMP and \
                     destroyed again — never into your own project.",
                )
                .font(font_meta())
                .color(TEXT_DIM),
            );
            ui.add_space(sp::MD);

            if pids.is_empty() {
                crate::theme::banner(
                    ui,
                    crate::theme::BannerTone::Warn,
                    "No TouchDesigner connected. Probing needs a live instance — spawn a \
                     throwaway project rather than pointing this at work you care about.",
                );
                ui.add_space(sp::SM);
                ui.horizontal(|ui| {
                    if action_button(ui, "Spawn throwaway probe", ActionTone::Accent)
                        .on_hover_text(probe_project(app).display().to_string())
                        .clicked()
                    {
                        spawn = true;
                    }
                    ui.add_space(sp::SM);
                    if action_button(ui, "Cancel", ActionTone::Neutral).clicked() {
                        close = true;
                    }
                });
                if app.spawn_busy {
                    ui.add_space(sp::SM);
                    ui.horizontal(|ui| {
                        ui.add(egui::Spinner::new().size(12.0));
                        ui.label(
                            egui::RichText::new("starting TouchDesigner…")
                                .font(font_meta())
                                .color(TEXT_DIM),
                        );
                    });
                }
            } else {
                super::widgets::section_caption(ui, "PROBE IN");
                for (pid, title) in &pids {
                    ui.horizontal(|ui| {
                        ui.add_space(sp::MD);
                        status_led(ui, OK);
                        ui.add_space(sp::XS);
                        ui.label(
                            egui::RichText::new(pid.to_string())
                                .font(font_mono())
                                .color(TEXT_DIM),
                        );
                        ui.label(
                            egui::RichText::new(title.as_str())
                                .font(font_label())
                                .color(TEXT),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if action_button(ui, "Run here", ActionTone::Accent).clicked() {
                                start_pid = Some(*pid);
                            }
                        });
                    });
                }
                ui.add_space(sp::SM);
                ui.horizontal(|ui| {
                    if action_button(ui, "Cancel", ActionTone::Neutral).clicked() {
                        close = true;
                    }
                });
            }
        } else {
            steps_view(ui, &app.palette.analyse);
            ui.add_space(sp::MD);
            ui.horizontal(|ui| {
                if finished {
                    if action_button(ui, "Copy brief for agent", ActionTone::Accent)
                        .on_hover_text(
                            "The describe loop for what is still undescribed, ready to paste",
                        )
                        .clicked()
                    {
                        copy = Some(pal::analyse_brief(&app.palette.analyse));
                    }
                    ui.add_space(sp::SM);
                    if action_button(ui, "Close", ActionTone::Neutral).clicked() {
                        close = true;
                    }
                } else if action_button(ui, "Stop", ActionTone::Danger)
                    .on_hover_text("Finish the batch in flight, then stop")
                    .clicked()
                {
                    cancel = true;
                }
            });
        }
    });

    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) && !running {
        close = true;
    }
    if spawn {
        let path = probe_project(app);
        app.spawn_project(path, true);
    }
    if let Some(pid) = start_pid {
        pal::analyse(app, pid);
    }
    if cancel {
        app.palette
            .cancel
            .store(true, std::sync::atomic::Ordering::Relaxed);
        app.snack("Stopping after the current batch", SnackTone::Info);
    }
    if let Some(text) = copy {
        ctx.copy_text(text);
        app.snack("Brief copied to clipboard", SnackTone::Info);
    }
    if close {
        app.palette.analyse_open = false;
    }
}

/// Painted step mark. Deliberately not a glyph: the bundled fonts cover
/// neither `✓` nor `◌`, and a tofu box in a progress list is worse than no
/// mark at all. Filled = settled, ring = not yet — colour carries the rest,
/// same as every other status LED in the app.
fn step_mark(ui: &mut egui::Ui, status: StepState) {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
    let center = rect.center();
    match status {
        StepState::Done => {
            ui.painter().circle_filled(center, 4.0, OK);
        }
        StepState::Failed => {
            ui.painter().circle_filled(center, 4.0, ERR);
        }
        StepState::Running => {
            // Breathing halo — the one moving thing on the panel.
            let t = ui.input(|i| i.time) as f32;
            let pulse = (t * 2.2).sin() * 0.5 + 0.5;
            ui.painter().circle_filled(
                center,
                4.0 + 2.5 * pulse,
                Color32::from_rgba_unmultiplied(
                    ACCENT.r(),
                    ACCENT.g(),
                    ACCENT.b(),
                    (30.0 + 60.0 * pulse) as u8,
                ),
            );
            ui.painter().circle_filled(center, 3.5, ACCENT);
            ui.ctx().request_repaint();
        }
        StepState::HandedOff => {
            ui.painter()
                .circle_stroke(center, 4.0, egui::Stroke::new(1.5, WARN));
        }
        StepState::Pending => {
            ui.painter()
                .circle_stroke(center, 4.0, egui::Stroke::new(1.0, TEXT_FAINT));
        }
    }
    let _ = response;
}

fn steps_view(ui: &mut egui::Ui, state: &AnalyseState) {
    for (step, status, note) in &state.states {
        ui.horizontal(|ui| {
            ui.add_space(sp::XS);
            step_mark(ui, *status);
            ui.add_space(sp::XS);
            let (rect, _) = ui.allocate_exact_size(egui::vec2(96.0, 18.0), egui::Sense::hover());
            ui.painter().text(
                egui::pos2(rect.left(), rect.center().y),
                egui::Align2::LEFT_CENTER,
                step.label(),
                font_label(),
                if *status == StepState::Pending {
                    TEXT_FAINT
                } else {
                    TEXT
                },
            );
            ui.label(
                egui::RichText::new(note.as_str())
                    .font(font_meta())
                    .color(TEXT_DIM),
            );
        });
    }
    if state
        .states
        .iter()
        .any(|(s, st, _)| *s == Step::Cards && *st == StepState::HandedOff)
    {
        ui.add_space(sp::SM);
        ui.label(
            egui::RichText::new(
                "Cards are written from probe evidence by an agent — nothing here generates \
                 them. The brief below says exactly which slice is left.",
            )
            .font(font_meta())
            .color(TEXT_FAINT),
        );
    }
}

// ---------------------------------------------------------------------------
// Deferred actions
// ---------------------------------------------------------------------------

fn apply(app: &mut DashboardApp, ctx: egui::Context, act: Actions) {
    if let Some(group) = act.toggle_group {
        if !app.palette.collapsed.remove(&group) {
            app.palette.collapsed.insert(group);
        }
    }
    if let Some(id) = act.select {
        if app.palette.selected.as_deref() != Some(id.as_str()) {
            app.palette.selected = Some(id.clone());
            app.palette.detail = None;
            pal::load_detail(app, id);
        }
    }
    if let Some((text, note)) = act.copy {
        ctx.copy_text(text);
        app.snack(note, SnackTone::Info);
    }
    if let Some(path) = act.reveal {
        let target = std::path::PathBuf::from(&path);
        let fallback = target.parent().map(std::path::Path::to_path_buf);
        if let Err(e) =
            crate::platform::reveal_in_file_manager(&target, fallback.as_deref().unwrap_or(&target))
        {
            app.snack(&format!("Reveal failed: {e}"), SnackTone::Warn);
        }
    }
    if let Some((id, ignore)) = act.set_ignored {
        pal::set_ignored(app, id, ignore);
    }
    if act.rescan {
        pal::rescan(app);
    }
    if act.open_analyse {
        open_analyse(app);
    }
}

/// Seed the modal from what the tree is currently showing, so "Analyse" means
/// "analyse what I am looking at" rather than "analyse all 281 of them".
fn open_analyse(app: &mut DashboardApp) {
    let category = app
        .palette
        .selected_row()
        .map(|r| r.group().to_owned())
        .filter(|g| g != "(root)");
    let filter = app.palette.filter;
    let status = if filter == StatusFilter::All {
        // The default slice is the one the describe loop is actually for.
        StatusFilter::Undescribed
    } else {
        filter
    };
    let slice = match &category {
        Some(cat) => format!("{cat} · {}", status.label()),
        None => format!("whole palette · {}", status.label()),
    };
    app.palette.analyse = AnalyseState::fresh(slice, status.select_status(), category);
    app.palette.analyse.running = false;
    app.palette.analyse_open = true;
}
