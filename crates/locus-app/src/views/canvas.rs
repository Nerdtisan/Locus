// SPDX-License-Identifier: AGPL-3.0-or-later
//! The canvas view (design §7.3): paints the scene and overlays and turns
//! pointer, touch and key input into tool steps and commands (SPEC §11).

use crate::app::Status;
use crate::grid;
use crate::hit::{self, BoxMode, Mods};
use crate::infer;
use crate::modify_tool::Modify;
use crate::render::{Emphasis, Frame, Palette};
use crate::settings::{Settings, Units};
use crate::state::AppState;
use crate::tool::ToolEvent;
use crate::tools::{Pick, Tool};
use egui::{Color32, PointerButton, Pos2, Sense, Stroke};
use locus_geom::{Aabb, Vec2};

#[cfg(not(target_arch = "wasm32"))]
mod ai;
mod bed;
mod machine;
mod offsets;
mod pattern_labels;
pub mod plugin_objects;
mod regions;

pub(crate) use machine::MachinePath;
pub(crate) use regions::RegionUi;

/// Pick radius while a finger drives the pointer (#251): a fingertip covers
/// far more than a mouse pixel. Mouse and pen keep `HIT_TOL_PT`.
const TOUCH_HIT_TOL_PT: f64 = 14.0;
/// A drag shorter than this is a click, not a box.
const MIN_BOX_PT: f32 = 4.0;
use crate::state::HIT_TOL_PT;

