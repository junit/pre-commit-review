pub fn seed(value: i32) -> i32 {
    provider_real_shared::shared(value)
}

pub fn caller() -> i32 {
    seed(41)
}
