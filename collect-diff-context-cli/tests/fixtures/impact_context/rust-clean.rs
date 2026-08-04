use std::collections::HashMap as Map;
use std::fmt::*;
use std::prelude::*;

#[derive(Debug)]
pub struct Service<T> {
    value: T,
}

pub enum Mode {
    Fast,
    Safe,
}

pub trait Runner {
    fn run(&self, value: u8) -> u8;
}

impl<T: Clone> Service<T> {
    pub fn new(value: T) -> Self {
        Self { value }
    }

    #[inline]
    pub async fn process<U>(&self, value: U) -> Result<U, String>
    where
        U: Clone + Send,
    {
        let mapper = |input: U| input.clone();
        tracing::debug!("processing");
        helper(value);
        Ok(mapper(value))
    }
}

impl Runner for Service<u8> {
    fn run(&self, value: u8) -> u8 {
        value + self.value
    }
}

pub type ServiceMap = Map<String, Service<u8>>;
pub const DEFAULT_MODE: Mode = Mode::Fast;
pub static ENABLED: bool = true;
pub mod nested {}

fn helper<T>(value: T) -> T {
    value
}

#[test]
#[ignore]
fn ignored_test() {
    let _ = Service::new(1_u8);
}
