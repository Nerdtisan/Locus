// SPDX-License-Identifier: AGPL-3.0-or-later
//! The action registry (#294, M5 design §7.1): every command the UI offers,
//! with its id, label, group, symbol, keywords, default keys and enabled
//! predicate, in one table. The menus, the tool strip, the constraint bar,
//! the hotkeys and the touch menu all read from it, so they can't disagree;
//! the ribbon, marking menu, tool search and shortcuts pane (M5 Phase C)
//! read from it too.
//!
//! The registry is UI. Every action that changes the document ends in
//! `Sketcher::apply` through the canvas, so it is one undoable command
//! (SPEC §11); the registry adds no new way to mutate the document.
//!
//! Constraints have no default keys (owner decision, design §2, §5).

use crate::constrain::Kind;
use crate::modify_tool::{Modify, TransformMode};
use crate::settings::Settings;
use crate::state::AppState;
use crate::tools::Tool;
use egui::{Key, KeyboardShortcut, Modifiers};
use std::borrow::Cow;

/// A stable action id: `sketch.line`, `modify.trim`, `constrain.tangent`,
/// `plugin.<id>.<command>`. Ids are what user settings (pinned toolbar
/// actions, key overrides) refer to, so they never change once shipped.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ActionId(pub Cow<'static, str>);

impl ActionId {
    pub const fn new(id: &'static str) -> Self {
        Self(Cow::Borrowed(id))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The ribbon group an action belongs to, in Fusion's naming (design §2).
#[allow(dead_code)] // Output and File fill in with the OUTPUT ribbon tab
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Group {
    Create,
    Modify,
    Constrain,
    Inspect,
    Insert,
    Select,
    Output,
    Edit,
    View,
    File,
}

/// Whether an action can run now; the reason shows in menus and tool search
/// instead of the action being hidden.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Enabled {
    Yes,
    No(Cow<'static, str>),
}

impl Enabled {
    pub fn yes(&self) -> bool {
        matches!(self, Enabled::Yes)
    }

    fn when(ok: bool, why: impl Into<Cow<'static, str>>) -> Self {
        if ok {
            Enabled::Yes
        } else {
            Enabled::No(why.into())
        }
    }
}

/// Where an action's key is live. Plain-letter keys only act while the
/// pointer is over the canvas and nothing else wants the keyboard (a text
/// field, the dimension editor, a typed value), as before the registry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyScope {
    /// Anywhere, unless a text field has focus (undo, redo, select all).
    Global,
    /// Pointer over the canvas.
    Canvas,
    /// Pointer over the canvas, with Select active and no modify tool.
    CanvasSelect,
}

/// A view setting an action toggles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flag {
    AutoConstrain,
    MachinePreview,
    StrokeWidth,
    /// Drawing tools make construction geometry (design §6.10, #289).
    ConstructionLinetype,
}

/// The two plugin kinds that become actions (design §6.13). Exporters
/// register formats with the export dialog instead; importers are picked by
/// file type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PluginKind {
    Generator,
    Modifier,
}

/// What an action does.
#[derive(Clone, Debug, PartialEq)]
pub enum Op {
    /// Make a drawing tool (or Select) active.
    Draw(Tool),
    /// Make a modify tool active.
    Modify(Modify),
    Constrain(Kind),
    Dimension,
    Undo,
    Redo,
    SelectAll,
    Deselect,
    /// Delete the selection and what depends on it (#223).
    Delete,
    /// Toggle the selection between normal and construction (#289).
    ToggleConstruction,
    /// Run the last tool or command again (marking menu N slot, design §4).
    Repeat,
    /// Grow the selection to the chains of its curves (#220).
    SelectChain,
    Fit,
    /// Something only the shell can do, because it owns the dock, its
    /// windows and dialogs; this only asks it to (see [`Shell`]).
    Shell(Shell),
    Toggle(Flag),
    /// A plugin command (SPEC §15), run by the [`PluginHost`].
    Plugin {
        plugin: String,
        kind: PluginKind,
    },
}

/// A request the registry passes to the shell, which drains them from
/// `ViewState::shell` each frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shell {
    /// Show or hide the shortcuts pane (design §5.1, #299).
    ToggleShortcuts,
    /// Put every pane back where the default layout has it (#327, design §2.4).
    ResetLayout,
    /// Show a pane if it is hidden (the Browser's "Edit parameters", #326).
    ShowPane(crate::layout::Tab),
}

/// The three kinds of action in the design (§7.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActionKind {
    /// Makes a tool active; shown highlighted while it is.
    Tool,
    /// Runs once.
    Command,
    /// Flips a setting; shown as a checkbox.
    Toggle,
}

impl Action {
    /// Is this worth repeating (design §4 N slot)? Tools and commands that
    /// make or change something; not history, selection, view or Select.
    fn repeatable(&self) -> bool {
        match &self.op {
            Op::Draw(t) => *t != Tool::Select,
            Op::Modify(_)
            | Op::Constrain(_)
            | Op::Dimension
            | Op::Delete
            | Op::ToggleConstruction
            | Op::Plugin { .. } => true,
            Op::Undo
            | Op::Redo
            | Op::Repeat
            | Op::SelectAll
            | Op::SelectChain
            | Op::Deselect
            | Op::Fit
            | Op::Shell(_)
            | Op::Toggle(_) => false,
        }
    }
}

impl Op {
    pub fn kind(&self) -> ActionKind {
        match self {
            Op::Draw(_) | Op::Modify(_) => ActionKind::Tool,
            Op::Toggle(_) => ActionKind::Toggle,
            _ => ActionKind::Command,
        }
    }
}

