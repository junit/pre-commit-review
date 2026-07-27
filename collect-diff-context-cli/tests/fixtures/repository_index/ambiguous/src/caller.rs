use crate::{a::*, b::*};

pub fn call(value: &str) -> bool {
    parse(value)
}

pub fn method(value: &str) -> usize {
    value.len()
}

pub fn generated() {
    tracing::debug!("generated");
}

#[cfg(feature = "optional")]
pub fn conditional(value: &str) -> bool {
    parse(value)
}

