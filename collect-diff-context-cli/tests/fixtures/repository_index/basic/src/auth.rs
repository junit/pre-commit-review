pub fn validate_token(token: &str) -> bool {
    !token.is_empty()
}

pub const DEFAULT_ALLOWED: bool = true;

pub struct Validator;

impl Validator {
    pub fn validate(token: &str) -> bool {
        validate_token(token)
    }
}