/// Built-in or contributed by a plugin (menus tag the latter PLUGIN).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    Builtin,
    Plugin(String),
}

#[derive(Clone, Debug)]
pub struct Action {
    pub id: ActionId,
    pub label: Cow<'static, str>,
    pub group: Group,
    /// A text symbol. The ribbon and shortcuts pane paint vector icons
    /// instead (`views::icons`, #299): the bundled fonts lack most of these
    /// glyphs.
    #[allow(dead_code)] // superseded by views::icons; drop with the next registry change
    pub icon: &'static str,
    /// Extra words tool search matches (#299).
    #[allow(dead_code)] // first reader: tool search
    pub keywords: &'static [&'static str],
    /// Hover text: what it does or what it needs.
    pub hint: Cow<'static, str>,
    /// Default keys; the first is the one menus show.
    pub keys: &'static [KeyboardShortcut],
    pub scope: KeyScope,
    pub op: Op,
    pub source: Source,
}

/// What actions read and act on. The registry acts on [`AppState`] (design §7.3).
pub struct Cx<'a> {
    pub state: &'a mut AppState,
    pub settings: &'a mut Settings,
    /// Runs plugin actions; `None` where no plugins are loaded (tests).
    pub plugins: Option<&'a mut dyn PluginHost>,
}

/// The plugin side of plugin actions: desktop (wasmtime) and web (Worker)
/// hosts both implement it.
pub trait PluginHost {
    fn run_action(&mut self, plugin: &str, kind: PluginKind, canvas: &AppState);
}

const fn key(k: Key) -> KeyboardShortcut {
    KeyboardShortcut::new(Modifiers::NONE, k)
}

const fn cmd(k: Key) -> KeyboardShortcut {
    KeyboardShortcut::new(Modifiers::COMMAND, k)
}

const fn cmd_shift(k: Key) -> KeyboardShortcut {
    KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), k)
}

#[allow(clippy::too_many_arguments)]
const fn a(
    id: &'static str,
    label: &'static str,
    group: Group,
    icon: &'static str,
    keywords: &'static [&'static str],
    hint: &'static str,
    keys: &'static [KeyboardShortcut],
    scope: KeyScope,
    op: Op,
) -> Action {
    Action {
        id: ActionId::new(id),
        label: Cow::Borrowed(label),
        group,
        icon,
        keywords,
        hint: Cow::Borrowed(hint),
        keys,
        scope,
        op,
        source: Source::Builtin,
    }
}

const fn draw(
    id: &'static str,
    group: Group,
    icon: &'static str,
    keywords: &'static [&'static str],
    keys: &'static [KeyboardShortcut],
    tool: Tool,
) -> Action {
    a(
        id,
        tool.label(),
        group,
        icon,
        keywords,
        "",
        keys,
        KeyScope::Canvas,
        Op::Draw(tool),
    )
}

const fn modify(
    id: &'static str,
    group: Group,
    icon: &'static str,
    keys: &'static [KeyboardShortcut],
    m: Modify,
) -> Action {
    a(
        id,
        m.label(),
        group,
        icon,
        &[],
        "",
        keys,
        KeyScope::Canvas,
        Op::Modify(m),
    )
}

/// Constraints: symbol buttons with no keys (owner decision, design §5).
const fn constraint(id: &'static str, icon: &'static str, k: Kind) -> Action {
    a(
        id,
        k.label(),
        Group::Constrain,
        icon,
        &["constraint"],
        k.needs(),
        &[],
        KeyScope::CanvasSelect,
        Op::Constrain(k),
    )
}

