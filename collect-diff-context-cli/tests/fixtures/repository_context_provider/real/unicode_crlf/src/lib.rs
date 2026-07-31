pub fn 计算(value: i32) -> i32 {
    value + 1
}

pub fn seed() -> i32 {
    计算(41)
}

pub fn caller() -> i32 {
    seed()
}
