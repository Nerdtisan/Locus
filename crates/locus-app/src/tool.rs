// SPDX-License-Identifier: AGPL-3.0-or-later
//! The tool trait (#294, M5 design §7.2): one interaction model for every
//! tool family. Drawing tools (including text and the Bézier pen), modify
//! tools and constraints all take [`ToolEvent`]s and answer with a [`Flow`];
//! a commit is one `locus_doc::Command`, which the [`ToolHost`]'s owner runs
//! through the `Sketcher` (SPEC §11). Tools never touch the document.
//!
//! The canvas view turns pointer, touch and key input into events, keeps
//! the typed value (the heads-up field) and handles Enter / Esc the same way
//! for every tool: Enter commits or finishes, Esc cancels the step, a second
//! Esc returns to Select.

use crate::actions::{ActionId, BUILTIN, Op};
use crate::constrain::{self, Kind};
use crate::hit;
use crate::modify_tool::Modify;
use crate::scene::{Scene, Shape};
use crate::settings::Units;
use crate::tools::{Draft, Outcome, Pick, Tool as DrawKind};
use locus_doc::{Command, Document, EntityId, LayerId, Style, TextData};
use locus_geom::Vec2;
use std::borrow::Cow;

/// What a tool is, for menus (which entry is highlighted) and the status
/// bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolKind {
    /// Select, or a drawing tool.
    Draw(DrawKind),
    Modify(Modify),
    Constrain(Kind),
}

/// A selection box a tool fills from the selection or from picks
/// (design §3: selection-first or tool-first).
#[allow(dead_code)] // first reader: the command dialogs (#218, #219, #221)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Need {
    /// What the constraint's own rule asks for ([`Kind::needs`]).
    Selection,
    /// One curve, picked on the canvas.
    Curve,
    /// Two curves meeting at a corner, or the corner point.
    Corner,
}

/// Input the canvas hands a tool.
#[derive(Clone, Copy, Debug)]
pub enum ToolEvent {
    /// A primary click at the designated point.
    Click(Pick),
    /// Press, drag and release (the pen's smooth point): where it went down
    /// and where it came up.
    Drag { from: Pick, to: Vec2 },
    /// A value typed into the heads-up field, in mm, with the cursor.
    Typed(f64, Pick),
    /// Enter with nothing typed: finish (end a polyline).
    Enter,
    /// Esc with nothing typed.
    Esc,
    /// The tool was picked with the selection already made (constraints).
    Activate,
}

/// What a tool wants after an event (design §7.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flow {
    /// Keep going.
    Continue,
    /// [`Tool::commit`] has a command to run.
    Commit,
    /// The step was cancelled; the tool stays active.
    Cancel,
    /// The tool is finished: return to Select.
    Done,
}

/// What a tool reads, and the requests it makes back. Read-only access to
/// the document: changes come back as a command from [`Tool::commit`].
pub struct ToolCx<'a> {
    pub doc: &'a Document,
    pub scene: &'a Scene,
    /// Pick radius in mm at the current zoom.
    pub tol: f64,
    pub units: Units,
    /// The layer new geometry goes on (#118).
    pub layer: LayerId,
    /// The style new geometry gets (design §6.2; #287).
    pub style: Style,
    /// New geometry is construction (the Linetype switch, #289).
    pub construction: bool,
    pub selection: Vec<EntityId>,
    /// The curve under the pointer, if any.
    pub hover: Option<EntityId>,
    /// Laid-out text for the Text tool (#180), when it could be laid out.
    pub text: Option<TextData>,
    // ── Requests back to the host ──
    /// Replace the status-bar notice (`Some(None)` clears it).
    pub notice: Option<Option<String>>,
    /// Replace the selection.
    pub select: Option<Vec<EntityId>>,
    /// An offset click: pick or create an offset on this curve (#164).
    pub offset: Option<EntityId>,
    /// A fillet or chamfer corner was picked: the two curves, for the
    /// command dialog to size and commit (#217).
    pub corner_pick: Option<(EntityId, EntityId)>,
}

/// What a commit left behind, for the host to show.
#[derive(Debug, Default)]
pub struct After {
    /// Select this (the shape just placed).
    pub select: Option<EntityId>,
    /// Tell the user this (constraints a trim removed).
    pub notice: Option<String>,
}

/// Inference hints for the cursor (#69).
#[derive(Clone, Copy, Debug)]
pub struct Snap {
    pub kind: DrawKind,
    /// The point a line is being drawn from.
    pub anchor: Option<(Vec2, Option<EntityId>)>,
    /// Snap onto curves, not only points.
    pub curve: bool,
}

