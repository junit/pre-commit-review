pub fn seed(value: i32) -> i32 {
    callee(value)
}

pub fn callee(value: i32) -> i32 {
    value + 1
}

pub fn caller() -> i32 {
    seed(41)
}
