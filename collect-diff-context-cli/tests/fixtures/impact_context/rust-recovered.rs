fn broken_outside() {
    let value = @;
}

pub fn stable(value: u8) -> u8 {
    value + 1
}

pub fn degraded(value: u8) -> u8 {
    let next = @;
    value + next
}