/// Every built-in action. Keys follow the design's shortcut map (§5) for
/// the actions that exist today.
pub static BUILTIN: &[Action] = &[
    // ── Select ──
    draw(
        "select.select",
        Group::Select,
        "⬉",
        &["pointer"],
        &[],
        Tool::Select,
    ),
    // ── Create ──
    draw(
        "sketch.point",
        Group::Create,
        "•",
        &[],
        &[key(Key::P)],
        Tool::Point,
    ),
    draw(
        "sketch.line",
        Group::Create,
        "╱",
        &["polyline"],
        &[key(Key::L)],
        Tool::Line,
    ),
    draw(
        "sketch.rect_2pt",
        Group::Create,
        "▭",
        &["rectangle", "box"],
        &[key(Key::R)],
        Tool::RectCorner,
    ),
    draw(
        "sketch.rect_centre",
        Group::Create,
        "▭",
        &["rectangle", "center"],
        &[],
        Tool::RectCentre,
    ),
    draw(
        "sketch.rect_3pt",
        Group::Create,
        "▱",
        &["rectangle", "rotated"],
        &[],
        Tool::RectThree,
    ),
    draw(
        "sketch.circle_centre",
        Group::Create,
        "◯",
        &["center", "diameter"],
        &[key(Key::C)],
        Tool::Circle,
    ),
    draw(
        "sketch.circle_2pt",
        Group::Create,
        "◯",
        &["diameter"],
        &[],
        Tool::Circle2,
    ),
    draw(
        "sketch.circle_3pt",
        Group::Create,
        "◯",
        &[],
        &[],
        Tool::Circle3,
    ),
    draw(
        "sketch.arc_3pt",
        Group::Create,
        "◠",
        &[],
        &[key(Key::A)],
        Tool::ArcThreePoint,
    ),
    draw(
        "sketch.arc_centre",
        Group::Create,
        "◠",
        &["center"],
        &[],
        Tool::ArcCentre,
    ),
    draw(
        "sketch.arc_tangent",
        Group::Create,
        "◠",
        &[],
        &[],
        Tool::ArcTangent,
    ),
    draw(
        "sketch.polygon",
        Group::Create,
        "⬡",
        &["hexagon"],
        &[key(Key::G)],
        Tool::Polygon,
    ),
    draw(
        "sketch.slot",
        Group::Create,
        "⊂⊃",
        &[],
        &[key(Key::U)],
        Tool::Slot,
    ),
    draw(
        "sketch.pen",
        Group::Create,
        "✒",
        &["bezier", "spline", "curve"],
        &[key(Key::N)],
        Tool::Bezier,
    ),
    draw(
        "sketch.text",
        Group::Create,
        "A",
        &["font", "lettering"],
        &[],
        Tool::Text,
    ),
    // Offset creates geometry, so it sits in Create as in Fusion (design §2).
    modify(
        "modify.offset",
        Group::Create,
        "⧉",
        &[key(Key::O)],
        Modify::Offset,
    ),
    // Patterns create geometry, so they sit in Create as in Fusion (design
    // §2, §6.8, #221).
    modify(
        "sketch.pattern_rect",
        Group::Create,
        "⣿",
        &[],
        Modify::Pattern(false),
    ),
    modify(
        "sketch.pattern_circ",
        Group::Create,
        "❂",
        &[],
        Modify::Pattern(true),
    ),
    // Mirror creates geometry too (design §2, §6.9, #215).
    modify("sketch.mirror", Group::Create, "⫴", &[], Modify::Mirror),
    // ── Modify ──
    modify(
        "modify.trim",
        Group::Modify,
        "✂",
        &[key(Key::T)],
        Modify::Trim,
    ),
    modify("modify.extend", Group::Modify, "⇥", &[], Modify::Extend),
    modify("modify.break", Group::Modify, "⌇", &[], Modify::Break),
    modify("modify.fillet", Group::Modify, "◜", &[], Modify::Fillet),
    modify("modify.chamfer", Group::Modify, "◸", &[], Modify::Chamfer),
    // Three tools sharing one dialog and manipulator (design §6.7, #219).
    modify(
        "modify.move",
        Group::Modify,
        "✥",
        &[key(Key::M)],
        Modify::Transform(TransformMode::Move),
    ),
    modify(
        "modify.rotate",
        Group::Modify,
        "⟳",
        &[],
        Modify::Transform(TransformMode::Rotate),
    ),
    modify(
        "modify.scale",
        Group::Modify,
        "⤡",
        &[],
        Modify::Transform(TransformMode::Scale),
    ),
    a(
        "edit.delete",
        "Delete",
        Group::Modify,
        "⌫",
        &["remove", "erase"],
        "Delete the selection, with the constraints and recipes that depend on it",
        &[key(Key::Delete), key(Key::Backspace)],
        KeyScope::Canvas,
        Op::Delete,
    ),
    a(
        "modify.construction",
        "Normal/Construction",
        Group::Modify,
        "┄",
        &["construction", "reference", "dashed", "linetype"],
        "Select curves to toggle between normal and construction",
        &[key(Key::X)],
        KeyScope::Canvas,
        Op::ToggleConstruction,
    ),
    a(
        "sketch.linetype_construction",
        "Construction linetype",
        Group::Create,
        "┈",
        &["construction", "linetype", "dashed"],
        "New geometry is construction: drawn dashed, constrainable, never exported",
        &[],
        KeyScope::Global,
        Op::Toggle(Flag::ConstructionLinetype),
    ),
    // ── Constrain ──
    a(
        "constrain.dimension",
        "Dimension",
        Group::Constrain,
        "⟷",
        &["sketch dimension", "measure", "length", "radius", "angle"],
        "Select a line, two points, a circle or arc…",
        &[key(Key::D)],
        KeyScope::CanvasSelect,
        Op::Dimension,
    ),
    constraint("constrain.coincident", "⦿", Kind::Coincident),
    constraint("constrain.horizontal", "⎯", Kind::Horizontal),
    constraint("constrain.vertical", "⏐", Kind::Vertical),
    constraint("constrain.parallel", "∥", Kind::Parallel),
    constraint("constrain.perpendicular", "⊥", Kind::Perpendicular),
    constraint("constrain.tangent", "◡", Kind::Tangent),
    constraint("constrain.collinear", "⋯", Kind::Collinear),
    constraint("constrain.concentric", "◎", Kind::Concentric),
    constraint("constrain.equal", "=", Kind::Equal),
    constraint("constrain.symmetric", "⫶", Kind::Symmetric),
    constraint("constrain.midpoint", "⊿", Kind::Midpoint),
    constraint("constrain.fix", "🔒", Kind::Fix),
    a(
        "constrain.auto",
        "Auto-constrain while drawing (hold Alt to suppress)",
        Group::Constrain,
        "⚡",
        &["infer", "snap"],
        "Infer coincident, horizontal and vertical constraints while drawing",
        &[],
        KeyScope::Global,
        Op::Toggle(Flag::AutoConstrain),
    ),
    // ── Edit ──
    a(
        "edit.repeat",
        "Repeat",
        Group::Edit,
        "⟳",
        &["again", "last command"],
        "Run the last tool or command again",
        &[],
        KeyScope::Global,
        Op::Repeat,
    ),
    a(
        "edit.undo",
        "Undo",
        Group::Edit,
        "↶",
        &[],
        "",
        &[cmd(Key::Z)],
        KeyScope::Global,
        Op::Undo,
    ),
    a(
        "edit.redo",
        "Redo",
        Group::Edit,
        "↷",
        &[],
        "",
        &[cmd_shift(Key::Z), cmd(Key::Y)],
        KeyScope::Global,
        Op::Redo,
    ),
    a(
        "select.chain",
        "Select chain",
        Group::Select,
        "⛓",
        &["connected", "loop", "contour", "profile"],
        "Select curves, then add every curve joined end to end with them (or double-click a curve)",
        &[],
        KeyScope::Global,
        Op::SelectChain,
    ),
    a(
        "edit.select_all",
        "Select all",
        Group::Select,
        "⬚",
        &[],
        "",
        &[cmd(Key::A)],
        KeyScope::Global,
        Op::SelectAll,
    ),
    a(
        "edit.deselect",
        "Deselect",
        Group::Select,
        "⬚",
        &["clear selection"],
        "",
        &[],
        KeyScope::Global,
        Op::Deselect,
    ),
    // ── View ──
    a(
        "view.fit",
        "Fit to content",
        Group::View,
        "⛶",
        &["zoom"],
        "",
        &[],
        KeyScope::Global,
        Op::Fit,
    ),
    a(
        "view.machine_preview",
        "Machine preview",
        Group::Inspect,
        "⚙",
        &["kerf", "hatch"],
        "Show kerf- and blade-compensated paths and hatch fills, as they will be exported",
        &[],
        KeyScope::Global,
        Op::Toggle(Flag::MachinePreview),
    ),
    a(
        "view.stroke_width",
        "Stroke width",
        Group::Inspect,
        "▬",
        &["pen width", "kerf"],
        "Draw the pen width of Draw layers and the kerf of Cut layers at true scale, so the edges a dimension can measure to are visible",
        &[],
        KeyScope::Global,
        Op::Toggle(Flag::StrokeWidth),
    ),
    a(
        "view.shortcuts",
        "Keyboard shortcuts",
        Group::View,
        "⌨",
        &["keys", "help", "shortcut map"],
        "Show or hide the shortcut map pane",
        &[key(Key::Questionmark)],
        KeyScope::Global,
        Op::Shell(Shell::ToggleShortcuts),
    ),
    a(
        "view.reset_layout",
        "Reset layout",
        Group::View,
        "⟲",
        &["panes", "dock", "default", "windows"],
        "Put every pane back where it starts: Browser left, Sketch palette over Inspector right",
        &[],
        KeyScope::Global,
        Op::Shell(Shell::ResetLayout),
    ),
];

