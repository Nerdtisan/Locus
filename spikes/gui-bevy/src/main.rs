// SPDX-License-Identifier: AGPL-3.0-or-later
//! Spike B (#45, SPEC §12.1, §12.2, §13 M0): the reference canvas on
//! Bevy 0.17 + bevy_egui + egui_dock.
//!
//! - egui (through bevy_egui) draws the docked panels and owns pointer input.
//! - The canvas tab is transparent; a Bevy `Camera2d` renders into its
//!   rectangle (a camera viewport) and egui is composited over it by a second,
//!   window-sized camera.
//! - Geometry is drawn with Bevy gizmos, rebuilt each frame in canvas
//!   screen space from f64 mm (see `view.rs` for why).

mod auto;
mod canvas;
mod doc;
mod ui;
mod view;

use bevy::camera::visibility::RenderLayers;
use bevy::camera::{CameraOutputMode, CameraUpdateSystems, Viewport};
use bevy::prelude::*;
use bevy::render::render_resource::BlendState;
use bevy::window::{PresentMode, PrimaryWindow, WindowResolution};
use bevy::winit::WinitSettings;
use bevy_egui::{EguiGlobalSettings, EguiPlugin, EguiPostUpdateSet, EguiPrimaryContextPass};

fn main() {
    let mut app = App::new();
    app.add_plugins(DefaultPlugins.set(WindowPlugin {
        primary_window: Some(Window {
            title: "Locus — GUI spike B (Bevy)".into(),
            resolution: WindowResolution::new(1280, 800),
            present_mode: PresentMode::AutoVsync,
            // Web: render into the page's canvas and track its size.
            canvas: Some("#locus-canvas".into()),
            fit_canvas_to_parent: true,
            ..default()
        }),
        ..default()
    }))
    .add_plugins(EguiPlugin::default())
    // An editor redraws on input only; the benchmark switches to continuous.
    .insert_resource(WinitSettings::desktop_app())
    .insert_resource(ClearColor(canvas::BACKGROUND))
    .init_resource::<canvas::Canvas>()
    .init_resource::<ui::Dock>()
    .init_resource::<ui::Bench>()
    .init_gizmo_group::<canvas::Thick>()
    .add_systems(Startup, (setup_cameras, canvas::setup_gizmos))
    .add_systems(First, ui::frame_begin)
    .add_systems(Last, ui::frame_end)
    .add_systems(EguiPrimaryContextPass, ui::ui)
    .add_systems(
        PostUpdate,
        (apply_viewport, canvas::draw, ui::bench_tick)
            .after(EguiPostUpdateSet::EndPass)
            .before(CameraUpdateSystems),
    );
    if let Some(auto) = auto::Auto::requested() {
        app.insert_resource(auto).add_systems(Update, auto::run);
    }
    app.run();
}

/// Marks the camera that renders the canvas.
#[derive(Component)]
struct CanvasCamera;

/// Two cameras, the pattern proven in the user's Bevy simulator: the canvas
/// camera renders only inside the canvas tab's rectangle; the egui camera
/// covers the window and is blended over it, so the canvas shows through the
/// transparent tab.
fn setup_cameras(mut commands: Commands, mut egui: ResMut<EguiGlobalSettings>) {
    egui.auto_create_primary_context = false;
    commands.spawn((
        CanvasCamera,
        Camera2d,
        Camera {
            order: 0,
            ..default()
        },
    ));
    commands.spawn((
        bevy_egui::PrimaryEguiContext,
        Camera2d,
        // Draws no gizmos or meshes, only egui.
        RenderLayers::none(),
        Camera {
            order: 1,
            // egui writes premultiplied alpha.
            output_mode: CameraOutputMode::Write {
                blend_state: Some(BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                clear_color: ClearColorConfig::None,
            },
            clear_color: ClearColorConfig::Custom(Color::NONE),
            ..default()
        },
    ));
}

/// Points the canvas camera's viewport at the canvas tab (or switches it off
/// while the tab is hidden).
fn apply_viewport(
    canvas: Res<canvas::Canvas>,
    windows: Query<&Window, With<PrimaryWindow>>,
    mut cameras: Query<&mut Camera, With<CanvasCamera>>,
) {
    let wanted = canvas.rect.and_then(|r| {
        let window = windows.single().ok()?;
        let (max_w, max_h) = (window.physical_width(), window.physical_height());
        if max_w == 0 || max_h == 0 {
            return None;
        }
        let ppp = canvas.ppp;
        // Clamped inside the window: wgpu rejects a viewport that overhangs the target.
        let x = ((r.min.x * ppp).round().max(0.0) as u32).min(max_w - 1);
        let y = ((r.min.y * ppp).round().max(0.0) as u32).min(max_h - 1);
        let w = ((r.width() * ppp).round().max(1.0) as u32).min(max_w - x);
        let h = ((r.height() * ppp).round().max(1.0) as u32).min(max_h - y);
        Some(Viewport {
            physical_position: UVec2::new(x, y),
            physical_size: UVec2::new(w, h),
            ..default()
        })
    });
    for mut camera in &mut cameras {
        let same = match (&camera.viewport, &wanted) {
            (Some(a), Some(b)) => {
                a.physical_position == b.physical_position && a.physical_size == b.physical_size
            }
            (None, None) => true,
            _ => false,
        };
        if !same {
            camera.viewport = wanted.clone();
        }
        let active = wanted.is_some();
        if camera.is_active != active {
            camera.is_active = active;
        }
    }
}
