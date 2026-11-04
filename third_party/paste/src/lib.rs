// SPDX-License-Identifier: MIT OR Apache-2.0
//! Drop-in stand-in for the archived `paste` crate (RUSTSEC-2024-0436).
//! `paste`'s whole public API is the `paste!` macro; `pastey` keeps it.
#![no_std]

pub use pastey::paste;