/// One entry in a menu or strip laid out from the registry.
#[derive(Clone, Copy, Debug)]
pub enum Item {
    Action(&'static str),
    Separator,
}

use Item::{Action as A, Separator as Sep};

const DRAWING: [Item; 15] = [
    A("sketch.point"),
    A("sketch.line"),
    A("sketch.rect_2pt"),
    A("sketch.rect_centre"),
    A("sketch.rect_3pt"),
    A("sketch.circle_centre"),
    A("sketch.circle_2pt"),
    A("sketch.circle_3pt"),
    A("sketch.arc_3pt"),
    A("sketch.arc_centre"),
    A("sketch.arc_tangent"),
    A("sketch.polygon"),
    A("sketch.slot"),
    A("sketch.pen"),
    A("sketch.text"),
];

const CONSTRAINTS: [Item; 12] = [
    A("constrain.coincident"),
    A("constrain.horizontal"),
    A("constrain.vertical"),
    A("constrain.parallel"),
    A("constrain.perpendicular"),
    A("constrain.tangent"),
    A("constrain.collinear"),
    A("constrain.concentric"),
    A("constrain.equal"),
    A("constrain.symmetric"),
    A("constrain.midpoint"),
    A("constrain.fix"),
];

const fn concat<const N: usize>(parts: &[&[Item]]) -> [Item; N] {
    let mut out = [Sep; N];
    let mut n = 0;
    let mut p = 0;
    while p < parts.len() {
        let mut i = 0;
        while i < parts[p].len() {
            out[n] = parts[p][i];
            n += 1;
            i += 1;
        }
        p += 1;
    }
    assert!(n == N, "layout length mismatch");
    out
}

/// Sketch menu: the drawing tools, then the linetype for new geometry.
pub const SKETCH_MENU: &[Item] =
    &concat::<17>(&[&DRAWING, &[Sep, A("sketch.linetype_construction")]]);

/// Constrain menu.
pub const CONSTRAIN_MENU: &[Item] = &concat::<16>(&[
    &CONSTRAINTS,
    &[Sep, A("constrain.dimension"), Sep, A("constrain.auto")],
]);

/// The marking menu's ring (design §4, #290), clockwise from the top:
/// N, NE, E, SE, S, SW, W, NW. It never changes with context, so a flick
/// toward a slot becomes muscle memory; actions that can't run are dimmed.
pub const MARKING_RING: [&str; 8] = [
    "edit.repeat",
    "edit.undo",
    "modify.move",
    "modify.offset",
    "edit.delete",
    "modify.construction",
    "modify.trim",
    "constrain.dimension",
];

/// The marking menu's list under the ring (design §4): it depends on
/// whether anything is selected. Constraints that fit the selection follow
/// it under "Constrain" (see [`constraints_for`]).
#[must_use]
pub fn marking_list(selected: bool) -> &'static [Item] {
    if selected {
        &[
            A("select.chain"),
            A("modify.construction"),
            A("edit.delete"),
            Sep,
            A("edit.deselect"),
            A("edit.select_all"),
        ]
    } else {
        &[A("edit.select_all"), A("view.fit"), Sep, A("edit.redo")]
    }
}