pub trait Tool {
    /// The registry action that makes this tool active.
    #[allow(dead_code)] // first readers: the ribbon highlight and tool search (#299)
    fn id(&self) -> ActionId;
    fn kind(&self) -> ToolKind;
    /// The status-bar prompt for the next step.
    fn prompt(&self) -> Cow<'static, str>;
    /// The selection boxes it fills.
    #[allow(dead_code)] // first reader: the command dialogs (#218, #219, #221)
    fn needs(&self) -> &'static [Need] {
        &[]
    }
    /// Mid-step: clicks are pending, so Esc cancels the step and typed
    /// digits go to the heads-up field.
    fn busy(&self) -> bool {
        false
    }
    fn event(&mut self, ev: ToolEvent, cx: &mut ToolCx<'_>) -> Flow;
    /// The command for a `Flow::Commit`: one undo step (SPEC §11).
    fn commit(&mut self) -> Option<Command> {
        None
    }
    /// The commit ran (`ok`) or was refused.
    fn committed(&mut self, _ok: bool) -> After {
        After::default()
    }
    /// Rubber-band geometry at the cursor; `pen` is a handle being dragged.
    fn preview(&self, _cursor: Vec2, _pen: Option<(Vec2, Vec2)>) -> Vec<Shape> {
        Vec::new()
    }
    /// The heads-up value at the cursor: what it is and its length in mm.
    fn readout(&self, _cursor: Vec2) -> Option<(&'static str, f64)> {
        None
    }
    fn snap(&self) -> Snap {
        Snap {
            kind: DrawKind::Select,
            anchor: None,
            curve: false,
        }
    }
    /// Forget a half-made step (undo, a load, a failed command).
    fn reset(&mut self) {}
    /// Options shown in the tool strip (palette, later).
    fn dialog(&mut self, _ui: &mut egui::Ui) {}
    /// Polygon sides and circumscribed flag, for a polygon tool (#157).
    fn polygon(&self) -> Option<(usize, bool)> {
        None
    }
    #[cfg(test)]
    fn dialog_set_polygon_for_test(&mut self, _sides: usize, _circumscribed: bool) {}
}

/// The id of the built-in action that runs `op`.
fn id_of(op: &Op) -> ActionId {
    BUILTIN
        .iter()
        .find(|a| &a.op == op)
        .map_or(ActionId::new("select.select"), |a| a.id.clone())
}

// ── Drawing tools, text and the pen ─────────────────────────────────────

/// Select and every drawing tool: a click sequence with a typed field
/// (#68). The pen's press-drag makes smooth points (#160); text takes its
/// laid-out content from the context (#180).
pub struct DrawTool {
    draft: Draft,
    pending: Option<Command>,
}

impl DrawTool {
    pub fn new(kind: DrawKind, layer: LayerId) -> Self {
        let mut draft = Draft::new(kind);
        draft.set_layer(layer);
        Self {
            draft,
            pending: None,
        }
    }

    pub fn set_polygon(&mut self, sides: usize, circumscribed: bool) {
        self.draft.set_polygon(sides, circumscribed);
    }

    fn outcome(&mut self, out: Outcome, cx: &mut ToolCx<'_>) -> Flow {
        match out {
            Outcome::Pending => Flow::Continue,
            Outcome::Rejected(why) => {
                cx.notice = Some(Some(why.to_owned()));
                Flow::Continue
            }
            Outcome::Commit(cmd) => {
                self.pending = Some(cmd);
                Flow::Commit
            }
        }
    }
}

impl Tool for DrawTool {
    fn id(&self) -> ActionId {
        id_of(&Op::Draw(self.draft.tool()))
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Draw(self.draft.tool())
    }

