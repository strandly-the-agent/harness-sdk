//! P-497: does rustc still print `String` (trimmed) in a crate that depends on strands?
pub fn a() { let _: u8 = String::new(); }