/// The constraint actions the selection supports now.
#[must_use]
pub fn constraints_for<'a>(reg: &'a Registry, state: &AppState) -> Vec<&'a Action> {
    CONSTRAINTS
        .iter()
        .filter_map(|i| match i {
            Item::Action(id) => reg.get(id),
            Item::Separator => None,
        })
        .filter(|a| reg.enabled(a, state).yes())
        .collect()
}

pub const EDIT_MENU: &[Item] = &[
    A("edit.undo"),
    A("edit.redo"),
    Sep,
    A("edit.select_all"),
    A("edit.deselect"),
    Sep,
    A("modify.construction"),
];

/// The View menu's action part; panes, theme and units follow it.
pub const VIEW_MENU: &[Item] = &[
    A("view.fit"),
    A("view.machine_preview"),
    A("view.stroke_width"),
];

/// The View menu's layout part, after the panes.
pub const LAYOUT_MENU: &[Item] = &[A("view.reset_layout")];

/// Help menu: the shortcut map (design §5.1).
pub const HELP_MENU: &[Item] = &[A("view.shortcuts")];

/// Every action, built-in and plugin, plus the key map.
pub struct Registry {
    actions: Vec<Action>,
    /// `(shortcut, action index)`, most modifiers first, so Ctrl+Shift+Z is
    /// tried before Ctrl+Z (which would also match it).
    keys: Vec<(KeyboardShortcut, usize)>,
    /// The plugin commands the actions were last built from.
    plugin_specs: Vec<PluginAction>,
}

/// One plugin command, as the plugin host lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginAction {
    pub plugin: String,
    pub name: String,
    pub kind: PluginKind,
}

/// The plugin commands a set of installed plugins contributes: one per
/// generator and one per modifier, in install order (design §6.13).
pub fn plugin_actions<'m>(
    manifests: impl Iterator<Item = &'m locus_plugin_host::manifest::Manifest>,
) -> Vec<PluginAction> {
    use locus_plugin_host::types::Kind as K;
    let manifests: Vec<_> = manifests.collect();
    let of = |want: K, kind: PluginKind| {
        manifests
            .iter()
            .filter(move |m| m.kinds.contains(&want))
            .map(move |m| PluginAction {
                plugin: m.id.clone(),
                name: m.name.clone(),
                kind,
            })
    };
    of(K::Generator, PluginKind::Generator)
        .chain(of(K::Modifier, PluginKind::Modifier))
        .collect()
}

impl Default for Registry {
    fn default() -> Self {
        let mut r = Self {
            actions: BUILTIN.to_vec(),
            keys: Vec::new(),
            plugin_specs: Vec::new(),
        };
        r.index_keys();
        r
    }
}

impl Registry {
    fn index_keys(&mut self) {
        self.keys = self
            .actions
            .iter()
            .enumerate()
            .flat_map(|(i, a)| a.keys.iter().map(move |k| (*k, i)))
            .collect();
        let weight = |m: Modifiers| {
            usize::from(m.alt) + usize::from(m.shift) + usize::from(m.command || m.ctrl)
        };
        self.keys
            .sort_by_key(|(k, _)| std::cmp::Reverse(weight(k.modifiers)));
    }

    pub fn actions(&self) -> &[Action] {
        &self.actions
    }

    pub fn get(&self, id: &str) -> Option<&Action> {
        self.actions.iter().find(|a| a.id.as_str() == id)
    }

    /// Replace the plugin actions with these (when plugins are installed,
    /// enabled, disabled or removed). Cheap when nothing changed, so the
    /// app can call it every frame.
    pub fn sync_plugins(&mut self, specs: Vec<PluginAction>) {
        if specs == self.plugin_specs {
            return;
        }
        self.actions.retain(|a| a.source == Source::Builtin);
        for s in &specs {
            let (verb, label, group, hint) = match s.kind {
                PluginKind::Generator => (
                    "insert",
                    format!("Insert {}…", s.name),
                    Group::Insert,
                    String::new(),
                ),
                PluginKind::Modifier => (
                    "apply",
                    format!("Apply {} to selection…", s.name),
                    Group::Modify,
                    "Select a chain of lines and arcs, or a circle.".to_owned(),
                ),
            };
            self.actions.push(Action {
                id: ActionId(Cow::Owned(format!("plugin.{}.{verb}", s.plugin))),
                label: Cow::Owned(label),
                group,
                icon: "🧩",
                keywords: &["plugin"],
                hint: Cow::Owned(hint),
                keys: &[],
                scope: KeyScope::Global,
                op: Op::Plugin {
                    plugin: s.plugin.clone(),
                    kind: s.kind,
                },
                source: Source::Plugin(s.plugin.clone()),
            });
        }
        self.plugin_specs = specs;
        self.index_keys();
    }

    /// Plugin actions of one kind, in install order.
    pub fn plugin_actions(&self, kind: PluginKind) -> impl Iterator<Item = &Action> {
        self.actions
            .iter()
            .filter(move |a| matches!(&a.op, Op::Plugin { kind: k, .. } if *k == kind))
    }

    /// The action Repeat would run: the last tool or command that ran.
    pub fn repeatable(&self, canvas: &AppState) -> Option<&Action> {
        canvas
            .tools
            .last_action
            .as_deref()
            .and_then(|id| self.get(id))
    }

    /// The first key bound to an action, as menus show it.
    pub fn shortcut(&self, a: &Action) -> Option<KeyboardShortcut> {
        a.keys.first().copied()
    }