    fn prompt(&self) -> Cow<'static, str> {
        Cow::Borrowed(self.draft.tool().prompt(self.draft.clicks()))
    }

    fn busy(&self) -> bool {
        self.draft.is_drawing()
    }

    fn event(&mut self, ev: ToolEvent, cx: &mut ToolCx<'_>) -> Flow {
        self.draft.set_layer(cx.layer);
        self.draft.set_style(cx.style);
        self.draft.set_construction(cx.construction);
        let next = cx.doc.next_id();
        match ev {
            ToolEvent::Click(pick) => {
                if self.draft.tool() == DrawKind::Text {
                    self.draft.set_text(cx.text.clone());
                }
                let out = self.draft.click(pick, next);
                self.outcome(out, cx)
            }
            ToolEvent::Drag { from, to } => {
                let out = self.draft.pen(from, Some(to - from.pos), next);
                self.outcome(out, cx)
            }
            ToolEvent::Typed(mm, at) => {
                let out = self.draft.typed(mm, at, next);
                self.outcome(out, cx)
            }
            ToolEvent::Enter => {
                self.draft.finish();
                Flow::Continue
            }
            ToolEvent::Esc if self.draft.is_drawing() => {
                self.draft.cancel();
                Flow::Cancel
            }
            ToolEvent::Esc => Flow::Done,
            ToolEvent::Activate => Flow::Continue,
        }
    }

    fn commit(&mut self) -> Option<Command> {
        self.pending.take()
    }

    fn committed(&mut self, ok: bool) -> After {
        if ok {
            After {
                select: self.draft.take_placed(),
                notice: None,
            }
        } else {
            // The draft's predicted IDs are void after a failure.
            self.draft.cancel();
            After::default()
        }
    }

    fn preview(&self, cursor: Vec2, pen: Option<(Vec2, Vec2)>) -> Vec<Shape> {
        match pen {
            Some((anchor, now)) => self.draft.preview_pen(anchor, now),
            None => self.draft.preview(cursor),
        }
    }

    fn readout(&self, cursor: Vec2) -> Option<(&'static str, f64)> {
        self.draft.readout(cursor)
    }

    fn snap(&self) -> Snap {
        Snap {
            kind: self.draft.tool(),
            anchor: self.draft.line_anchor(),
            curve: self.draft.curve_snap(),
        }
    }

    fn reset(&mut self) {
        self.draft.cancel();
        self.pending = None;
    }

    fn polygon(&self) -> Option<(usize, bool)> {
        Some(self.draft.polygon())
    }

    #[cfg(test)]
    fn dialog_set_polygon_for_test(&mut self, sides: usize, circumscribed: bool) {
        self.draft.set_polygon(sides, circumscribed);
    }

    fn dialog(&mut self, ui: &mut egui::Ui) {
        if self.draft.tool() != DrawKind::Polygon {
            return;
        }
        let (mut sides, mut outside) = self.draft.polygon();
        let changed = ui
            .add(
                egui::DragValue::new(&mut sides)
                    .range(3..=64)
                    .suffix(" sides"),
            )
            .changed()
            | ui.checkbox(&mut outside, "Circumscribed").changed();
        if changed {
            self.draft.set_polygon(sides, outside);
        }
        ui.separator();
    }
}

// ── Modify tools ────────────────────────────────────────────────────────

/// Trim, extend, break, fillet, chamfer and offset (SPEC §6.2): pick a
/// curve, or two curves (or the corner they share). A fillet or chamfer
/// corner goes to the command dialog, which sizes it with a live preview
/// and commits it (#217). Move/rotate/scale has its own manipulator.
pub struct ModifyTool {
    m: Modify,
    /// The first curve picked for a fillet or chamfer.
    first: Option<EntityId>,
    pending: Option<Command>,
    /// Constraints the pending step removes, named for the status bar
    /// (SPEC §4 rule 5).
    dropped: Vec<String>,
}

impl ModifyTool {
    pub fn new(m: Modify) -> Self {
        Self {
            m,
            first: None,
            pending: None,
            dropped: Vec::new(),
        }
    }
}

impl Tool for ModifyTool {
    fn id(&self) -> ActionId {
        id_of(&Op::Modify(self.m))
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Modify(self.m)
    }