impl AppState {
    /// The refusal banner over the canvas: why, and the driven offer.
    pub(crate) fn refusal_banner(&mut self, ui: &egui::Ui, rect: egui::Rect) {
        let Some(r) = self.doc.refusal.clone() else {
            return;
        };
        let what = if r.as_driven.is_some() {
            "dimension"
        } else {
            "constraint"
        };
        let why = if r.redundant {
            format!(
                "is already implied by the {} highlighted constraint(s)",
                r.constraints.len()
            )
        } else {
            format!(
                "conflicts with the {} highlighted constraint(s)",
                r.constraints.len()
            )
        };
        let (mut add, mut dismiss) = (false, false);
        egui::Area::new(egui::Id::new("refusal"))
            .order(egui::Order::Foreground)
            .fixed_pos(rect.left_top() + egui::vec2(12.0, 12.0))
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.colored_label(
                        Palette::new(ui.visuals().dark_mode).conflict,
                        r.message
                            .clone()
                            .unwrap_or_else(|| format!("This {what} {why}, so it was not added.")),
                    );
                    ui.horizontal(|ui| {
                        if r.as_driven.is_some()
                            && ui
                                .button("Add as driven")
                                .on_hover_text(
                                    "A driven (reference) dimension shows the measured value without constraining the sketch",
                                )
                                .clicked()
                        {
                            add = true;
                        }
                        dismiss = ui.button("Dismiss").clicked();
                    });
                });
            });
        if add && let Some(cmd) = r.as_driven {
            self.doc.refusal = None;
            self.apply_reporting(&cmd);
        } else if dismiss {
            self.doc.refusal = None;
        }
    }

    /// The inline value box (#70).
    pub(crate) fn dimension_editor(&mut self, ui: &egui::Ui, rect: egui::Rect, units: Units) {
        let notice = self.doc.notice.clone();
        let Some(edit) = self.tools.dim_edit.as_mut() else {
            return;
        };
        let id = egui::Id::new("dim-edit-text");
        let at = self.view.camera.to_screen(edit.at, rect) + egui::vec2(14.0, -34.0);
        let (mut commit, mut cancel) = (false, false);
        egui::Area::new(egui::Id::new("dim-edit"))
            .order(egui::Order::Foreground)
            .fixed_pos(at)
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(edit.what);
                        let out = egui::TextEdit::singleline(&mut edit.text)
                            .id(id)
                            .desired_width(84.0)
                            .show(ui);
                        if edit.fresh {
                            out.response.request_focus();
                            let mut state = out.state.clone();
                            let end = edit.text.chars().count();
                            state
                                .cursor
                                .set_char_range(Some(egui::text::CCursorRange::two(
                                    egui::text::CCursor::new(0),
                                    egui::text::CCursor::new(end),
                                )));
                            state.store(ui.ctx(), id);
                            edit.fresh = false;
                        } else if out.response.lost_focus() {
                            if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                                commit = true;
                            } else {
                                cancel = true;
                            }
                        }
                        ui.weak(if edit.angle { "°" } else { units.label() });
                    });
                    if let Some(msg) = &notice {
                        ui.colored_label(ui.visuals().error_fg_color, msg);
                    }
                });
            });
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            cancel = true;
        }
        if commit && let Some(mut edit) = self.tools.dim_edit.take() {
            if !self.commit_dimension(&edit, units) {
                // Keep the box open, with the error beneath it.
                edit.fresh = false;
                self.tools.dim_edit = Some(edit);
            }
        } else if cancel {
            self.tools.dim_edit = None;
        }
    }

    pub fn canvas_ui(&mut self, ui: &mut egui::Ui, settings: &Settings, status: &mut Status) {
        let (resp, painter) = ui.allocate_painter(ui.available_size(), Sense::click_and_drag());
        let rect = resp.rect;
        let dark = ui.visuals().dark_mode;
        let pal = Palette::new(dark);
        painter.rect_filled(rect, 0.0, pal.background);

        if self.view.auto_fit && rect.size() != self.view.fit_size && rect.width() > 10.0 {
            let (lo, hi) = self.bounds();
            self.view.camera.fit(lo, hi, rect);
        }
        self.view.fit_size = rect.size();

        // Touch: a second finger turns the sequence into a pinch/pan gesture
        // and keeps every tool out of it (#250, epic #30).
        let now = ui.input(|i| i.time);
        let step = ui.input(|i| self.view.touches.update(&i.events, now));
        let suppress = step.suppress;
        self.view.tol_pt = if self.view.touches.coarse(now) {
            TOUCH_HIT_TOL_PT
        } else {
            HIT_TOL_PT
        };
        if let Some(due) = self.view.touches.long_press_due(now) {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_secs_f64(due + 0.01));
        }
        if let Some(p) = step.long_press {
            self.open_marking(p, rect, false, now);
        }
        // Right-click opens the marking menu (design §3.1, §4, #290).
        if !suppress
            && resp.hovered()
            && let Some(p) = ui.input(|i| {
                i.pointer
                    .button_pressed(PointerButton::Secondary)
                    .then(|| i.pointer.press_origin())
                    .flatten()
            })
        {
            self.open_marking(p, rect, true, now);
        }
        let gesture = if self.view.touches.gesture_allowed() {
            ui.input(|i| i.multi_touch())
        } else {
            None
        };
        if let Some(g) = gesture {
            self.view
                .camera
                .gesture(g.center_pos, g.zoom_delta as f64, g.translation_delta, rect);
            self.view.auto_fit = false;
        }

        // Zoom about the cursor; middle-drag or Space + drag pans (design §3.1).
        let space = ui.input(|i| i.key_down(egui::Key::Space));
        let hover = resp.hover_pos();
        if let Some(hp) = hover.filter(|_| !suppress) {
            let (scroll, zoom) = ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
            let factor = zoom as f64 * (scroll as f64 * 0.0025).exp();
            if factor != 1.0 {
                self.view.camera.zoom_about(hp, factor, rect);
                self.view.auto_fit = false;
            }
        }
        if resp.dragged_by(PointerButton::Middle)
            || (space && resp.dragged_by(PointerButton::Primary))
        {
            self.view.camera.pan_points(resp.drag_delta());
            self.view.auto_fit = false;
        }

        if suppress {
            self.abandon_pointer_action();
        }
        self.view.pointer_over = resp.hovered();
        self.view.rect = rect;
        let wanted = self.wanted_handles();
        if wanted != self.selection.handles {
            self.selection.handles = wanted;
            self.rebuild();
        }
        if suppress || space || self.view.marking.is_some() {
            // A gesture, a Space pan or the marking menu: no tool sees the pointer.
        } else if self.modify().is_some_and(|m| m != Modify::Transform) {
            self.modify_input(ui, &resp, rect, settings.units);
        } else if self.tool() == Tool::Select {
            self.pick_and_select(ui, &resp, rect, settings.units);
        } else {
            self.tool_input(ui, &resp, rect, settings);
        }

        let painter = painter.with_clip_rect(rect);
        grid::draw(&painter, &self.view.camera, rect, dark);
        let frame = Frame {
            painter: &painter,
            view: &self.view.camera,
            rect,
            pal,
            units: settings.units,
        };
        let (selection, hovered) = (&self.selection.items, self.selection.hover);
        let (flag_constraints, flag_entities) = self.flagged();
        self.draw_regions(&painter, rect, dark);
        self.draw_bed(&painter, rect, dark, settings);
        self.draw_machine(&painter, rect, dark);
        self.draw_stroke_preview(&frame, dark);
        self.view.drawn = frame.draw_items(&self.doc.scene, |item| {
            if flag_entities.contains(&item.id) {
                Emphasis::Conflict
            } else if selection.contains(item.id) {
                Emphasis::Selected
            } else if hovered == Some(item.id) {
                Emphasis::Hover
            } else {
                Emphasis::None
            }
        });
        let doc = self.doc.sketcher.document();
        let guides: Vec<(Vec2, Vec2)> = self
            .selection
            .handles
            .iter()
            .filter_map(|id| doc.cubic(*id))
            .flat_map(|c| [(c.p0, c.c0), (c.p1, c.c1)])
            .collect();
        frame.draw_handle_guides(&guides);
        self.view.dim_rects = frame.draw_annotations(&self.doc.scene, &flag_constraints);
        self.draw_tool_overlay(&frame, &painter);
        if let (Some(start), Some(now)) = (self.view.rubber, resp.interact_pointer_pos()) {
            draw_rubber_band(&painter, start, now);
        }
        grid::rulers(&painter, &self.view.camera, rect);
        self.draw_offset_labels(&painter, rect, dark, settings.units);
        self.draw_pattern_labels(&painter, rect, dark, settings.units);
        self.dimension_editor(ui, rect, settings.units);
        self.refusal_banner(ui, rect);

        status.cursor = hover.map(|p| self.view.camera.to_world(p, rect));
        status.extent_mm = self.view.camera.extent_mm(rect);
        status.grid = grid::label(grid::minor_step(self.view.camera.pt_per_mm).0);
    }

    /// Right-click or long press (design §3.1, §4; #251, #290): in Select,
    /// the thing under the pointer is selected first (unless it already is),
    /// then the marking menu opens there.
    pub(crate) fn open_marking(&mut self, at: Pos2, rect: egui::Rect, held: bool, now: f64) {
        if self.tool() == Tool::Select && self.modify().is_none() {
            let tol = self.view.tol_pt / self.view.camera.pt_per_mm;
            let w = self.view.camera.to_world(at, rect);
            if let Some(id) = hit::pick(&self.doc.scene.items, w, tol)
                && !self.selection.items.contains(id)
            {
                self.selection.items.set_all([id]);
            }
        }
        self.view.marking = Some(crate::state::Marking {
            at,
            held,
            since: now,
        });
    }

    /// Sticky on-screen Shift / Ctrl for touch selection (#251). Shown once
    /// a touch screen has been used; mouse sessions never see it.
    pub fn touch_modifiers_ui(&mut self, ui: &mut egui::Ui) {
        if !ui.input(|i| i.has_touch_screen()) {
            return;
        }
        ui.separator();
        ui.toggle_value(&mut self.view.touch_add, "Add")
            .on_hover_text("On-screen Shift: tapping adds to the selection");
        ui.toggle_value(&mut self.view.touch_toggle, "Toggle")
            .on_hover_text("On-screen Ctrl: tapping toggles selection membership");
    }

    /// A second finger landed: put back whatever the first one had started
    /// (a drag, a box, a pen handle) so the gesture can't edit anything.
    pub(crate) fn abandon_pointer_action(&mut self) {
        if self.tools.dragging {
            self.doc.sketcher.cancel_drag();
            self.tools.dragging = false;
            self.rebuild();
        }
        self.view.rubber = None;
        self.tools.pen = None;
        self.selection.hover = None;
    }

    /// What the cursor designates for the active tool: snapped onto a point,
    /// or with inferred constraints (#68, #69). Holding Alt suppresses
    /// inference for the moment, like the global toggle.
    pub(crate) fn designate(&self, world: Vec2, auto: bool) -> Pick {
        let opts = infer::Options {
            enabled: auto,
            tol: self.view.tol_pt / self.view.camera.pt_per_mm,
        };
        let snap = self.tools.host.tool().snap();
        let anchor = snap.anchor.map(|(pos, point)| infer::Anchor { pos, point });
        infer::infer(
            &self.doc.scene.items,
            opts,
            snap.kind,
            anchor,
            snap.curve,
            world,
        )
    }

    /// Clicks, typed values and keys while a drawing tool is active (#68).
    pub(crate) fn tool_input(
        &mut self,
        ui: &egui::Ui,
        resp: &egui::Response,
        rect: egui::Rect,
        settings: &Settings,
    ) {
        self.selection.hover = None;
        let auto = settings.auto_constrain && !ui.input(|i| i.modifiers.alt);
        self.tools.cursor = resp
            .hover_pos()
            .map(|p| self.designate(self.view.camera.to_world(p, rect), auto));
        if resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
        }
        if resp.hovered() || !self.tools.typed.is_empty() {
            self.tool_keys(ui, settings.units);
        }
        if self.tool() == Tool::Bezier {
            self.pen_drag(ui, resp, rect, auto, settings.units);
        }
        if resp.clicked_by(PointerButton::Primary)
            && let Some(pick) = self.tools.cursor
        {
            self.tool_event(ToolEvent::Click(pick), None, settings.units);
            self.tools.typed.clear();
        }
    }

    /// Clicks for the modify tools (SPEC §6.2): the curve under the pointer is
    /// trimmed, extended or broken as one undo step, and any constraint the
    /// step removes is named in the status bar (SPEC §4 rule 5).
    pub(crate) fn modify_input(
        &mut self,
        ui: &egui::Ui,
        resp: &egui::Response,
        rect: egui::Rect,
        units: Units,
    ) {
        let tol = self.view.tol_pt / self.view.camera.pt_per_mm;
        let at = resp.hover_pos().map(|p| self.view.camera.to_world(p, rect));
        self.selection.hover = at.and_then(|w| hit::pick_curve(&self.doc.scene.items, w, tol));
        if resp.hovered() {
            ui.ctx().set_cursor_icon(if self.selection.hover.is_some() {
                egui::CursorIcon::PointingHand
            } else {
                egui::CursorIcon::Default
            });
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                self.tool_event(ToolEvent::Esc, None, units);
                return;
            }
        }
        if !resp.clicked_by(PointerButton::Primary) {
            return;
        }
        if let Some(w) = at {
            let hover = self.selection.hover;
            self.tool_event(ToolEvent::Click(Pick::free(w)), hover, units);
        }
    }

    /// Click-drag with the pen: the press places the anchor, the drag pulls
    /// out its handle and the release commits the curve (#160).
    pub(crate) fn pen_drag(
        &mut self,
        ui: &egui::Ui,
        resp: &egui::Response,
        rect: egui::Rect,
        auto: bool,
        units: Units,
    ) {
        if resp.drag_started_by(PointerButton::Primary)
            && let Some(o) = ui.input(|i| i.pointer.press_origin())
        {
            let pick = self.designate(self.view.camera.to_world(o, rect), auto);
            self.tools.pen = Some((pick, pick.pos));
        }
        if resp.dragged_by(PointerButton::Primary)
            && let (Some((pick, _)), Some(p)) = (self.tools.pen, resp.interact_pointer_pos())
        {
            self.tools.pen = Some((pick, self.view.camera.to_world(p, rect)));
        }
        if resp.drag_stopped_by(PointerButton::Primary)
            && let Some((pick, now)) = self.tools.pen.take()
        {
            self.tool_event(
                ToolEvent::Drag {
                    from: pick,
                    to: now,
                },
                None,
                units,
            );
            self.tools.typed.clear();
        }
    }

    pub(crate) fn tool_keys(&mut self, ui: &egui::Ui, units: Units) {
        use egui::Key;
        let (text, enter, esc, back) = ui.input(|i| {
            let text: String = i
                .events
                .iter()
                .filter_map(|e| match e {
                    egui::Event::Text(t) => Some(t.as_str()),
                    _ => None,
                })
                .collect();
            (
                text,
                i.key_pressed(Key::Enter),
                i.key_pressed(Key::Escape),
                i.key_pressed(Key::Backspace),
            )
        });
        if self.is_drawing() {
            self.tools
                .typed
                .extend(text.chars().filter(|c| c.is_ascii_digit() || *c == '.'));
        }
        if back {
            self.tools.typed.pop();
        }
        if enter {
            let value = std::mem::take(&mut self.tools.typed);
            if value.is_empty() {
                self.tool_event(ToolEvent::Enter, None, units);
            } else if let Ok(v) = value.parse::<f64>() {
                let mm = match units {
                    Units::Mm => v,
                    Units::Inch => v * 25.4,
                };
                let at = self.tools.cursor.unwrap_or_else(|| Pick::free(Vec2::ZERO));
                self.tool_event(ToolEvent::Typed(mm, at), None, units);
            } else {
                self.doc.notice = Some(format!("\"{value}\" is not a number"));
            }
        }
        if esc {
            if !self.tools.typed.is_empty() {
                self.tools.typed.clear();
            } else {
                // Cancels the step; with nothing pending, back to Select.
                self.tool_event(ToolEvent::Esc, None, units);
            }
        }
    }

    pub(crate) fn draw_tool_overlay(&self, frame: &Frame<'_>, painter: &egui::Painter) {
        if self.tool() == Tool::Select {
            return;
        }
        let Some(pick) = self.tools.cursor else {
            return;
        };
        let tool = self.tools.host.tool();
        let pen = self.tools.pen.map(|(anchor, now)| (anchor.pos, now));
        frame.draw_preview(&tool.preview(pick.pos, pen));
        if pick.point.is_some() {
            frame.draw_snap(pick.pos);
        }
        // Inference hint glyph (#69): what the shape will be constrained to.
        let badge = match (pick.line, pick.on_curve) {
            (Some(h), _) => Some(h.badge()),
            (None, Some(_)) => Some("o"),
            (None, None) => pick.point.map(|_| "•"),
        };
        if let Some(b) = badge {
            frame.draw_hint(pick.pos, b);
        }
        let label = if self.tools.typed.is_empty() {
            tool.readout(pick.pos)
                .map(|(what, mm)| format!("{what} {}", frame.units.format_len(mm)))
        } else {
            let what = tool.readout(pick.pos).map_or("Value", |(w, _)| w);
            Some(format!("{what} {}▏", self.tools.typed))
        };
        if let Some(text) = label {
            let at = frame.view.to_screen(pick.pos, frame.rect) + egui::vec2(14.0, -14.0);
            // Keep the label on the canvas near the right edge.
            let anchor = if at.x > frame.rect.right() - 170.0 {
                egui::Align2::RIGHT_BOTTOM
            } else {
                egui::Align2::LEFT_BOTTOM
            };
            painter.text(
                at - egui::vec2(
                    if anchor == egui::Align2::RIGHT_BOTTOM {
                        28.0
                    } else {
                        0.0
                    },
                    0.0,
                ),
                anchor,
                text,
                egui::FontId::monospace(13.0),
                frame.pal.hover,
            );
        }
    }

    /// One frame of a drag: re-solve towards the pointer; release records
    /// one undo step; Esc puts everything back (#71).
    pub(crate) fn drag_frame(&mut self, ui: &egui::Ui, resp: &egui::Response, rect: egui::Rect) {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.doc.sketcher.cancel_drag();
            self.tools.dragging = false;
            self.rebuild();
            return;
        }
        if resp.dragged_by(PointerButton::Primary)
            && let Some(p) = resp.interact_pointer_pos()
        {
            let to = self.view.camera.to_world(p, rect);
            match self.doc.sketcher.drag_to(to) {
                Ok(f) if f.converged => {
                    // Keep the selection: rebuild() prunes it by id, and a
                    // drag never removes entities.
                    self.rebuild();
                }
                Ok(_) => {}
                Err(e) => {
                    self.doc.sketcher.cancel_drag();
                    self.tools.dragging = false;
                    self.doc.notice = Some(e.to_string());
                    self.rebuild();
                    return;
                }
            }
        }
        if resp.drag_stopped_by(PointerButton::Primary) || !resp.dragged() {
            self.tools.dragging = false;
            if let Err(e) = self.doc.sketcher.end_drag() {
                self.doc.notice = Some(e.to_string());
            }
            self.rebuild();
        }
    }

    /// Hover, click and box selection (#67). Left-drag is a rubber-band box;
    /// pan stays on middle/right-drag.
    pub(crate) fn pick_and_select(
        &mut self,
        ui: &egui::Ui,
        resp: &egui::Response,
        rect: egui::Rect,
        units: Units,
    ) {
        let mods = ui.input(|i| Mods {
            add: i.modifiers.shift || self.view.touch_add,
            toggle: i.modifiers.command || self.view.touch_toggle,
        });
        let tol = self.view.tol_pt / self.view.camera.pt_per_mm;
        let pick_at = |this: &Self, p: Pos2| {
            let w = this.view.camera.to_world(p, rect);
            hit::pick(&this.doc.scene.items, w, tol)
        };

        if self.tools.dragging {
            self.drag_frame(ui, resp, rect);
            return;
        }
        if resp.drag_started_by(PointerButton::Primary) {
            // The press, not where the drag threshold was crossed.
            let origin = ui.input(|i| i.pointer.press_origin());
            match origin.and_then(|o| pick_at(self, o).map(|id| (o, id))) {
                Some((o, id)) if self.begin_drag(id, self.view.camera.to_world(o, rect)) => {
                    self.selection.items.set_all([id]);
                }
                _ => self.view.rubber = resp.interact_pointer_pos(),
            }
        }
        let boxing = self.view.rubber.is_some();
        self.selection.hover = match resp.hover_pos() {
            Some(p) if !boxing => pick_at(self, p),
            _ => None,
        };
        // Inside a region but off its curves: shade the region (#122).
        self.selection.regions.hover = match resp.hover_pos() {
            Some(p) if !boxing && self.selection.hover.is_none() => {
                self.region_at(self.view.camera.to_world(p, rect))
            }
            _ => None,
        };
        if self.selection.hover.is_some() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }

        if resp.double_clicked_by(PointerButton::Primary)
            && let Some(p) = resp.interact_pointer_pos()
            && let Some(&(rid, _)) = self.view.offset_rects.iter().find(|(_, r)| r.contains(p))
        {
            if self
                .doc
                .sketcher
                .document()
                .recipe(rid)
                .is_some_and(|r| r.kind.is_pattern())
            {
                self.edit_pattern(rid, units);
            } else {
                self.edit_offset(rid, units);
            }
        } else if resp.double_clicked_by(PointerButton::Primary)
            && let Some(p) = resp.interact_pointer_pos()
            && let Some(&(id, _)) = self.view.dim_rects.iter().find(|(_, r)| r.contains(p))
        {
            self.edit_existing_dimension(id, units);
        } else if resp.clicked_by(PointerButton::Primary)
            && let Some(p) = resp.interact_pointer_pos()
        {
            let hit = pick_at(self, p);
            let region = self.region_at(self.view.camera.to_world(p, rect));
            let plain = !(mods.add || mods.toggle);
            if self.selection.regions.combine.is_some() {
                // Picking tool regions for a boolean (#124).
                if hit.is_none() {
                    self.combine_click(region);
                }
            } else if hit.is_some() || region.is_none() {
                self.selection.items.click(hit, mods);
                if plain {
                    self.selection.regions.selected.clear();
                }
            } else {
                // Regions select alongside entities, by the same rules.
                self.selection.regions.click(region.as_ref(), mods);
                if plain {
                    self.selection.items.clear();
                }
            }
        }
        if resp.drag_stopped_by(PointerButton::Primary)
            && let Some(start) = self.view.rubber.take()
            && let Some(end) = resp.interact_pointer_pos()
            && (end - start).length() >= MIN_BOX_PT
        {
            let a = self.view.camera.to_world(start, rect);
            let b = self.view.camera.to_world(end, rect);
            let mode = BoxMode::from_drag(start.x, end.x);
            let hits = hit::box_select(&self.doc.scene.items, Aabb::from_points(a, b), mode);
            self.selection.items.apply_box(&hits, mods);
        }
        if !resp.dragged_by(PointerButton::Primary) {
            self.view.rubber = None;
        }

        if resp.hovered() {
            let esc = ui.input(|i| i.key_pressed(egui::Key::Escape));
            let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
            if esc && self.selection.regions.combine.is_some() {
                self.cancel_combine();
            } else if esc {
                self.doc.refusal = None;
                self.clear_selection();
            }
            if enter && self.selection.regions.combine.is_some() {
                self.finish_combine();
            }
        }
    }
}

/// Window boxes are solid blue, crossing boxes dashed green (Fusion style).
fn draw_rubber_band(painter: &egui::Painter, start: Pos2, now: Pos2) {
    let r = egui::Rect::from_two_pos(start, now);
    let window = BoxMode::from_drag(start.x, now.x) == BoxMode::Window;
    let c = if window {
        Color32::from_rgb(70, 140, 240)
    } else {
        Color32::from_rgb(70, 190, 110)
    };
    painter.rect_filled(r, 0.0, c.gamma_multiply(0.15));
    let stroke = Stroke::new(1.0, c);
    if window {
        painter.rect_stroke(r, 0.0, stroke, egui::StrokeKind::Inside);
    } else {
        let path = [
            r.left_top(),
            r.right_top(),
            r.right_bottom(),
            r.left_bottom(),
            r.left_top(),
        ];
        painter.extend(egui::Shape::dashed_line(&path, stroke, 6.0, 4.0));
    }
}