    /// Can `a` run now, and if not, why.
    pub fn enabled(&self, a: &Action, canvas: &AppState) -> Enabled {
        match &a.op {
            Op::Constrain(k) => {
                Enabled::when(canvas.can_constrain(*k), format!("Select {}", k.needs()))
            }
            Op::Dimension => Enabled::when(canvas.can_dimension(), a.hint.clone()),
            Op::Undo => Enabled::when(canvas.undo_label().is_some(), "Nothing to undo"),
            Op::Redo => Enabled::when(canvas.redo_label().is_some(), "Nothing to redo"),
            Op::Delete => Enabled::when(!canvas.is_drawing(), "Finish the shape being drawn"),
            Op::Repeat => match self.repeatable(canvas) {
                Some(last) => self.enabled(last, canvas),
                None => Enabled::No("Nothing to repeat yet".into()),
            },
            Op::SelectChain => Enabled::when(!canvas.selection.items.is_empty(), a.hint.clone()),
            Op::ToggleConstruction => {
                Enabled::when(canvas.construction_toggle().is_some(), a.hint.clone())
            }
            Op::Plugin {
                kind: PluginKind::Modifier,
                ..
            } => Enabled::when(!canvas.selected_curves().is_empty(), a.hint.clone()),
            _ => Enabled::Yes,
        }
    }

    /// For a tool: is it the active one. For a toggle: is it on.
    pub fn is_on(&self, a: &Action, canvas: &AppState, settings: &Settings) -> bool {
        match &a.op {
            Op::Draw(t) => canvas.tool() == *t,
            Op::Modify(m) => canvas.modify() == Some(*m),
            Op::Toggle(Flag::AutoConstrain) => settings.auto_constrain,
            Op::Toggle(Flag::MachinePreview) => canvas.machine_preview(),
            Op::Toggle(Flag::StrokeWidth) => canvas.stroke_preview(),
            Op::Toggle(Flag::ConstructionLinetype) => canvas.doc.construction_linetype,
            _ => false,
        }
    }

    /// The label to show now: undo and redo name the step ("Undo Add line").
    pub fn label<'a>(&self, a: &'a Action, canvas: &AppState) -> Cow<'a, str> {
        let step = match a.op {
            Op::Undo => canvas.undo_label(),
            Op::Redo => canvas.redo_label(),
            Op::Repeat => self.repeatable(canvas).map(|l| l.label.as_ref()),
            _ => None,
        };
        match step {
            Some(s) => Cow::Owned(format!("{} {s}", a.label)),
            None => Cow::Borrowed(&a.label),
        }
    }

    /// Run `a`. A disabled command explains itself in the status bar
    /// rather than doing nothing silently.
    pub fn run(&self, a: &Action, cx: &mut Cx<'_>) {
        let canvas = &mut *cx.state;
        if a.repeatable() {
            canvas.tools.last_action = Some(a.id.as_str().to_owned());
        }
        match &a.op {
            Op::Repeat => {
                if let Some(last) = self.repeatable(canvas) {
                    self.run(last, cx);
                }
            }
            Op::Draw(t) => canvas.set_tool(*t),
            Op::Modify(m) => canvas.set_modify(*m),
            Op::Constrain(k) => canvas.constrain(*k),
            Op::Dimension => canvas.start_dimension(cx.settings.units),
            Op::Undo => {
                canvas.undo();
            }
            Op::Redo => {
                canvas.redo();
            }
            Op::SelectAll => canvas.select_all(),
            Op::SelectChain => canvas.select_chains(),
            Op::Deselect => canvas.clear_selection(),
            Op::Delete => {
                if self.enabled(a, canvas).yes() {
                    canvas.delete_selection();
                }
            }
            Op::ToggleConstruction => match canvas.construction_toggle() {
                Some(cmd) => {
                    canvas.apply_reporting(&cmd);
                }
                None => canvas.doc.notice = Some(a.hint.to_string()),
            },
            Op::Fit => canvas.request_fit(),
            Op::Shell(req) => canvas.view.shell.push(*req),
            Op::Toggle(f) => {
                let on = !self.is_on(a, canvas, cx.settings);
                match f {
                    Flag::AutoConstrain => cx.settings.auto_constrain = on,
                    Flag::MachinePreview => canvas.set_machine_preview(on),
                    Flag::StrokeWidth => canvas.set_stroke_preview(on),
                    Flag::ConstructionLinetype => canvas.doc.construction_linetype = on,
                }
            }
            Op::Plugin { plugin, kind } => {
                if let Some(p) = cx.plugins.as_deref_mut() {
                    p.run_action(plugin, *kind, canvas);
                }
            }
        }
    }

    /// Run the action whose key was pressed this frame, if any. One loop
    /// over the key map (design §7.1). A focused text field gets keys first;
    /// canvas-scoped keys also wait for the pointer to be over the canvas
    /// and for the canvas to have no typed value or dimension editor open.
    pub fn dispatch_keys(&self, ctx: &egui::Context, cx: &mut Cx<'_>, over_canvas: bool) {
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        let canvas_free = over_canvas && cx.state.accepts_keys();
        let select_mode = cx.state.tool() == Tool::Select && cx.state.modify().is_none();
        for &(sc, i) in &self.keys {
            let a = &self.actions[i];
            let live = match a.scope {
                KeyScope::Global => true,
                KeyScope::Canvas => canvas_free,
                KeyScope::CanvasSelect => canvas_free && select_mode,
            };
            if live && pressed(ctx, sc) {
                self.run(a, cx);
                return;
            }
        }
    }
}

