//! #153 spike: does `solidcore` build here, and what does it cost to link?
//! It has no constraint solver, so the only honest measurement is footprint.
pub fn touch() -> solidcore::Tolerances {
    solidcore::Tolerances::default()
}
