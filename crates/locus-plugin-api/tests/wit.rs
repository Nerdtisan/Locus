// SPDX-License-Identifier: MIT OR Apache-2.0

//! The plugin interface in `wit/` must stay valid WIT (#137, SPEC §15.3).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;
use wit_parser::Resolve;

fn resolve() -> (Resolve, wit_parser::PackageId) {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("wit");
    let mut resolve = Resolve::new();
    let (pkg, _) = resolve.push_dir(&dir).expect("wit/ parses and resolves");
    (resolve, pkg)
}

#[test]
fn package_is_locus_plugin_1_0_0() {
    let (resolve, pkg) = resolve();
    let name = &resolve.packages[pkg].name;
    assert_eq!(name.namespace, "locus");
    assert_eq!(name.name, "plugin");
    assert_eq!(
        name.version.as_ref().map(ToString::to_string).as_deref(),
        Some("1.0.0")
    );
}

#[test]
fn every_plugin_kind_has_a_world_that_imports_nothing() {
    let (resolve, pkg) = resolve();
    for kind in ["generator", "modifier", "exporter", "importer", "validator"] {
        let world = resolve
            .select_world(&[pkg], Some(&format!("{kind}-plugin")))
            .unwrap();
        let w = &resolve.worlds[world];
        // Interfaces used only for types may appear as imports; no functions may.
        let imported_funcs = w.imports.values().any(|item| match item {
            wit_parser::WorldItem::Function(..) => true,
            wit_parser::WorldItem::Interface { id, .. } => {
                !resolve.interfaces[*id].functions.is_empty()
            }
            wit_parser::WorldItem::Type { .. } => false,
        });
        assert!(
            !imported_funcs,
            "{kind}-plugin must import no functions (§15.4 deny by default)"
        );
        assert_eq!(w.exports.len(), 2, "{kind}-plugin exports info + {kind}");
    }
}