/// Plain keys match with Shift but not Command or Alt held, as the
/// single-letter keys always have; chords are consumed so egui widgets
/// don't see them too.
fn pressed(ctx: &egui::Context, sc: KeyboardShortcut) -> bool {
    ctx.input_mut(|i| {
        if sc.modifiers == Modifiers::NONE {
            !(i.modifiers.command || i.modifiers.alt) && i.key_pressed(sc.logical_key)
        } else {
            i.consume_shortcut(&sc)
        }
    })
}

/// A shortcut as menus and the shortcuts pane show it. Punctuation reads
/// as itself ("?", not egui's "Questionmark"); egui's arrow symbols are not
/// in the bundled fonts, so other keys keep their names.
pub fn key_text(ctx: &egui::Context, sc: &KeyboardShortcut) -> String {
    let full = ctx.format_shortcut(sc);
    let sym = sc.logical_key.symbol_or_name();
    if sym.chars().all(|c| c.is_ascii_punctuation())
        && let Some(mods) = full.strip_suffix(sc.logical_key.name())
    {
        return format!("{mods}{sym}");
    }
    full
}

/// How a laid-out list is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    /// A drop-down menu: tools as selectable rows, commands as buttons with
    /// their key, toggles as checkboxes. Closes the menu on use.
    Menu,
    /// The same rows in a pane (the sketch palette): nothing to close.
    Pane,
}

/// Draw `items` from the registry. Returns true when an action ran.
pub fn show(
    ui: &mut egui::Ui,
    reg: &Registry,
    cx: &mut Cx<'_>,
    items: &[Item],
    style: Style,
) -> bool {
    let mut ran = false;
    for item in items {
        match item {
            Item::Separator => {
                ui.separator();
            }
            Item::Action(id) => {
                if let Some(a) = reg.get(id) {
                    ran |= show_one(ui, reg, cx, a, style);
                }
            }
        }
    }
    ran
}