    fn prompt(&self) -> Cow<'static, str> {
        Cow::Borrowed(self.m.prompt(self.first.is_some()))
    }

    fn needs(&self) -> &'static [Need] {
        if self.m.takes_two() {
            &[Need::Corner]
        } else {
            &[Need::Curve]
        }
    }

    fn busy(&self) -> bool {
        self.first.is_some()
    }

    fn event(&mut self, ev: ToolEvent, cx: &mut ToolCx<'_>) -> Flow {
        let at = match ev {
            ToolEvent::Esc => {
                cx.select = Some(Vec::new());
                cx.notice = Some(None);
                return Flow::Done;
            }
            ToolEvent::Click(p) => p.pos,
            _ => return Flow::Continue,
        };
        if let Modify::Transform(_) = self.m {
            return Flow::Continue;
        }
        if self.m == Modify::Offset {
            cx.offset = cx.hover;
            return Flow::Continue;
        }
        let doc = cx.doc;
        if self.m.takes_two() {
            // A corner point shared by exactly two curves picks both.
            let corner = hit::pick_point(&cx.scene.items, at, cx.tol)
                .map(|(p, _)| doc.dependents(p).0)
                .filter(|users| users.len() == 2);
            let pair = match (corner, self.first, cx.hover) {
                (Some(users), _, _) => (users[0], users[1]),
                (None, None, Some(id)) => {
                    self.first = Some(id);
                    cx.select = Some(vec![id]);
                    cx.notice = Some(None);
                    return Flow::Continue;
                }
                (None, Some(first), Some(id)) if first != id => (first, id),
                (None, _, _) => return Flow::Continue,
            };
            self.first = None;
            cx.select = Some(vec![pair.0, pair.1]);
            cx.notice = Some(None);
            cx.corner_pick = Some(pair);
            return Flow::Continue;
        }
        let Some(id) = cx.hover else {
            return Flow::Continue;
        };
        let plan = self.m.plan(doc, id, at);
        self.first = None;
        cx.select = Some(Vec::new());
        match plan {
            Err(e) => {
                cx.notice = Some(Some(e.to_string()));
                Flow::Continue
            }
            Ok(plan) => {
                self.dropped = plan
                    .dropped
                    .iter()
                    .filter_map(|c| doc.constraint(*c))
                    .map(|c| constrain::describe(doc, c, cx.units))
                    .collect();
                self.pending = Some(plan.command);
                Flow::Commit
            }
        }
    }

    fn commit(&mut self) -> Option<Command> {
        self.pending.take()
    }

    fn committed(&mut self, ok: bool) -> After {
        let names = std::mem::take(&mut self.dropped);
        if !ok || names.is_empty() {
            return After::default();
        }
        let s = if names.len() == 1 { "" } else { "s" };
        After {
            select: None,
            notice: Some(format!(
                "{}: removed {} constraint{s}: {}",
                self.m.label(),
                names.len(),
                names.join("; ")
            )),
        }
    }

    fn reset(&mut self) {
        self.first = None;
        self.pending = None;
    }
}

// ── Constraints ─────────────────────────────────────────────────────────

/// A geometric constraint (#70). Its needs are met by the selection, so it
/// commits as soon as it is picked (selection-first, design §3) and is done.
pub struct ConstrainTool {
    kind: Kind,
    pending: Option<Command>,
}

impl ConstrainTool {
    pub fn new(kind: Kind) -> Self {
        Self {
            kind,
            pending: None,
        }
    }
}

impl Tool for ConstrainTool {
    fn id(&self) -> ActionId {
        id_of(&Op::Constrain(self.kind))
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Constrain(self.kind)
    }

    fn prompt(&self) -> Cow<'static, str> {
        Cow::Owned(format!("Select {}", self.kind.needs()))
    }

    fn event(&mut self, ev: ToolEvent, cx: &mut ToolCx<'_>) -> Flow {
        if !matches!(ev, ToolEvent::Activate) {
            return Flow::Done;
        }
        match constrain::plan(self.kind, cx.doc, &cx.selection) {
            Ok(cmd) => {
                self.pending = Some(cmd);
                Flow::Commit
            }
            Err(why) => {
                cx.notice = Some(Some(why));
                Flow::Done
            }
        }
    }

    fn commit(&mut self) -> Option<Command> {
        self.pending.take()
    }
}

// ── Host ────────────────────────────────────────────────────────────────

/// Owns the active tool (design §7.2). Select is the default tool.
pub struct ToolHost {
    active: Box<dyn Tool>,
    /// Polygon sides and kind, kept across tool changes as they always were.
    polygon: (usize, bool),
}

impl Default for ToolHost {
    fn default() -> Self {
        let select = DrawTool::new(DrawKind::Select, Document::default_layer());
        let polygon = select.draft.polygon();
        Self {
            active: Box::new(select),
            polygon,
        }
    }
}

impl ToolHost {
    pub fn tool(&self) -> &dyn Tool {
        self.active.as_ref()
    }

    pub fn tool_mut(&mut self) -> &mut dyn Tool {
        self.active.as_mut()
    }

    pub fn kind(&self) -> ToolKind {
        self.active.kind()
    }

    /// The drawing tool in use; Select while a modify tool is active.
    pub fn draw_kind(&self) -> DrawKind {
        match self.kind() {
            ToolKind::Draw(k) => k,
            _ => DrawKind::Select,
        }
    }

    pub fn modify(&self) -> Option<Modify> {
        match self.kind() {
            ToolKind::Modify(m) => Some(m),
            _ => None,
        }
    }

    /// Replace the active tool, remembering the polygon options.
    pub fn set(&mut self, tool: Box<dyn Tool>) {
        if let Some(p) = self.active.polygon() {
            self.polygon = p;
        }
        self.active = tool;
    }

