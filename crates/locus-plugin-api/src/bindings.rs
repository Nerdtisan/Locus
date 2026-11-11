// SPDX-License-Identifier: MIT OR Apache-2.0

//! Guest bindings generated from `wit/plugin.wit` by `wit-bindgen`.
//!
//! One SDK-only world per interface rather than one per plugin kind: `info`
//! is exported by every kind, and generating it once lets a plugin implement
//! any combination of kinds (`export_plugin!`). The shared `types` and
//! `params` interfaces are generated once in [`shared`] and reused.
//! `wit-component` merges the pieces when the plugin is packed.

pub mod shared {
    wit_bindgen::generate!({
        inline: "package locus:sdk; world shared { import locus:plugin/types@1.0.0; import locus:plugin/params@1.0.0; }",
        path: "wit",
        world: "locus:sdk/shared",
        generate_all,
    });
}

pub mod info {
    wit_bindgen::generate!({
        inline: "package locus:sdk; world info { export locus:plugin/info@1.0.0; }",
        path: "wit",
        world: "locus:sdk/info",
        pub_export_macro: true,
        export_macro_name: "export_info",
        default_bindings_module: "locus_plugin_api::info",
    });
}

pub mod generator {
    wit_bindgen::generate!({
        inline: "package locus:sdk; world generator { export locus:plugin/generator@1.0.0; }",
        path: "wit",
        world: "locus:sdk/generator",
        pub_export_macro: true,
        export_macro_name: "export_generator",
        default_bindings_module: "locus_plugin_api::generator",
        with: {
            "locus:plugin/types@1.0.0": crate::shared::locus::plugin::types,
            "locus:plugin/params@1.0.0": crate::shared::locus::plugin::params,
        },
    });
}

pub mod modifier {
    wit_bindgen::generate!({
        inline: "package locus:sdk; world modifier { export locus:plugin/modifier@1.0.0; }",
        path: "wit",
        world: "locus:sdk/modifier",
        pub_export_macro: true,
        export_macro_name: "export_modifier",
        default_bindings_module: "locus_plugin_api::modifier",
        with: {
            "locus:plugin/types@1.0.0": crate::shared::locus::plugin::types,
            "locus:plugin/params@1.0.0": crate::shared::locus::plugin::params,
        },
    });
}

pub mod exporter {
    wit_bindgen::generate!({
        inline: "package locus:sdk; world exporter { export locus:plugin/exporter@1.0.0; }",
        path: "wit",
        world: "locus:sdk/exporter",
        pub_export_macro: true,
        export_macro_name: "export_exporter",
        default_bindings_module: "locus_plugin_api::exporter",
        with: {
            "locus:plugin/types@1.0.0": crate::shared::locus::plugin::types,
        },
    });
}
