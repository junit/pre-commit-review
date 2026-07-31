pub fn first(value: u8) -> u8 {
    if value == 0 {
        0
    } else {
        second(value - 1)
    }
}

pub fn second(value: u8) -> u8 {
    third(value)
}

pub fn third(value: u8) -> u8 {
    first(value)
}

pub fn seed() -> u8 {
    first(2)
}
