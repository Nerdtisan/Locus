// SPDX-License-Identifier: MIT OR Apache-2.0

//! A minimal Locus generator plugin: a rectangle whose sides carry
//! horizontal and vertical constraints. Copy this folder to start your own
//! (see README.md).
//!
//! Rules every plugin follows (SPEC §15.3–§15.5):
//! - No I/O of any kind. The sandbox gives a plugin nothing to call.
//! - Same parameters in, same geometry out. Need randomness? Take a seed
//!   parameter.
//! - Millimetres and radians, f64.

use locus_plugin_api::prelude::*;

struct Rectangle;

impl InfoGuest for Rectangle {
    fn describe() -> PluginInfo {
        // Must match plugin.toml.
        PluginInfo {
            id: "com.example.my-plugin".into(),
            name: "Rectangle".into(),
            version: "0.1.0".into(),
        }
    }
}

impl GeneratorGuest for Rectangle {
    /// The parameters; Locus draws the panel from this.
    fn schema() -> Vec<ParamSpec> {
        vec![
            number("width", "Width", "mm", 1.0, 1000.0, 40.0),
            number("height", "Height", "mm", 1.0, 1000.0, 25.0),
        ]
    }

    fn generate(params: Vec<Param>) -> Result<Generated, PluginError> {
        let p = Params::new(&params);
        let w = p.number_in("width", 1.0, 1000.0)?;
        let h = p.number_in("height", 1.0, 1000.0)?;

        let mut sketch = Sketch::new();
        let sides = sketch.polygon(&[(0.0, 0.0), (w, 0.0), (w, h), (0.0, h)]);
        sketch.constrain(Constraint::Horizontal(sides[0]));
        sketch.constrain(Constraint::Vertical(sides[1]));
        sketch.constrain(Constraint::Horizontal(sides[2]));
        sketch.constrain(Constraint::Vertical(sides[3]));
        Ok(sketch.finish())
    }
}

locus_plugin_api::export_plugin!(Rectangle: generator);