    /// Put `tool` in place and hand back the one it replaced, without
    /// touching options: for tools that run once (constraints).
    pub fn swap(&mut self, tool: Box<dyn Tool>) -> Box<dyn Tool> {
        std::mem::replace(&mut self.active, tool)
    }

    /// Make a drawing tool (or Select) active.
    pub fn set_draw(&mut self, kind: DrawKind, layer: LayerId) {
        if let Some(p) = self.active.polygon() {
            self.polygon = p;
        }
        let mut t = DrawTool::new(kind, layer);
        t.set_polygon(self.polygon.0, self.polygon.1);
        self.active = Box::new(t);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::demo;

    fn cx<'a>(doc: &'a Document, scene: &'a Scene) -> ToolCx<'a> {
        ToolCx {
            doc,
            scene,
            tol: 1.0,
            units: Units::Mm,
            layer: Document::default_layer(),
            style: Style::default(),
            construction: false,
            selection: Vec::new(),
            hover: None,
            text: None,
            notice: None,
            select: None,
            offset: None,
            corner_pick: None,
        }
    }

    #[test]
    fn every_tool_names_its_registry_action() {
        assert_eq!(
            DrawTool::new(DrawKind::Line, Document::default_layer())
                .id()
                .as_str(),
            "sketch.line"
        );
        assert_eq!(ModifyTool::new(Modify::Trim).id().as_str(), "modify.trim");
        assert_eq!(
            ConstrainTool::new(Kind::Tangent).id().as_str(),
            "constrain.tangent"
        );
    }

    #[test]
    fn a_line_commits_one_command_per_segment_and_esc_cancels_then_finishes() {
        let doc = Document::new();
        let scene = Scene::default();
        let mut c = cx(&doc, &scene);
        let mut t = DrawTool::new(DrawKind::Line, Document::default_layer());
        assert_eq!(
            t.event(ToolEvent::Click(Pick::free(Vec2::ZERO)), &mut c),
            Flow::Continue
        );
        assert!(t.busy());
        assert_eq!(
            t.event(ToolEvent::Click(Pick::free(Vec2::new(10.0, 0.0))), &mut c),
            Flow::Commit
        );
        assert!(t.commit().is_some());
        assert!(t.commit().is_none(), "a commit is taken once");
        // Esc cancels the step, a second Esc is done (design §3).
        assert_eq!(t.event(ToolEvent::Esc, &mut c), Flow::Cancel);
        assert_eq!(t.event(ToolEvent::Esc, &mut c), Flow::Done);
    }

    #[test]
    fn a_constraint_is_selection_first_and_explains_a_refusal() {
        let doc = demo::sketch();
        let scene = Scene::default();
        let mut c = cx(&doc, &scene);
        let mut t = ConstrainTool::new(Kind::Parallel);
        assert_eq!(t.event(ToolEvent::Activate, &mut c), Flow::Done);
        assert!(matches!(&c.notice, Some(Some(n)) if n.contains("two lines")));
        assert!(t.commit().is_none());
    }

    #[test]
    fn a_chamfer_pick_goes_to_the_dialog_not_straight_to_a_commit() {
        let doc = demo::sketch();
        let scene = Scene::default();
        let lines: Vec<EntityId> = doc
            .entities()
            .iter()
            .filter(|(_, e)| matches!(e.geom, locus_doc::Geom::Line { .. }))
            .map(|(id, _)| id)
            .take(2)
            .collect();
        let mut t = ModifyTool::new(Modify::Chamfer);
        let mut c = cx(&doc, &scene);
        c.hover = Some(lines[0]);
        assert_eq!(
            t.event(ToolEvent::Click(Pick::free(Vec2::ZERO)), &mut c),
            Flow::Continue
        );
        assert!(t.busy());
        let mut c = cx(&doc, &scene);
        c.hover = Some(lines[1]);
        assert_eq!(
            t.event(ToolEvent::Click(Pick::free(Vec2::ZERO)), &mut c),
            Flow::Continue
        );
        assert_eq!(c.corner_pick, Some((lines[0], lines[1])));
        assert!(t.commit().is_none() && !t.busy());
    }

    #[test]
    fn modify_esc_clears_and_is_done() {
        let doc = Document::new();
        let scene = Scene::default();
        let mut c = cx(&doc, &scene);
        let mut t = ModifyTool::new(Modify::Fillet);
        assert_eq!(t.event(ToolEvent::Esc, &mut c), Flow::Done);
        assert_eq!(c.select, Some(Vec::new()));
    }
}