/// Draw one action; runs it on click. Returns true when it ran.
pub fn show_one(
    ui: &mut egui::Ui,
    reg: &Registry,
    cx: &mut Cx<'_>,
    a: &Action,
    style: Style,
) -> bool {
    let key = reg.shortcut(a).map(|k| key_text(ui.ctx(), &k));
    let label = reg.label(a, cx.state);
    let clicked = match (a.op.kind(), style) {
        (ActionKind::Toggle, _) => {
            let mut on = reg.is_on(a, cx.state, cx.settings);
            let r = ui.checkbox(&mut on, label.as_ref());
            let r = if a.hint.is_empty() {
                r
            } else {
                r.on_hover_text(a.hint.as_ref())
            };
            r.changed()
        }
        (ActionKind::Tool, _) => {
            let text = match &key {
                Some(k) => format!("{label}  ({k})"),
                None => label.into_owned(),
            };
            ui.selectable_label(reg.is_on(a, cx.state, cx.settings), text)
                .clicked()
        }
        (ActionKind::Command, _) => {
            let enabled = reg.enabled(a, cx.state);
            let mut b = egui::Button::new(label.as_ref());
            if let Some(k) = &key {
                b = b.shortcut_text(k.as_str());
            }
            let r = ui.add_enabled(enabled.yes(), b);
            let r = match &enabled {
                Enabled::No(why) => r.on_disabled_hover_text(why.as_ref()),
                Enabled::Yes => r,
            };
            r.clicked()
        }
    };
    if clicked {
        reg.run(a, cx);
        if style == Style::Menu {
            ui.close();
        }
    }
    clicked
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn ids_are_unique_and_namespaced() {
        let r = Registry::default();
        let mut seen = BTreeSet::new();
        for a in r.actions() {
            assert!(seen.insert(a.id.as_str()), "duplicate id {}", a.id.as_str());
            let ns = a.id.as_str().split('.').next().unwrap_or_default();
            assert!(
                ["select", "sketch", "modify", "constrain", "edit", "view"].contains(&ns),
                "{}",
                a.id.as_str()
            );
        }
    }

    #[test]
    fn no_two_actions_share_a_key() {
        let r = Registry::default();
        let mut seen = BTreeSet::new();
        for (k, _) in &r.keys {
            assert!(seen.insert(format!("{k:?}")), "key bound twice: {k:?}");
        }
    }

    #[test]
    fn constraints_have_no_keys() {
        // Owner decision (design §2, §5): symbols only.
        for a in BUILTIN {
            if let Op::Constrain(_) = a.op {
                assert!(a.keys.is_empty(), "{} has a key", a.id.as_str());
                assert!(!a.icon.is_empty(), "{} has no symbol", a.id.as_str());
            }
        }
    }

    #[test]
    fn the_design_shortcut_map_holds_for_existing_actions() {
        let r = Registry::default();
        let bound = |id: &str| r.get(id).and_then(|a| r.shortcut(a)).map(|k| k.logical_key);
        for (id, k) in [
            ("sketch.line", Key::L),
            ("sketch.rect_2pt", Key::R),
            ("sketch.circle_centre", Key::C),
            ("sketch.arc_3pt", Key::A),
            ("constrain.dimension", Key::D),
            ("modify.trim", Key::T),
            ("modify.offset", Key::O),
            ("modify.move", Key::M),
            ("sketch.point", Key::P),
            ("sketch.polygon", Key::G),
            ("sketch.slot", Key::U),
            ("sketch.pen", Key::N),
            ("edit.delete", Key::Delete),
            ("modify.construction", Key::X),
        ] {
            assert_eq!(bound(id), Some(k), "{id}");
        }
        // S is tool search (#299), Select is Esc.
        assert_eq!(bound("select.select"), None);
    }

    #[test]
    fn every_layout_names_a_registered_action() {
        let r = Registry::default();
        for list in [
            SKETCH_MENU,
            CONSTRAIN_MENU,
            marking_list(true),
            marking_list(false),
            &MARKING_RING.map(Item::Action),
            EDIT_MENU,
            VIEW_MENU,
            LAYOUT_MENU,
        ] {
            for item in list {
                if let Item::Action(id) = item {
                    assert!(r.get(id).is_some(), "{id} is not registered");
                }
            }
        }
        // Every drawing tool and every constraint is reachable.
        for t in Tool::DRAWING {
            assert!(r.actions().iter().any(|a| a.op == Op::Draw(t)), "{t:?}");
        }
        for k in Kind::ALL {
            assert!(
                r.actions().iter().any(|a| a.op == Op::Constrain(k)),
                "{k:?}"
            );
        }
        for m in Modify::ALL {
            assert!(r.actions().iter().any(|a| a.op == Op::Modify(m)), "{m:?}");
        }
    }

    #[test]
    fn enabled_predicates_follow_the_selection_and_history() {
        let r = Registry::default();
        let c = AppState::default();
        let get = |id| r.get(id).map(|a| r.enabled(a, &c));
        // Nothing selected: constraints say what they need.
        assert_eq!(
            get("constrain.parallel"),
            Some(Enabled::No("Select two lines".into()))
        );
        assert!(matches!(get("constrain.dimension"), Some(Enabled::No(_))));
        // A fresh document has no history.
        assert!(matches!(get("edit.undo"), Some(Enabled::No(_))));
        assert_eq!(get("sketch.line"), Some(Enabled::Yes));
        assert_eq!(get("edit.delete"), Some(Enabled::Yes));
    }

    #[test]
    fn plugin_actions_come_and_go_with_their_plugins() {
        let mut r = Registry::default();
        let n = r.actions().len();
        let gear = PluginAction {
            plugin: "gear".into(),
            name: "Spur gear".into(),
            kind: PluginKind::Generator,
        };
        let dog = PluginAction {
            plugin: "dogbone".into(),
            name: "Dogbone".into(),
            kind: PluginKind::Modifier,
        };
        r.sync_plugins(vec![gear.clone(), dog]);
        assert_eq!(r.actions().len(), n + 2);
        let g = r.get("plugin.gear.insert");
        assert!(g.is_some_and(|a| a.group == Group::Insert && a.label == "Insert Spur gear…"));
        let d = r.get("plugin.dogbone.apply");
        assert!(d.is_some_and(|a| a.group == Group::Modify));
        // A modifier needs a selection.
        let c = AppState::default();
        assert!(d.is_some_and(|a| !r.enabled(a, &c).yes()));
        // Removing one drops its action.
        r.sync_plugins(vec![gear]);
        assert!(r.get("plugin.dogbone.apply").is_none());
        assert_eq!(r.plugin_actions(PluginKind::Generator).count(), 1);
        r.sync_plugins(Vec::new());
        assert_eq!(r.actions().len(), n);
    }

    #[test]
    fn toggles_flip_their_setting() {
        let r = Registry::default();
        let mut canvas = AppState::default();
        let mut settings = Settings::default();
        let before = settings.auto_constrain;
        let mut cx = Cx {
            state: &mut canvas,
            settings: &mut settings,
            plugins: None,
        };
        if let Some(a) = r.get("constrain.auto") {
            r.run(a, &mut cx);
        }
        if let Some(a) = r.get("view.stroke_width") {
            r.run(a, &mut cx);
        }
        assert!(canvas.stroke_preview());
        assert_eq!(settings.auto_constrain, !before);
    }

    /// Shell actions only queue a request; the shell owns the dock (#327).
    #[test]
    fn shell_actions_queue_a_request() {
        let r = Registry::default();
        let mut canvas = AppState::default();
        let mut settings = Settings::default();
        let mut cx = Cx {
            state: &mut canvas,
            settings: &mut settings,
            plugins: None,
        };
        for id in ["view.reset_layout", "view.shortcuts"] {
            if let Some(a) = r.get(id) {
                r.run(a, &mut cx);
            }
        }
        assert_eq!(
            canvas.view.shell,
            vec![Shell::ResetLayout, Shell::ToggleShortcuts]
        );
    }

    /// Repeat (marking menu N slot) runs the last tool or command; undo,
    /// selection and view actions don't count as one.
    #[test]
    #[allow(clippy::unwrap_used)]
    fn repeat_runs_the_last_tool_again() {
        let r = Registry::default();
        let mut canvas = AppState::default();
        let mut settings = Settings::default();
        let repeat = r.get("edit.repeat").unwrap();
        assert!(!r.enabled(repeat, &canvas).yes(), "nothing to repeat yet");
        let mut cx = Cx {
            state: &mut canvas,
            settings: &mut settings,
            plugins: None,
        };
        r.run(r.get("modify.trim").unwrap(), &mut cx);
        r.run(r.get("select.select").unwrap(), &mut cx);
        r.run(r.get("view.fit").unwrap(), &mut cx);
        assert_eq!(cx.state.modify(), None);
        assert_eq!(r.label(repeat, cx.state), "Repeat Trim");
        r.run(repeat, &mut cx);
        assert_eq!(cx.state.modify(), Some(Modify::Trim));
    }

    #[test]
    fn chords_are_tried_before_their_unshifted_form() {
        let r = Registry::default();
        let pos = |id: &str| {
            r.keys
                .iter()
                .position(|(_, i)| r.actions[*i].id.as_str() == id)
                .unwrap_or(usize::MAX)
        };
        assert!(pos("edit.redo") < pos("edit.undo"));
    }
}
